//! Per-iteration durable snapshots of agent loop state.
//!
//! After every iteration of [`super::loop_::run_tool_call_loop`] completes
//! (LLM response + tool results folded into `history`), the loop emits a
//! [`Snapshot`] via the [`CheckpointWriter`] held in the task-local scope
//! set up by [`super::loop_::run_tool_call_loop_with_non_cli_approval_context`].
//!
//! Default writer: [`FsCheckpointWriter`] persists each snapshot as a
//! standalone JSON file at
//! `<data_dir>/state/checkpoints/<turn_id>/<iteration:06>.json`.
//!
//! This module ships the **writer surface only** as Phase 0. There is no
//! reader API, no resume capability, no fork/branch semantics yet — those
//! are deliberately split into follow-up PRs so this slice stays
//! reviewable. Today's value: post-crash forensics (the on-disk snapshots
//! capture the full conversation history at every iteration boundary)
//! plus a stable foundation for the resume path that lands next.
//!
//! Design DNA borrowed from LangGraph's persistence layer (per the
//! `rig-rs-spike-discovery` workflow, lens C): full state per super-step
//! boundary, parent-pointer chain for replay/fork, simple put/list API.
//! Choices that diverge from LangGraph (justified for plaw's
//! single-binary desktop scope):
//!
//! - Filesystem JSON instead of SQLite — plaw is single-user; rm-and-cat
//!   are the inspection tools; no schema migration risk.
//! - No `writes` table (per-task partial writes within a super-step) —
//!   plaw's iteration boundary is the only checkpoint boundary; mid-tool
//!   recovery is out of scope.
//! - No `checkpoint_ns` — single graph per turn; no subgraph composition.
//! - `iteration` doubles as `checkpoint_id` within a turn — ordering is
//!   trivially derivable from the integer, no ULID dependency needed.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::providers::ChatMessage;

/// Schema version of the on-disk [`Snapshot`] format. Bumped only when an
/// incompatible field change ships; the resume PR that lands later will
/// branch on this when reading older files. Readers should reject snapshots
/// with `schema_version > SNAPSHOT_SCHEMA_VERSION` (forward-incompatible).
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// A single point-in-time capture of agent loop state at an iteration
/// boundary.
///
/// Field selection follows the LOAD-BEARING categorization from the
/// `state-machine-discovery` workflow (lens A). EPHEMERAL state
/// (`seen_tool_signatures`, `tool_call_counts`, per-iteration result
/// buffers) is intentionally omitted — it is reconstructable from
/// `history` on resume. SESSION-scoped values (`cancellation_token`,
/// `on_delta`, approval context) are also omitted — they belong to the
/// runtime, not the conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    /// Schema version. Always equals [`SNAPSHOT_SCHEMA_VERSION`] when
    /// produced by this build.
    pub schema_version: u32,
    /// The agent loop's per-invocation UUID (`turn_id`). Used as the
    /// directory name when persisted to disk and as the "thread" identifier
    /// in LangGraph terms.
    pub turn_id: String,
    /// Zero-based iteration counter of the agent loop. Doubles as the
    /// in-turn checkpoint id; the resume PR will look up snapshots by
    /// `(turn_id, iteration)`.
    pub iteration: usize,
    /// Iteration index of the previous snapshot in the same turn. `None`
    /// for the first iteration (`iteration == 0`). Forms a parent-pointer
    /// chain so the resume / fork PR can walk history and a future
    /// debugger UI can render the iteration as a tree.
    pub parent_iteration: Option<usize>,
    /// Full LOAD-BEARING conversation history at this iteration boundary.
    /// Includes system prompt, user messages, all assistant turns, and all
    /// tool result messages.
    pub history: Vec<ChatMessage>,
    /// RFC3339 UTC timestamp of when this snapshot was taken (i.e. when
    /// the iteration finished). Useful for post-hoc latency analysis and
    /// for ordering snapshots independently of `iteration` when comparing
    /// across forks (a future capability).
    pub created_at: String,
}

