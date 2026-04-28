//! M6 integration test — push synthetic case results through the full
//! aggregation + comparison + Markdown pipeline.
//!
//! We don't drive plaw here (M3 already covers that). The point of this
//! test is to verify that `runner::aggregate` + `report::gate` +
//! `report::markdown` glue together correctly when the data is already
//! in SQLite.

use std::collections::HashMap;

use plaw_eval::report::{
    extract_failing_rows, render_aggregate_json, render_aggregate_md, render_comparison_md,
    render_pr_comment, ComparisonReport, GateVerdict, MetricVerdict,
};
use plaw_eval::runner::{aggregate, DEFAULT_AGGREGATE_ALPHA};
use plaw_eval::storage::{CaseResult, EvalRepo, MetricScore, Run};

fn run_skeleton(id: &str, suite: &str, n: usize) -> Run {
    Run {
        id: id.into(),
        suite_name: suite.into(),
        suite_version: "1.0.0".into(),
        started_at: 0,
        finished_at: Some(1),
        plaw_commit: "deadbeef".into(),
        model_version: "kimi-k2.5".into(),
        config_hash: "h".into(),
        n_total: n,
        n_completed: n,
        n_failed: 0,
    }
}

fn case_result(run_id: &str, case_id: &str, cluster: Option<&str>, score: f64) -> CaseResult {
    let mut scores = HashMap::new();
    scores.insert(
        "g_eval".into(),
        MetricScore {
            value: score,
            raw: serde_json::Value::Null,
            judge_model: "mock".into(),
        },
    );
    CaseResult {
        run_id: run_id.into(),
        case_id: case_id.into(),
        case_cluster: cluster.map(|s| s.into()),
        plaw_response: format!("response for {case_id}"),
        plaw_trace_id: None,
        metric_scores: scores,
        latency_ms: 100,
        tokens_in: 50,
        tokens_out: 10,
        cache_read_tokens: 0,
        error: None,
    }
}

#[test]
fn aggregate_then_gate_pipeline_pass() {
    let repo = EvalRepo::open_in_memory().unwrap();

    // Baseline + candidate runs with the same case ids and equivalent
    // scores. Should pass the gate cleanly.
    let baseline = run_skeleton("base-1", "smoke", 30);
    let candidate = run_skeleton("cand-1", "smoke", 30);
    repo.insert_run(&baseline).unwrap();
    repo.insert_run(&candidate).unwrap();
    for i in 0..30 {
        let cluster = format!("topic-{}", i % 3); // 3 clusters of 10
        repo.insert_case_result(&case_result(
            &baseline.id,
            &format!("c{i}"),
            Some(&cluster),
            0.80,
        ))
        .unwrap();
        repo.insert_case_result(&case_result(
            &candidate.id,
            &format!("c{i}"),
            Some(&cluster),
            0.81, // tiny improvement, well within ε
        ))
        .unwrap();
    }

    // Aggregate each run individually and verify the cluster pathway fired.
    let cand_agg = aggregate(&repo, &candidate.id, DEFAULT_AGGREGATE_ALPHA).unwrap();
    let m = cand_agg.metrics.get("g_eval").unwrap();
    assert_eq!(m.n, 30);
    assert_eq!(m.n_clusters, Some(3));
    assert!(m.stderr_clustered.is_some());

    // JSON / Markdown rendering produces non-empty output.
    let json = render_aggregate_json(&cand_agg).unwrap();
    assert!(json.contains("\"g_eval\""));
    let md = render_aggregate_md(&cand_agg);
    assert!(md.contains("g_eval"));

    // Gate comparison should pass.
    let report = plaw_eval::report::compare_runs_default(&repo, &baseline.id, &candidate.id)
        .unwrap();
    assert_eq!(report.verdict, GateVerdict::Pass);
    assert_eq!(report.paired_case_count, 30);
    let g = report.metrics.iter().find(|m| m.metric == "g_eval").unwrap();
    assert_eq!(g.verdict, MetricVerdict::Pass);
    assert!(g.paired_diff.is_some());
    let paired = g.paired_diff.as_ref().unwrap();
    // candidate − baseline = 0.81 − 0.80 = 0.01
    assert!((paired.mean_diff - 0.01).abs() < 1e-9);

    // PR comment of a passing run shouldn't include a details block.
    let comment = render_pr_comment(&report, &[]);
    assert!(comment.contains("✅ PASS"));
    assert!(!comment.contains("<details>"));
}

