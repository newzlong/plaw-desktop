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
//! Read-side companion: [`FsCheckpointReader`] enumerates turns
//! ([`TurnSummary`]), lists per-turn snapshots sorted by iteration, and
//! loads the latest snapshot for resume-target selection. Reader and
//! writer share [`FsCheckpointWriter::path_for`] / [`FsCheckpointReader::path_for`]
//! so any layout change touches one place.
//!
//! Together these two halves cover Phase 0 (durable persistence,
//! PR #58) and Phase 1 (forensic read). The remaining capabilities —
//! actual agent-loop resume from a snapshot, CLI / desktop UI for
//! browsing checkpoint history, and fork / time-travel semantics —
//! are deliberately split into follow-up PRs so each slice stays
//! reviewable.
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

impl Snapshot {
    /// Read a single snapshot from a JSON file produced by
    /// [`FsCheckpointWriter::put`]. Returns an error if the file is
    /// missing, unreadable, or its `schema_version` is newer than this
    /// build understands (forward-incompatible).
    pub fn load_from(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read snapshot at {}: {e}", path.display()))?;
        let snapshot: Snapshot = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("failed to parse snapshot at {}: {e}", path.display()))?;
        if snapshot.schema_version > SNAPSHOT_SCHEMA_VERSION {
            anyhow::bail!(
                "snapshot at {} has schema_version {} but this build only \
                 understands up to version {SNAPSHOT_SCHEMA_VERSION}",
                path.display(),
                snapshot.schema_version
            );
        }
        Ok(snapshot)
    }
}

/// One-line summary of a turn's persisted snapshots, returned by
/// [`FsCheckpointReader::list_turns`]. Designed for a future
/// `plaw checkpoint list` CLI / desktop forensic UI: enough information
/// to render a row in a table without loading every snapshot body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSummary {
    /// Turn identifier (directory name).
    pub turn_id: String,
    /// Number of snapshot files present for this turn.
    pub snapshot_count: usize,
    /// Highest iteration index seen (i.e. most recent snapshot's
    /// `iteration` field). Useful for resume-target picking. `None`
    /// when no snapshot files are present (empty turn directory) —
    /// should be rare but possible if the writer crashed before any
    /// `put` completed.
    pub latest_iteration: Option<usize>,
}

/// Read-side companion to [`FsCheckpointWriter`]. Enumerates and loads
/// snapshots from the on-disk layout `<root>/<turn_id>/<iter:06>.json`.
///
/// Cheap to construct (no I/O); methods perform the directory walking.
/// Returned snapshots are eager (full JSON parse per file) — fine at
/// plaw's scale (snapshots are ~1 KB) but worth revisiting if turn
/// directories ever grow into the tens of thousands of iterations.
pub struct FsCheckpointReader {
    root: PathBuf,
}

impl FsCheckpointReader {
    /// Build a reader rooted at `root`. The directory is NOT created;
    /// missing roots surface as "no turns" via [`Self::list_turns`].
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Path layout helper — same as [`FsCheckpointWriter::path_for`]
    /// so callers can use either side of the API interchangeably.
    pub fn path_for(&self, turn_id: &str, iteration: usize) -> PathBuf {
        self.root.join(turn_id).join(format!("{iteration:06}.json"))
    }