impl Snapshot {
    /// Build a snapshot for an iteration just completed. `parent_iteration`
    /// is derived automatically (`iteration - 1` when nonzero, `None`
    /// otherwise).
    pub fn new(turn_id: impl Into<String>, iteration: usize, history: Vec<ChatMessage>) -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            turn_id: turn_id.into(),
            iteration,
            parent_iteration: iteration.checked_sub(1),
            history,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Sink for per-iteration agent loop snapshots.
///
/// Implementations must be `Send + Sync` because the agent loop holds the
/// writer via [`std::sync::Arc`] and may call `put` on any tokio worker.
/// `put` is intentionally sync — filesystem JSON writes complete in <1ms
/// on the target hardware (Tauri desktop, SSD), and threading `async_trait`
/// through the loop's already-busy signature is needless ceremony at this
/// scale. Implementations that genuinely need to do async I/O (e.g. a
/// future SQLite or remote checkpointer) can spawn from inside `put`
/// using `tokio::spawn`.
pub trait CheckpointWriter: Send + Sync {
    /// Persist a snapshot. Errors are surfaced to the caller, but the
    /// agent loop callsite logs and continues (best-effort) — losing one
    /// snapshot is strictly better than failing the whole turn.
    fn put(&self, snapshot: &Snapshot) -> Result<()>;
}

/// Writer that discards every snapshot. Used as the safe default when
/// checkpointing is disabled in config or when no writer has been pushed
/// into the task-local scope (e.g. unit-test invocations of the loop).
pub struct NullCheckpointWriter;

impl CheckpointWriter for NullCheckpointWriter {
    fn put(&self, _snapshot: &Snapshot) -> Result<()> {
        Ok(())
    }
}

/// Filesystem-backed [`CheckpointWriter`]: one JSON file per snapshot.
///
/// Path layout: `<root>/<turn_id>/<iteration:06>.json`. Each turn gets its
/// own subdirectory; iterations sort naturally by filename (six-digit
/// zero-padded). Pretty-printed JSON for `cat`-friendly post-mortem
/// inspection.
///
/// **Atomic write:** snapshots are first written to `<path>.tmp` in the
/// same directory, then renamed. A crash mid-write leaves the previous
/// snapshot intact and at most a stray `.tmp` file on disk. Same-directory
/// rename is atomic on every filesystem plaw targets (NTFS, APFS, ext4).
pub struct FsCheckpointWriter {
    root: PathBuf,
}

impl FsCheckpointWriter {
    /// Build a writer rooted at `root`. The root directory is created on
    /// first `put`, not in this constructor — constructing is cheap and
    /// shouldn't touch the filesystem.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Compute the on-disk path for a given `(turn_id, iteration)` pair
    /// without writing anything. Used in tests and by future reader APIs.
    pub fn path_for(&self, turn_id: &str, iteration: usize) -> PathBuf {
        self.root.join(turn_id).join(format!("{iteration:06}.json"))
    }

    /// Root directory this writer publishes under. Useful for the future
    /// `plaw checkpoint list` CLI / UI.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl CheckpointWriter for FsCheckpointWriter {
    fn put(&self, snapshot: &Snapshot) -> Result<()> {
        let dir = self.root.join(&snapshot.turn_id);
        std::fs::create_dir_all(&dir)?;
        let final_path = self.path_for(&snapshot.turn_id, snapshot.iteration);
        // Write to a temp sibling, then rename — leaves any prior snapshot
        // for this iteration intact if we crash mid-serialize.
        let tmp_path = final_path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(snapshot)?;
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_history() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("you are helpful"),
            ChatMessage::user("ping"),
            ChatMessage::assistant("pong"),
        ]
    }

    #[test]
    fn snapshot_schema_version_is_one() {
        let snap = Snapshot::new("turn-a", 0, sample_history());
        assert_eq!(snap.schema_version, 1);
    }

    #[test]
    fn snapshot_parent_iteration_is_none_for_zero() {
        let snap = Snapshot::new("turn-a", 0, sample_history());
        assert_eq!(snap.parent_iteration, None);
    }

    #[test]
    fn snapshot_parent_iteration_is_predecessor_for_nonzero() {
        let snap = Snapshot::new("turn-a", 3, sample_history());
        assert_eq!(snap.parent_iteration, Some(2));
    }

