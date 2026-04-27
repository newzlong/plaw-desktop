//! M2 integration test — load the template suite from disk and round-trip
//! a synthetic run through SQLite.

use std::collections::HashMap;
use std::path::PathBuf;

use plaw_eval::storage::{CaseResult, EvalRepo, MetricScore, Run};
use plaw_eval::suite::{load_suite, CaseInput};

fn template_path() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/plaw-eval; ../../ is the repo root.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("..");
    p.push("..");
    p.push("evals");
    p.push("_template");
    p.push("cases.toml");
    p
}

#[test]
fn template_suite_loads_and_round_trips_through_storage() {
    // 1. Load the template TOML from disk.
    let suite = load_suite(template_path()).expect("template should parse");
    assert_eq!(suite.name, "template");
    assert_eq!(suite.version, "1.0.0");
    assert!(suite.cases.len() >= 3);

    // Sanity-check that each `kind` parsed correctly.
    let mut saw_chat = false;
    let mut saw_agent = false;
    let mut saw_rag = false;
    for c in &suite.cases {
        match &c.input {
            CaseInput::Chat { .. } => saw_chat = true,
            CaseInput::Agent { .. } => saw_agent = true,
            CaseInput::Rag { .. } => saw_rag = true,
        }
    }
    assert!(saw_chat && saw_agent && saw_rag);

    // 2. Open an in-memory repo, insert a run + case results.
    let repo = EvalRepo::open_in_memory().unwrap();
    let run = Run {
        id: "integration-run".into(),
        suite_name: suite.name.clone(),
        suite_version: suite.version.clone(),
        started_at: 1000,
        finished_at: None,
        plaw_commit: "deadbeef".into(),
        model_version: "kimi-k2.5".into(),
        config_hash: "hash".into(),
        n_total: suite.cases.len(),
        n_completed: 0,
        n_failed: 0,
    };
    repo.insert_run(&run).unwrap();

    for case in &suite.cases {
        let mut scores = HashMap::new();
        scores.insert(
            "g_eval".into(),
            MetricScore {
                value: 0.75,
                raw: serde_json::json!({"score": 4, "confidence": 0.8}),
                judge_model: "kimi-k2.5".into(),
            },
        );
        repo.insert_case_result(&CaseResult {
            run_id: run.id.clone(),
            case_id: case.id.clone(),
            case_cluster: case.cluster_id.clone(),
            plaw_response: "synthetic response".into(),
            plaw_trace_id: None,
            metric_scores: scores,
            latency_ms: 500,
            tokens_in: 100,
            tokens_out: 50,
            cache_read_tokens: 0,
            error: None,
        })
        .unwrap();
    }

    repo.update_run_finished(&run.id, 2000, suite.cases.len(), 0)
        .unwrap();

    // 3. Read everything back and assert it matches.
    let stored_run = repo.load_run(&run.id).unwrap().expect("run should exist");
    assert_eq!(stored_run.finished_at, Some(2000));
    assert_eq!(stored_run.n_completed, suite.cases.len());

    let stored_results = repo.load_case_results(&run.id).unwrap();
    assert_eq!(stored_results.len(), suite.cases.len());
    assert!(stored_results
        .iter()
        .all(|r| r.metric_scores.get("g_eval").map(|s| s.value).unwrap_or(0.0) == 0.75));

    // 4. Quick summary should average to the constant we wrote.
    let agg = repo.quick_summary(&run.id).unwrap();
    let g = agg.metrics.get("g_eval").expect("g_eval aggregate present");
    assert_eq!(g.n, suite.cases.len());
    assert!((g.mean - 0.75).abs() < 1e-12);

    // 5. Baseline lookup should return this run.
    let baseline = repo.get_baseline(&suite.name).unwrap().unwrap();
    assert_eq!(baseline.id, run.id);
}
