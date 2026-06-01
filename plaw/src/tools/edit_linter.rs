//! Edit-with-linter — tree-sitter parse pass that gates `file_write` and
//! `file_edit` writes.
//!
//! The linter lives at the LAST point in each tool's call sequence before
//! `tokio::fs::write`: after security checks, rate-limit debiting, and
//! `record_action`, but before any disk mutation that could leave the
//! filesystem in a half-written state.
//!
//! Three modes, surfaced via [`EditLinterMode`]:
//!
//! - `Off` — bypass entirely.
//! - `Warn` — always allow the write; if parse errors exist, return the
//!   diagnostic so the caller can append it to the tool's `output` field.
//! - `Block` — reject the write only when the proposed content has
//!   STRICTLY MORE parse errors than the pre-edit content. file_write
//!   uses pre = `None` (treated as zero errors), so block-mode file_write
//!   rejects ANY parse errors. file_edit passes pre = `Some(&content)` so a
//!   refactor that doesn't make parse worse still proceeds.
//!
//! Escape hatches (cheapest first):
//! 1. Per-call: tool args may carry `"skip_lint": true` — bypass for that call.
//! 2. Per-path: [`EditLinterConfig::skip_paths`] glob list.
//! 3. Per-extension: [`EditLinterConfig::skip_extensions`].
//! 4. Per-size: [`EditLinterConfig::max_file_bytes`] — skip parse for big files.
//! 5. Per-session: `mode = "off"` or `enabled = false`.
//!
//! All decisions are returned as the [`Decision`] enum so the caller can
//! choose how to surface the result in its `ToolResult`.

use crate::config::{EditLinterConfig, EditLinterMode};
use plaw_repo_map::{parse_diagnostics, ParseReport};
use std::path::Path;

/// Per-write linter decision. The caller maps each variant to its own
/// `ToolResult` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Proceed unchanged.
    Allow,
    /// Proceed AND append `note` to the tool's `output` field.
    Warn(String),
    /// Refuse the write and surface `msg` as the tool's `error` field.
    Block(String),
}

/// Evaluate whether `new_content` should be written to `path`.
///
/// `pre_content` is the file's text BEFORE the edit (file_edit only).
/// `file_write` passes `None` — its baseline is "no file", treated as zero
/// pre-existing parse errors.
///
/// Returns [`Decision::Allow`] in any of these cases:
///   - `cfg.enabled = false`
///   - `cfg.mode = Off`
///   - `skip_lint = true`
///   - path matches `cfg.skip_paths` or extension in `cfg.skip_extensions`
///   - `Lang::from_path` returns `None` (unsupported source extension)
///   - content size exceeds `cfg.max_file_bytes`
///   - parse reports zero `ERROR` + `MISSING` nodes
///
/// Returns [`Decision::Warn`] when `mode = Warn` and parse reports > 0 problems.
/// Returns [`Decision::Block`] when `mode = Block` and new_content has
/// STRICTLY MORE problems than `pre_content`.
pub(crate) fn evaluate(
    cfg: &EditLinterConfig,
    path: &str,
    new_content: &str,
    pre_content: Option<&str>,
    skip_lint: bool,
) -> Decision {
    if !cfg.enabled || cfg.mode == EditLinterMode::Off || skip_lint {
        return Decision::Allow;
    }
    if path_skipped(path, &cfg.skip_paths) {
        return Decision::Allow;
    }
    if extension_skipped(path, &cfg.skip_extensions) {
        return Decision::Allow;
    }
    if new_content.len() > cfg.max_file_bytes {
        return Decision::Allow;
    }

    let new_report = match parse_diagnostics(Path::new(path), new_content) {
        Ok(Some(r)) => r,
        Ok(None) => return Decision::Allow,
        Err(_) => return Decision::Allow,
    };
    if new_report.is_clean() {
        return Decision::Allow;
    }

    match cfg.mode {
        EditLinterMode::Off => Decision::Allow,
        EditLinterMode::Warn => Decision::Warn(format_diagnostic(path, &new_report)),
        EditLinterMode::Block => {
            let pre_problems = pre_content
                .and_then(|src| parse_diagnostics(Path::new(path), src).ok().flatten())
                .map(|r| r.problem_count())
                .unwrap_or(0);
            if new_report.problem_count() > pre_problems {
                Decision::Block(format!(
                    "edit_linter blocked write: {}",
                    format_diagnostic(path, &new_report)
                ))
            } else {
                Decision::Allow
            }
        }
    }
}

fn format_diagnostic(path: &str, report: &ParseReport) -> String {
    let lines = if report.first_error_lines.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = report
            .first_error_lines
            .iter()
            .map(|n| n.to_string())
            .collect();
        format!(" (first error line(s): {})", parts.join(", "))
    };
    format!(
        "tree-sitter parse: {} ERROR + {} MISSING node(s) in {path}{lines}",
        report.error_nodes, report.missing_nodes
    )
}

fn extension_skipped(path: &str, skip_extensions: &[String]) -> bool {
    let lower = path.to_lowercase();
    skip_extensions.iter().any(|ext| {
        let needle = ext.to_lowercase();
        if needle.starts_with('.') {
            lower.ends_with(&needle)
        } else {
            lower.ends_with(&format!(".{needle}"))
        }
    })
}