    #[test]
    fn snapshot_json_roundtrip_preserves_all_fields() {
        let original = Snapshot::new("turn-xyz", 7, sample_history());
        let json = serde_json::to_string(&original).unwrap();
        let parsed: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn null_writer_succeeds_for_any_snapshot() {
        let writer = NullCheckpointWriter;
        let snap = Snapshot::new("turn-a", 0, sample_history());
        assert!(writer.put(&snap).is_ok());
    }

    #[test]
    fn fs_writer_path_for_uses_six_digit_zero_padded_iteration() {
        let writer = FsCheckpointWriter::new("/tmp/checkpoints");
        let path = writer.path_for("turn-abc", 42);
        // Path comparison is platform-aware; compare the trailing component.
        assert_eq!(path.file_name().unwrap().to_string_lossy(), "000042.json");
        // Parent dir is the turn dir.
        assert_eq!(
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "turn-abc"
        );
    }

    #[test]
    fn fs_writer_creates_turn_directory_and_file() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        let snap = Snapshot::new("turn-1", 0, sample_history());
        writer.put(&snap).unwrap();

        let expected = tmp.path().join("turn-1").join("000000.json");
        assert!(expected.exists(), "expected snapshot file at {expected:?}");
    }

    #[test]
    fn fs_writer_writes_pretty_json_with_newlines() {
        // Pretty-printed output is deliberate — these files are meant to be
        // read by humans for forensics. A regression to compact JSON would
        // be a UX regression.
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        let snap = Snapshot::new("turn-1", 0, sample_history());
        writer.put(&snap).unwrap();

        let body = std::fs::read_to_string(writer.path_for("turn-1", 0)).unwrap();
        assert!(body.contains('\n'), "pretty JSON should contain newlines");
        assert!(body.contains("\"history\""));
        assert!(body.contains("\"iteration\""));
    }

    #[test]
    fn fs_writer_roundtrips_via_disk() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        let snap = Snapshot::new("turn-1", 5, sample_history());
        writer.put(&snap).unwrap();

        let body = std::fs::read_to_string(writer.path_for("turn-1", 5)).unwrap();
        let parsed: Snapshot = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, snap);
    }

    #[test]
    fn fs_writer_overwrites_existing_snapshot_for_same_iteration() {
        // Overwrite semantics matter: a retry / replay of the same
        // iteration should end with the LATEST history, not a stale
        // first-write. The atomic rename-into-place pattern handles this.
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());

        let first = Snapshot::new("turn-1", 0, vec![ChatMessage::user("first")]);
        writer.put(&first).unwrap();

        let second = Snapshot::new("turn-1", 0, vec![ChatMessage::user("second")]);
        writer.put(&second).unwrap();

        let body = std::fs::read_to_string(writer.path_for("turn-1", 0)).unwrap();
        let parsed: Snapshot = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.history.len(), 1);
        assert_eq!(parsed.history[0].content, "second");
    }

    #[test]
    fn fs_writer_leaves_no_tmp_file_after_successful_put() {
        // The .tmp sibling should not exist after a successful rename.
        // (A leftover would accumulate and confuse future readers.)
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        let snap = Snapshot::new("turn-1", 0, sample_history());
        writer.put(&snap).unwrap();

        let tmp_sibling = writer.path_for("turn-1", 0).with_extension("json.tmp");
        assert!(
            !tmp_sibling.exists(),
            "tmp sibling should not survive a successful put"
        );
    }

    #[test]
    fn fs_writer_supports_multiple_turns_in_same_root() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        writer
            .put(&Snapshot::new("turn-a", 0, sample_history()))
            .unwrap();
        writer
            .put(&Snapshot::new("turn-b", 0, sample_history()))
            .unwrap();
        writer
            .put(&Snapshot::new("turn-a", 1, sample_history()))
            .unwrap();

        assert!(tmp.path().join("turn-a").join("000000.json").exists());
        assert!(tmp.path().join("turn-a").join("000001.json").exists());
        assert!(tmp.path().join("turn-b").join("000000.json").exists());
    }
}
