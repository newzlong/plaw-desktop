//! CLI entrypoint for `plaw checkpoint ...` subcommands.
//!
//! Reads the on-disk per-iteration agent loop snapshots produced by
//! [`crate::agent::checkpoint::FsCheckpointWriter`] (PR #58) via the
//! reader API ([`crate::agent::checkpoint::FsCheckpointReader`], PR #59)
//! and prints either a human-readable table / detail view or JSON for
//! scripting. Wired from `main.rs`; lib-only build can't see the bin
//! caller, hence the `dead_code` allow.
//!
//! No write or resume capability ships here — this PR is the
//! forensic-only consumer that pairs with PR #58 + PR #59. The resume
//! entry point and Tauri / desktop UI bindings are follow-up PRs.

use crate::agent::checkpoint::{
    resolve_checkpoint_root, FsCheckpointReader, Snapshot, TurnSummary,
};
use crate::config::Config;
use anyhow::{bail, Context, Result};

#[allow(dead_code)]
#[allow(clippy::needless_pass_by_value)]
pub fn handle_command(command: crate::CheckpointCommands, config: &Config) -> Result<()> {
    let root = resolve_checkpoint_root(config);
    let reader = FsCheckpointReader::new(&root);
    match command {
        crate::CheckpointCommands::List { json } => list_turns(&reader, &root, json),
        crate::CheckpointCommands::Show {
            turn_id,
            iteration,
            json,
        } => show_turn(&reader, &root, &turn_id, iteration, json),
    }
}

fn list_turns(reader: &FsCheckpointReader, root: &std::path::Path, json: bool) -> Result<()> {
    let turns = reader
        .list_turns()
        .with_context(|| format!("failed to list checkpoint turns under {}", root.display()))?;

    if json {
        let body = serde_json::to_string_pretty(&turns)?;
        println!("{body}");
        return Ok(());
    }

    if turns.is_empty() {
        println!("No checkpoint snapshots under {}.", root.display());
        println!();
        println!("Checkpointing is disabled by default. To enable, set in config.toml:");
        println!("  [agent.checkpoint]");
        println!("  enabled = true");
        return Ok(());
    }

    println!(
        "📋 Checkpoint turns ({}) under {}:",
        turns.len(),
        root.display()
    );
    println!(
        "  {:<40} {:>10} {:>14}",
        "TURN ID", "ITERATIONS", "LATEST ITER"
    );
    println!("  {:─<40} {:─>10} {:─>14}", "", "", "");
    for t in &turns {
        let latest = t
            .latest_iteration
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string());
        println!(
            "  {:<40} {:>10} {:>14}",
            truncate(&t.turn_id, 40),
            t.snapshot_count,
            latest
        );
    }
    Ok(())
}

fn show_turn(
    reader: &FsCheckpointReader,
    root: &std::path::Path,
    turn_id: &str,
    iteration: Option<usize>,
    json: bool,
) -> Result<()> {
    let snapshots = reader
        .list_for_turn(turn_id)
        .with_context(|| format!("failed to read turn {turn_id} under {}", root.display()))?;

    if snapshots.is_empty() {
        bail!(
            "no snapshots found for turn {turn_id} under {}",
            root.display()
        );
    }

    match iteration {
        None => {
            // No specific iteration: print a one-line index of every iteration.
            if json {
                let summary: Vec<_> = snapshots
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "iteration": s.iteration,
                            "parent_iteration": s.parent_iteration,
                            "history_len": s.history.len(),
                            "created_at": s.created_at,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&summary)?);
                return Ok(());
            }

            println!("📋 Turn {turn_id} — {} snapshot(s):", snapshots.len());
            println!(
                "  {:>5} {:>9} {:>11} {}",
                "ITER", "PARENT", "HIST_LEN", "CREATED_AT"
            );
            println!("  {:─>5} {:─>9} {:─>11} {:─<30}", "", "", "", "");
            for s in &snapshots {
                let parent = s
                    .parent_iteration
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "  {:>5} {:>9} {:>11} {}",
                    s.iteration,
                    parent,
                    s.history.len(),
                    s.created_at
                );
            }
            Ok(())
        }
        Some(target_iter) => {
            // Specific iteration: print the full snapshot detail.
            let snap = snapshots
                .iter()
                .find(|s| s.iteration == target_iter)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "iteration {target_iter} not found for turn {turn_id} \
                         (available: {})",
                        snapshots
                            .iter()
                            .map(|s| s.iteration.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;

            if json {
                println!("{}", serde_json::to_string_pretty(snap)?);
                return Ok(());
            }

            print_snapshot_detail(snap);
            Ok(())
        }
    }
}

fn print_snapshot_detail(snap: &Snapshot) {
    println!("📸 Snapshot");
    println!("  schema_version    : {}", snap.schema_version);
    println!("  turn_id           : {}", snap.turn_id);
    println!("  iteration         : {}", snap.iteration);
    println!(
        "  parent_iteration  : {}",
        snap.parent_iteration
            .map(|n| n.to_string())
            .unwrap_or_else(|| "(none)".into())
    );
    println!("  created_at        : {}", snap.created_at);
    println!("  history (len={}):", snap.history.len());
    for (i, msg) in snap.history.iter().enumerate() {
        // Cap each message preview at 200 chars so even a noisy snapshot
        // stays scannable. Users wanting full bodies should pass --json.
        let body = truncate(&msg.content.replace('\n', "  "), 200);
        println!("    [{i:>3}] {:<10} {body}", msg.role);
    }
}