/// Match `path` against a list of glob patterns using [`globset`]. Honors
/// `*` (any non-separator chars) and `**` (zero or more path segments).
/// Backslashes are normalized to forward slashes before matching so Windows
/// paths interact predictably with Unix-style globs in config.
pub(crate) fn path_matches_any(path: &str, globs: &[String]) -> bool {
    if globs.is_empty() {
        return false;
    }
    let normalized = path.replace('\\', "/");
    let mut builder = globset::GlobSetBuilder::new();
    for g in globs {
        if let Ok(parsed) = globset::Glob::new(g) {
            builder.add(parsed);
        }
    }
    match builder.build() {
        Ok(set) => set.is_match(Path::new(&normalized)),
        Err(_) => false,
    }
}

fn path_skipped(path: &str, globs: &[String]) -> bool {
    path_matches_any(path, globs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_warn() -> EditLinterConfig {
        EditLinterConfig {
            enabled: true,
            mode: EditLinterMode::Warn,
            ..EditLinterConfig::default()
        }
    }

    fn cfg_block() -> EditLinterConfig {
        EditLinterConfig {
            enabled: true,
            mode: EditLinterMode::Block,
            ..EditLinterConfig::default()
        }
    }

    fn cfg_off() -> EditLinterConfig {
        EditLinterConfig {
            enabled: true,
            mode: EditLinterMode::Off,
            ..EditLinterConfig::default()
        }
    }

    #[test]
    fn evaluate_disabled_returns_allow() {
        let mut cfg = cfg_warn();
        cfg.enabled = false;
        let d = evaluate(&cfg, "a.rs", "pub fn ( {", None, false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_mode_off_returns_allow() {
        let d = evaluate(&cfg_off(), "a.rs", "pub fn ( {", None, false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_skip_lint_true_returns_allow() {
        let d = evaluate(&cfg_warn(), "a.rs", "pub fn ( {", None, true);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_unsupported_extension_returns_allow() {
        let d = evaluate(&cfg_warn(), "README.md", "# anything", None, false);
        assert_eq!(d, Decision::Allow);
        let d = evaluate(&cfg_block(), "config.toml", "invalid = ", None, false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_skip_extensions_match() {
        let mut cfg = cfg_block();
        cfg.skip_extensions = vec![".rs.tpl".into()];
        let d = evaluate(&cfg, "lib.rs.tpl", "pub fn ( {", None, false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_skip_paths_match() {
        let mut cfg = cfg_block();
        cfg.skip_paths = vec!["target/**".into(), "**/*_pb.rs".into()];
        assert_eq!(
            evaluate(&cfg, "target/debug/build.rs", "fn (", None, false),
            Decision::Allow
        );
        assert_eq!(
            evaluate(&cfg, "src/proto/user_pb.rs", "fn (", None, false),
            Decision::Allow
        );
    }

    #[test]
    fn evaluate_size_cap_skips_parse() {
        let mut cfg = cfg_block();
        cfg.max_file_bytes = 4;
        let d = evaluate(&cfg, "a.rs", "pub fn ( {", None, false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_warn_emits_diagnostic_on_broken_rust() {
        let d = evaluate(&cfg_warn(), "a.rs", "pub fn ( {", None, false);
        match d {
            Decision::Warn(note) => {
                assert!(note.contains("tree-sitter"));
                assert!(note.contains("a.rs"));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_warn_allows_clean_rust() {
        let d = evaluate(&cfg_warn(), "a.rs", "pub fn foo() {}\n", None, false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_block_rejects_broken_rust_file_write() {
        let d = evaluate(&cfg_block(), "a.rs", "pub fn ( {", None, false);
        match d {
            Decision::Block(msg) => assert!(msg.contains("edit_linter blocked")),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_block_allows_when_pre_has_same_errors() {
        // Pre + new both broken with the same shape — refactor-step exception.
        let broken = "pub fn ( {";
        let d = evaluate(&cfg_block(), "a.rs", broken, Some(broken), false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_block_allows_when_new_has_fewer_errors() {
        let pre = "fn (\nfn (\nfn (\n"; // many errors
        let new = "fn foo() {}\nfn bar() {}\n"; // clean
        let d = evaluate(&cfg_block(), "a.rs", new, Some(pre), false);
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn evaluate_block_rejects_when_new_has_more_errors() {
        let pre = "fn foo() {}\n";
        let new = "fn foo() { \nfn bar(\n";
        match evaluate(&cfg_block(), "a.rs", new, Some(pre), false) {
            Decision::Block(_) => {}
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn path_matches_doublestar_globs() {
        let globs = vec!["**/*.rs".into()];
        assert!(path_matches_any("src/foo.rs", &globs));
        assert!(path_matches_any("src/sub/foo.rs", &globs));
        let prefix = vec!["target/**".into()];
        assert!(path_matches_any("target/debug/build.rs", &prefix));
        assert!(!path_matches_any("src/target.rs", &prefix));
    }

    #[test]
    fn path_matches_any_normalizes_backslashes() {
        let globs = vec!["target/**".into()];
        assert!(path_matches_any("target\\debug\\foo.rs", &globs));
    }
}