    /// Root directory this reader observes.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// List every turn directory under the root, with a one-row summary
    /// per turn. Missing root → empty `Vec`. Subdirectories that fail
    /// to enumerate are skipped (logged) rather than aborting — partial
    /// forensic data is better than none.
    ///
    /// Result is sorted by `turn_id` (lexicographic, stable).
    pub fn list_turns(&self) -> Result<Vec<TurnSummary>> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut summaries = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(turn_id) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let snapshots = match self.list_for_turn(turn_id) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        turn_id = %turn_id,
                        error = %e,
                        "skipping unreadable turn directory in list_turns"
                    );
                    continue;
                }
            };
            let snapshot_count = snapshots.len();
            let latest_iteration = snapshots.last().map(|s| s.iteration);
            summaries.push(TurnSummary {
                turn_id: turn_id.to_string(),
                snapshot_count,
                latest_iteration,
            });
        }
        summaries.sort_by(|a, b| a.turn_id.cmp(&b.turn_id));
        Ok(summaries)
    }

    /// Load every snapshot for a turn, sorted by iteration ascending.
    /// Missing turn directory → empty `Vec`. Files whose names don't
    /// match the `<iter:06>.json` pattern (e.g. stray `.tmp` siblings
    /// from a crash mid-write) are skipped. Files that fail JSON
    /// parsing surface an error — partial corruption shouldn't be
    /// silently swallowed at the per-turn level.
    pub fn list_for_turn(&self, turn_id: &str) -> Result<Vec<Snapshot>> {
        let dir = self.root.join(turn_id);
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|s| s.to_str()) == Some("json")
                    && p.file_stem().and_then(|s| s.to_str()).is_some_and(|name| {
                        name.len() == 6 && name.chars().all(|c| c.is_ascii_digit())
                    })
            })
            .collect();
        paths.sort();

        paths.iter().map(|p| Snapshot::load_from(p)).collect()
    }

    /// Load only the highest-iteration snapshot for a turn — the natural
    /// resume point. Returns `Ok(None)` when the turn has no snapshots
    /// (or doesn't exist), matching `list_for_turn` semantics.
    pub fn latest_for_turn(&self, turn_id: &str) -> Result<Option<Snapshot>> {
        Ok(self.list_for_turn(turn_id)?.pop())
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

    // ── Reader API tests ────────────────────────────────────────────

    #[test]
    fn snapshot_load_from_roundtrips_a_file_written_by_writer() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        let original = Snapshot::new("turn-1", 3, sample_history());
        writer.put(&original).unwrap();

        let loaded = Snapshot::load_from(&writer.path_for("turn-1", 3)).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn snapshot_load_from_returns_error_on_missing_file() {
        let tmp = TempDir::new().unwrap();
        let result = Snapshot::load_from(&tmp.path().join("nonexistent.json"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("failed to read snapshot"),
            "expected error to mention 'failed to read snapshot', got: {msg}"
        );
    }

    #[test]
    fn snapshot_load_from_rejects_forward_incompatible_schema_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("000000.json");
        // Hand-craft a snapshot JSON with a schema_version higher than
        // the current build understands; the loader must reject it.
        let bogus = serde_json::json!({
            "schema_version": SNAPSHOT_SCHEMA_VERSION + 1,
            "turn_id": "future",
            "iteration": 0,
            "parent_iteration": null,
            "history": [],
            "created_at": "2099-01-01T00:00:00Z"
        });
        std::fs::write(&path, serde_json::to_vec(&bogus).unwrap()).unwrap();

        let result = Snapshot::load_from(&path);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("schema_version"),
            "error must mention schema_version, got: {msg}"
        );
    }

    #[test]
    fn snapshot_load_from_returns_parse_error_on_corrupt_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("corrupt.json");
        std::fs::write(&path, b"{ this isn't json").unwrap();

        let result = Snapshot::load_from(&path);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("failed to parse snapshot"));
    }

    #[test]
    fn reader_list_turns_returns_empty_for_missing_root() {
        let tmp = TempDir::new().unwrap();
        let reader = FsCheckpointReader::new(tmp.path().join("does-not-exist"));
        let turns = reader.list_turns().unwrap();
        assert!(turns.is_empty());
    }

    #[test]
    fn reader_list_turns_returns_empty_for_empty_root() {
        let tmp = TempDir::new().unwrap();
        let reader = FsCheckpointReader::new(tmp.path());
        let turns = reader.list_turns().unwrap();
        assert!(turns.is_empty());
    }

    #[test]
    fn reader_list_turns_summarizes_each_turn_directory() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());

        // turn-a has 3 iterations.
        for i in 0..3 {
            writer
                .put(&Snapshot::new("turn-a", i, sample_history()))
                .unwrap();
        }
        // turn-b has 1 iteration.
        writer
            .put(&Snapshot::new("turn-b", 0, sample_history()))
            .unwrap();

        let reader = FsCheckpointReader::new(tmp.path());
        let turns = reader.list_turns().unwrap();
        assert_eq!(turns.len(), 2);
        // Sorted lexicographically.
        assert_eq!(turns[0].turn_id, "turn-a");
        assert_eq!(turns[0].snapshot_count, 3);
        assert_eq!(turns[0].latest_iteration, Some(2));
        assert_eq!(turns[1].turn_id, "turn-b");
        assert_eq!(turns[1].snapshot_count, 1);
        assert_eq!(turns[1].latest_iteration, Some(0));
    }

    #[test]
    fn reader_list_turns_ignores_non_directory_entries_in_root() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        writer
            .put(&Snapshot::new("turn-a", 0, sample_history()))
            .unwrap();
        // Stray file at root level (e.g. a lock file or accidental drop).
        std::fs::write(tmp.path().join("README.txt"), b"not a turn").unwrap();

        let reader = FsCheckpointReader::new(tmp.path());
        let turns = reader.list_turns().unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id, "turn-a");
    }

    #[test]
    fn reader_list_for_turn_returns_snapshots_sorted_by_iteration() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        // Write out of order to verify the reader sorts, not the FS.
        for i in [5, 0, 2, 1, 3, 4] {
            writer
                .put(&Snapshot::new("turn-a", i, sample_history()))
                .unwrap();
        }

        let reader = FsCheckpointReader::new(tmp.path());
        let snapshots = reader.list_for_turn("turn-a").unwrap();
        assert_eq!(snapshots.len(), 6);
        for (idx, snap) in snapshots.iter().enumerate() {
            assert_eq!(snap.iteration, idx);
        }
    }

    #[test]
    fn reader_list_for_turn_returns_empty_for_missing_turn() {
        let tmp = TempDir::new().unwrap();
        let reader = FsCheckpointReader::new(tmp.path());
        let snapshots = reader.list_for_turn("nonexistent").unwrap();
        assert!(snapshots.is_empty());
    }

    #[test]
    fn reader_list_for_turn_skips_unrelated_files() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        writer
            .put(&Snapshot::new("turn-a", 0, sample_history()))
            .unwrap();
        // Stray .tmp sibling (simulates a crashed mid-write).
        let turn_dir = tmp.path().join("turn-a");
        std::fs::write(turn_dir.join("000001.json.tmp"), b"partial").unwrap();
        // Stray non-iteration filename.
        std::fs::write(turn_dir.join("notes.txt"), b"hand-edited").unwrap();
        // Stray differently-named JSON (resume-marker etc.).
        std::fs::write(turn_dir.join("resume.json"), b"{}").unwrap();

        let reader = FsCheckpointReader::new(tmp.path());
        let snapshots = reader.list_for_turn("turn-a").unwrap();
        // Only the well-formed 000000.json should appear.
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].iteration, 0);
    }

    #[test]
    fn reader_list_for_turn_surfaces_parse_errors_on_corrupt_files() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        writer
            .put(&Snapshot::new("turn-a", 0, sample_history()))
            .unwrap();
        // Truncate the file in place — convincing-looking 000001.json but
        // not parseable. We expect list_for_turn to surface the error
        // (per-turn level corruption is not silent).
        let path = tmp.path().join("turn-a").join("000001.json");
        std::fs::write(&path, b"{ truncated").unwrap();

        let reader = FsCheckpointReader::new(tmp.path());
        let result = reader.list_for_turn("turn-a");
        assert!(result.is_err());
    }

    #[test]
    fn reader_latest_for_turn_returns_the_highest_iteration() {
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        writer
            .put(&Snapshot::new("turn-a", 0, sample_history()))
            .unwrap();
        writer
            .put(&Snapshot::new("turn-a", 7, sample_history()))
            .unwrap();
        writer
            .put(&Snapshot::new("turn-a", 3, sample_history()))
            .unwrap();

        let reader = FsCheckpointReader::new(tmp.path());
        let latest = reader.latest_for_turn("turn-a").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().iteration, 7);
    }

    #[test]
    fn reader_latest_for_turn_returns_none_for_missing_turn() {
        let tmp = TempDir::new().unwrap();
        let reader = FsCheckpointReader::new(tmp.path());
        let latest = reader.latest_for_turn("nonexistent").unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn reader_and_writer_use_identical_path_layout() {
        // Regression guard: any divergence in path layout would break
        // round-trip and is easy to introduce silently if either side
        // hardcodes the format.
        let tmp = TempDir::new().unwrap();
        let writer = FsCheckpointWriter::new(tmp.path());
        let reader = FsCheckpointReader::new(tmp.path());
        for &(turn, iter) in &[("a", 0_usize), ("b-with-dashes", 42), ("c", 999_999)] {
            assert_eq!(writer.path_for(turn, iter), reader.path_for(turn, iter));
        }
    }

    #[test]
    fn turn_summary_json_serialization_uses_expected_keys() {
        let summary = TurnSummary {
            turn_id: "abc".to_string(),
            snapshot_count: 3,
            latest_iteration: Some(2),
        };
        let v = serde_json::to_value(&summary).unwrap();
        assert_eq!(v["turn_id"], "abc");
        assert_eq!(v["snapshot_count"], 3);
        assert_eq!(v["latest_iteration"], 2);
    }
}
