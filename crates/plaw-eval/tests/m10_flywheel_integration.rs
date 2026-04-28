//! M10 integration test — end-to-end flywheel pipeline:
//!   run results in DB → sample → review approve → promote → target
//!   suite contains the new case.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use plaw_eval::flywheel::{
    promote, read_promoted_case, review, sample_run, ReviewVerdict, SampleStrategy,
};
use plaw_eval::storage::{CaseResult, EvalRepo, MetricScore, Run};
use plaw_eval::suite::load_suite;

fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("plaw-eval-m10-{}-{}", std::process::id(), n));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn write_minimal_suite(dir: &PathBuf, original_case_id: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("cases.toml");
    let toml = format!(
        r#"
name = "chat_quality"
version = "1.0.0"
description = "M10 integration"

[default_judge]
model = "kimi-k2.5"
provider = "kimi"
mode = {{ kind = "pairwise", dual_pass = true }}

[[cases]]
id = "{original_case_id}"
[cases.input]
kind = "chat"
messages = [{{ role = "user", content = "What is 2 + 2?" }}]
[cases.expected]
answer_keywords = ["4"]
"#
    );
    fs::write(&path, toml).unwrap();
    path
}

fn seed_run(repo: &EvalRepo, run_id: &str, case_id: &str, score: f64) {
    repo.insert_run(&Run {
        id: run_id.into(),
        suite_name: "chat_quality".into(),
        suite_version: "1.0.0".into(),
        started_at: 0,
        finished_at: Some(1),
        plaw_commit: "abc".into(),
        model_version: "kimi-k2.5".into(),
        config_hash: "h".into(),
        n_total: 1,
        n_completed: 1,
        n_failed: 0,
    })
    .unwrap();
    let mut metric_scores = HashMap::new();
    metric_scores.insert(
        "g_eval".into(),
        MetricScore {
            value: score,
            raw: serde_json::Value::Null,
            judge_model: "mock".into(),
        },
    );
    repo.insert_case_result(&CaseResult {
        run_id: run_id.into(),
        case_id: case_id.into(),
        case_cluster: None,
        plaw_response: format!("Eight is the answer (low-quality response, score {score})"),
        plaw_trace_id: None,
        metric_scores,
        latency_ms: 0,
        tokens_in: 0,
        tokens_out: 0,
        cache_read_tokens: 0,
        error: None,
        tool_calls: Vec::new(),
    })
    .unwrap();
}

#[test]
fn end_to_end_flywheel_pipeline() {
    let tmp = tempdir();
    let suite_path = write_minimal_suite(&tmp.join("chat_quality"), "math-2plus2");

    // 1. Run produces a low-score result.
    let repo = EvalRepo::open_in_memory().unwrap();
    seed_run(&repo, "run-1", "math-2plus2", 0.3);

    // 2. Sampler picks low-score cases.
    let summary = sample_run(
        &repo,
        "run-1",
        SampleStrategy::LowScore {
            metric: "g_eval".into(),
            threshold: 0.5,
        },
        Some("chat_quality"),
    )
    .unwrap();
    assert_eq!(summary.queued, 1);

    // 3. Reviewer approves the queued entry.
    let pending = repo.flywheel_list_pending(10).unwrap();
    assert_eq!(pending.len(), 1);
    let queue_id = pending[0].id.clone();
    review(&repo, &queue_id, ReviewVerdict::Approve).unwrap();

    // 4. Promoter writes the new case into the target suite.
    let result = promote(&repo, &queue_id, &suite_path).unwrap();
    assert_eq!(result.new_case_id, "math-2plus2-flywheel-001");

    // 5. The target suite now has the new case with flywheel metadata.
    let suite = load_suite(&suite_path).unwrap();
    assert_eq!(suite.cases.len(), 2);
    let new_case = read_promoted_case(&suite_path, &result.new_case_id)
        .unwrap()
        .unwrap();
    assert_eq!(new_case.source, "flywheel");
    assert!(new_case.tags.contains(&"flywheel".into()));
    assert!(new_case.promoted_at.is_some());

    // 6. Queue entry is stamped as promoted.
    let entry = repo.flywheel_get(&queue_id).unwrap().unwrap();
    assert_eq!(entry.review_status, "promoted");
    assert_eq!(entry.promoted_to_suite.as_deref(), Some("chat_quality"));
    assert_eq!(
        entry.promoted_case_id.as_deref(),
        Some("math-2plus2-flywheel-001")
    );
}

#[test]
fn rejected_entries_are_not_promotable() {
    let tmp = tempdir();
    let suite_path = write_minimal_suite(&tmp.join("c"), "case-1");
    let repo = EvalRepo::open_in_memory().unwrap();
    seed_run(&repo, "run-1", "case-1", 0.2);

    let summary = sample_run(
        &repo,
        "run-1",
        SampleStrategy::LowScore {
            metric: "g_eval".into(),
            threshold: 0.5,
        },
        Some("c"),
    )
    .unwrap();
    assert_eq!(summary.queued, 1);
    let queue_id = repo.flywheel_list_pending(10).unwrap()[0].id.clone();
    review(&repo, &queue_id, ReviewVerdict::Reject).unwrap();

    let err = promote(&repo, &queue_id, &suite_path).unwrap_err();
    assert!(format!("{err:#}").contains("approved"));

    // Suite untouched.
    let suite = load_suite(&suite_path).unwrap();
    assert_eq!(suite.cases.len(), 1);
}

#[test]
fn migration_upgrades_old_dbs() {
    // Build a DB the old way (without the new columns) and confirm
    // that opening it via EvalRepo::open replays the runtime migration
    // and brings the new columns online.
    use rusqlite::Connection;

    let path = tempdir().join("legacy.db");
    {
        // Manual create with the OLD schema (no source_run_id etc.)
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE flywheel_queue (
                id TEXT PRIMARY KEY,
                trace_id TEXT NOT NULL,
                sampled_at INTEGER NOT NULL,
                judge_score REAL,
                review_status TEXT NOT NULL,
                reviewed_at INTEGER,
                promoted_to_suite TEXT,
                promoted_case_id TEXT
            );
            "#,
        )
        .unwrap();
        // Insert a row that wouldn't survive a strict-column migration
        conn.execute(
            "INSERT INTO flywheel_queue VALUES ('legacy-1','t','0',NULL,'pending',NULL,NULL,NULL)",
            [],
        )
        .unwrap();
    }

    // Re-open via EvalRepo — triggers apply_runtime_migrations.
    let repo = EvalRepo::open(&path).unwrap();
    let entry = repo.flywheel_get("legacy-1").unwrap().unwrap();
    assert_eq!(entry.id, "legacy-1");
    assert!(entry.source_run_id.is_none());
    assert!(entry.target_suite.is_none());
}