/// Truncate a string to `max` characters, appending "…" if cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::checkpoint::{CheckpointWriter, FsCheckpointWriter, Snapshot};
    use crate::providers::ChatMessage;
    use tempfile::TempDir;

    fn write_sample_turn(root: &std::path::Path, turn_id: &str, iterations: usize) {
        let writer = FsCheckpointWriter::new(root);
        for i in 0..iterations {
            let history = vec![
                ChatMessage::system("sys"),
                ChatMessage::user(format!("user-{i}")),
                ChatMessage::assistant(format!("asst-{i}")),
            ];
            writer
                .put(&Snapshot::new(turn_id, i, history))
                .expect("writer should succeed in test");
        }
    }

    fn config_for(workspace: &std::path::Path) -> Config {
        let mut cfg = Config::default();
        cfg.workspace_dir = workspace.to_path_buf();
        // Default checkpoint dir is "state/checkpoints" relative to
        // workspace — matches the resolution logic in resolve_checkpoint_root.
        cfg
    }

    // Path-resolution tests live next to the helper in
    // `agent::checkpoint::tests` after PR #61 moved `resolve_checkpoint_root`
    // there (Rule of Three: three call sites = one shared helper).

    #[test]
    fn list_turns_handler_succeeds_on_missing_root() {
        // CLI must not crash when the checkpoint root doesn't exist yet
        // — that's the default state on a fresh install.
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        let result = handle_command(crate::CheckpointCommands::List { json: false }, &cfg);
        assert!(result.is_ok());
    }

    #[test]
    fn list_turns_handler_succeeds_on_empty_root() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        std::fs::create_dir_all(tmp.path().join("state/checkpoints")).unwrap();
        let result = handle_command(crate::CheckpointCommands::List { json: false }, &cfg);
        assert!(result.is_ok());
    }

    #[test]
    fn list_turns_handler_succeeds_on_populated_root() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        write_sample_turn(&tmp.path().join("state/checkpoints"), "turn-a", 2);
        write_sample_turn(&tmp.path().join("state/checkpoints"), "turn-b", 1);

        let result = handle_command(crate::CheckpointCommands::List { json: false }, &cfg);
        assert!(result.is_ok());

        // Also exercise the JSON path.
        let result = handle_command(crate::CheckpointCommands::List { json: true }, &cfg);
        assert!(result.is_ok());
    }

    #[test]
    fn show_handler_lists_iterations_when_iteration_arg_is_none() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        write_sample_turn(&tmp.path().join("state/checkpoints"), "turn-a", 3);

        let result = handle_command(
            crate::CheckpointCommands::Show {
                turn_id: "turn-a".into(),
                iteration: None,
                json: false,
            },
            &cfg,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn show_handler_prints_detail_when_iteration_arg_is_some() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        write_sample_turn(&tmp.path().join("state/checkpoints"), "turn-a", 3);

        let result = handle_command(
            crate::CheckpointCommands::Show {
                turn_id: "turn-a".into(),
                iteration: Some(1),
                json: false,
            },
            &cfg,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn show_handler_emits_json_when_requested() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        write_sample_turn(&tmp.path().join("state/checkpoints"), "turn-a", 2);

        // Index mode (no --iteration) with JSON.
        let result = handle_command(
            crate::CheckpointCommands::Show {
                turn_id: "turn-a".into(),
                iteration: None,
                json: true,
            },
            &cfg,
        );
        assert!(result.is_ok());

        // Detail mode (--iteration) with JSON.
        let result = handle_command(
            crate::CheckpointCommands::Show {
                turn_id: "turn-a".into(),
                iteration: Some(0),
                json: true,
            },
            &cfg,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn show_handler_errors_when_turn_is_missing() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        let result = handle_command(
            crate::CheckpointCommands::Show {
                turn_id: "does-not-exist".into(),
                iteration: None,
                json: false,
            },
            &cfg,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("no snapshots found"),
            "expected 'no snapshots found' in error, got: {msg}"
        );
    }

    #[test]
    fn show_handler_errors_when_iteration_is_missing() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(tmp.path());
        write_sample_turn(&tmp.path().join("state/checkpoints"), "turn-a", 2);

        let result = handle_command(
            crate::CheckpointCommands::Show {
                turn_id: "turn-a".into(),
                iteration: Some(99),
                json: false,
            },
            &cfg,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("iteration 99 not found"));
        assert!(msg.contains("available: 0, 1"));
    }

    #[test]
    fn truncate_keeps_short_strings_intact() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_appends_ellipsis_when_cut() {
        // 6-char input, max=4 → keep 3 chars + "…".
        let out = truncate("abcdef", 4);
        let chars: Vec<char> = out.chars().collect();
        assert_eq!(chars.len(), 4);
        assert_eq!(chars[3], '…');
    }
}