#[test]
fn aggregate_then_gate_pipeline_fail_with_pr_comment_details() {
    let repo = EvalRepo::open_in_memory().unwrap();
    let baseline = run_skeleton("base-2", "smoke", 30);
    let candidate = run_skeleton("cand-2", "smoke", 30);
    repo.insert_run(&baseline).unwrap();
    repo.insert_run(&candidate).unwrap();

    let mut baseline_cases = Vec::new();
    let mut candidate_cases = Vec::new();
    for i in 0..30 {
        let id = format!("c{i}");
        let b = case_result(&baseline.id, &id, None, 0.80);
        let c = case_result(&candidate.id, &id, None, 0.40); // big regression
        repo.insert_case_result(&b).unwrap();
        repo.insert_case_result(&c).unwrap();
        baseline_cases.push(b);
        candidate_cases.push(c);
    }

    let report = plaw_eval::report::compare_runs_default(&repo, &baseline.id, &candidate.id)
        .unwrap();
    assert_eq!(report.verdict, GateVerdict::Fail);
    let g = &report.metrics[0];
    assert_eq!(g.verdict, MetricVerdict::Fail);

    let md = render_comparison_md(&report);
    assert!(md.contains("❌ FAIL"));

    let rows = extract_failing_rows(&report, &baseline_cases, &candidate_cases, 5);
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|r| r.delta() < 0.0));

    let comment = render_pr_comment(&report, &rows);
    assert!(comment.contains("❌ FAIL"));
    assert!(comment.contains("<details>"));
    assert!(comment.contains("Failing cases"));
    assert!(comment.contains("-0.4000"));
}

#[test]
fn rendering_handles_metric_present_only_in_one_run() {
    // Baseline has g_eval, candidate has tool_selection_f1 — paired
    // analysis isn't possible per metric, gate is Inconclusive.
    let repo = EvalRepo::open_in_memory().unwrap();
    let baseline = run_skeleton("base-3", "smoke", 5);
    let candidate = run_skeleton("cand-3", "smoke", 5);
    repo.insert_run(&baseline).unwrap();
    repo.insert_run(&candidate).unwrap();

    for i in 0..5 {
        let mut b_scores = HashMap::new();
        b_scores.insert(
            "g_eval".into(),
            MetricScore {
                value: 0.8,
                raw: serde_json::Value::Null,
                judge_model: "mock".into(),
            },
        );
        repo.insert_case_result(&CaseResult {
            run_id: baseline.id.clone(),
            case_id: format!("c{i}"),
            case_cluster: None,
            plaw_response: "x".into(),
            plaw_trace_id: None,
            metric_scores: b_scores,
            latency_ms: 0,
            tokens_in: 0,
            tokens_out: 0,
            cache_read_tokens: 0,
            error: None,
        })
        .unwrap();

        let mut c_scores = HashMap::new();
        c_scores.insert(
            "tool_selection_f1".into(),
            MetricScore {
                value: 0.6,
                raw: serde_json::Value::Null,
                judge_model: "mock".into(),
            },
        );
        repo.insert_case_result(&CaseResult {
            run_id: candidate.id.clone(),
            case_id: format!("c{i}"),
            case_cluster: None,
            plaw_response: "x".into(),
            plaw_trace_id: None,
            metric_scores: c_scores,
            latency_ms: 0,
            tokens_in: 0,
            tokens_out: 0,
            cache_read_tokens: 0,
            error: None,
        })
        .unwrap();
    }

    let report: ComparisonReport =
        plaw_eval::report::compare_runs_default(&repo, &baseline.id, &candidate.id).unwrap();
    assert_eq!(report.verdict, GateVerdict::Inconclusive);
    for m in &report.metrics {
        assert_eq!(m.verdict, MetricVerdict::Inconclusive);
    }
    let md = render_comparison_md(&report);
    assert!(md.contains("⚠️ INCONCLUSIVE"));
}
