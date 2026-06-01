//! Per-WS-session repository-map cache.
//!
//! Phase 0 (PR #70): build the rendered map exactly once per WebSocket
//! session, on the first user message, via `tokio::task::spawn_blocking`
//! (the tree-sitter pass is CPU-bound and would otherwise block the runtime).
//! On subsequent turns the cached text is reused; no mtime polling, no
//! sqlite, no refresh. Refresh strategy + library/CLI parity defer to
//! follow-up PRs per the discovery memo.
//!
//! Ownership: the session cache is a function-local in `handle_socket`.
//! No `Arc<Mutex<…>>` — a single async task owns the cache for the lifetime
//! of one WebSocket connection.

use std::path::PathBuf;

use plaw_repo_map::build_for_root;

use crate::providers::traits::ChatMessage;

/// Build state for the per-session repo-map.
#[derive(Debug)]
pub(crate) struct RepoMapSession {
    /// Repo root that will be walked + parsed. Resolved from
    /// `RepoMapConfig::root` or falls back to the workspace dir.
    root: PathBuf,
    /// Token budget for the rendered map.
    max_tokens: usize,
    /// Rendered map text. `Some("")` means built-empty (e.g. no supported
    /// files); `None` means never built or build failed.
    rendered: Option<String>,
    /// True after `ensure_built` has been called at least once — guards
    /// against repeat spawn_blocking calls in the per-turn loop.
    build_attempted: bool,
    /// True after the rendered text has been spliced into a history once.
    /// Phase 0 injects exactly once per session (position 1, right after
    /// the system prompt) so the cacheable prefix stays stable across
    /// every subsequent turn.
    injected: bool,
}

impl RepoMapSession {
    pub(crate) fn new(root: PathBuf, max_tokens: usize) -> Self {
        Self {
            root,
            max_tokens,
            rendered: None,
            build_attempted: false,
            injected: false,
        }
    }

    /// Build the repo-map once. Idempotent. Failures (missing root, parse
    /// errors, etc.) log a warning and leave `rendered = None`, never
    /// propagate.
    pub(crate) async fn ensure_built(&mut self) {
        if self.build_attempted {
            return;
        }
        self.build_attempted = true;

        let root = self.root.clone();
        let max_tokens = self.max_tokens;
        let result = tokio::task::spawn_blocking(move || build_for_root(&root, max_tokens)).await;

        match result {
            Ok(Ok(map)) => {
                if map.is_empty() {
                    tracing::debug!(root = %self.root.display(), "repo-map built but empty");
                    self.rendered = Some(String::new());
                } else {
                    tracing::info!(
                        root = %self.root.display(),
                        files = map.file_count,
                        tags = map.tag_count,
                        tokens = map.tokens,
                        "repo-map built"
                    );
                    self.rendered = Some(map.text);
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, root = %self.root.display(), "repo-map build failed; injection skipped for this session");
            }
            Err(e) => {
                tracing::warn!(error = %e, "repo-map build task panicked or was cancelled");
            }
        }
    }

    /// Splice the rendered map into `history` once per session. Inserts at
    /// position 1 (right after the system prompt) so the static prefix
    /// `[system prompt][repo map]` stays bit-identical across all turns —
    /// any provider-side prefix cache then keeps hitting on the prefix.
    ///
    /// Returns `true` on the actual injection. Subsequent calls are no-ops.
    pub(crate) fn inject_once(&mut self, history: &mut Vec<ChatMessage>) -> bool {
        if self.injected {
            return false;
        }
        let Some(text) = self.rendered.as_deref().filter(|s| !s.trim().is_empty()) else {
            return false;
        };
        // Position 1 sits between system prompt at [0] and everything else.
        // Use saturating min in case `history` was unexpectedly emptied.
        let insert_at = 1.min(history.len());
        history.insert(
            insert_at,
            ChatMessage::system(format!("[Repository map]\n{}", text.trim())),
        );
        self.injected = true;
        tracing::info!(
            map_chars = text.len(),
            insert_at,
            "repo-map injected into history"
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &TempDir, rel: &str, content: &str) {
        let p = dir.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    #[tokio::test]
    async fn ensure_built_is_idempotent() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "lib.rs",
            "pub fn alpha() {}\npub fn beta() { alpha(); }\n",
        );
        let mut session = RepoMapSession::new(dir.path().to_path_buf(), 1024);

        session.ensure_built().await;
        let first = session.rendered.clone();
        session.ensure_built().await;
        let second = session.rendered.clone();

        assert_eq!(first, second);
        assert!(session.build_attempted);
    }

    #[tokio::test]
    async fn empty_repo_does_not_inject() {
        let dir = TempDir::new().unwrap();
        let mut session = RepoMapSession::new(dir.path().to_path_buf(), 1024);
        session.ensure_built().await;

        let mut history = vec![ChatMessage::system("base prompt")];
        let injected = session.inject_once(&mut history);

        assert!(!injected);
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn nonexistent_root_does_not_panic() {
        let mut session =
            RepoMapSession::new(PathBuf::from("/this/path/should/not/exist/xyz"), 1024);
        session.ensure_built().await;

        let mut history = vec![ChatMessage::system("base prompt")];
        let injected = session.inject_once(&mut history);

        assert!(!injected, "no injection when build fails");
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn inject_once_splices_at_position_1_then_no_ops() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "core.rs",
            "pub struct Hub {}\nimpl Hub { pub fn ping(&self) {} }\n",
        );
        write(
            &dir,
            "user.rs",
            "use crate::Hub;\nfn drive() { Hub {}.ping(); }\n",
        );

        let mut session = RepoMapSession::new(dir.path().to_path_buf(), 2048);
        session.ensure_built().await;

        let mut history = vec![
            ChatMessage::system("base prompt"),
            ChatMessage::user("hello"),
        ];

        assert!(session.inject_once(&mut history));
        assert_eq!(history.len(), 3);
        assert!(history[0].content.contains("base prompt"));
        assert!(history[1].content.starts_with("[Repository map]\n"));
        assert!(history[2].content.contains("hello"));

        // Second call must be a no-op.
        assert!(!session.inject_once(&mut history));
        assert_eq!(history.len(), 3);
    }
}
