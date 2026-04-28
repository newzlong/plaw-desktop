//! Markdown renderers for aggregate and comparison reports.
//!
//! Used directly by `plaw eval run` (writing alongside the JSON) and by
//! `plaw eval compare` for human review.

use std::fmt::Write;

use crate::report::gate::{ComparisonReport, GateVerdict, MetricVerdict};
use crate::storage::AggregateReport;

/// Render a single-run aggregate report as a Markdown table.
pub fn render_aggregate(report: &AggregateReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## plaw-eval aggregate report");
    let _ = writeln!(out);
    let _ = writeln!(out, "Run: `{}`", report.run_id);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| Metric | n | mean | 95% CI | SE | clustered SE | clusters |"
    );
    let _ = writeln!(out, "|---|---:|---:|---|---:|---:|---:|");

    let mut keys: Vec<&String> = report.metrics.keys().collect();
    keys.sort();
    for k in keys {
        let m = &report.metrics[k];
        let _ = writeln!(
            out,
            "| `{}` | {} | {:.4} | [{:.4}, {:.4}] | {:.4} | {} | {} |",
            k,
            m.n,
            m.mean,
            m.ci_lower,
            m.ci_upper,
            m.stderr,
            m.stderr_clustered
                .map(|s| format!("{s:.4}"))
                .unwrap_or_else(|| "—".into()),
            m.n_clusters
                .map(|n| n.to_string())
                .unwrap_or_else(|| "—".into()),
        );
    }
    out
}

/// Render a baseline-vs-candidate comparison.
pub fn render_comparison(report: &ComparisonReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## plaw-eval gate: {}", verdict_badge(report.verdict));
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Baseline: `{}` ({} cases) · Candidate: `{}` ({} cases) · paired: {} · ε = {} · α = {}",
        report.baseline_run_id,
        report.baseline_case_count,
        report.candidate_run_id,
        report.candidate_case_count,
        report.paired_case_count,
        report.epsilon,
        report.alpha,
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| Metric | Verdict | Baseline mean | Candidate mean | Δ | Candidate 95% CI | Notes |"
    );
    let _ = writeln!(out, "|---|:---:|---:|---:|---:|---|---|");
    for m in &report.metrics {
        let baseline_mean = m
            .baseline
            .as_ref()
            .map(|a| format!("{:.4}", a.mean))
            .unwrap_or_else(|| "—".into());
        let candidate_mean = m
            .candidate
            .as_ref()
            .map(|a| format!("{:.4}", a.mean))
            .unwrap_or_else(|| "—".into());
        let delta = match (m.baseline.as_ref(), m.candidate.as_ref()) {
            (Some(b), Some(c)) => format!("{:+.4}", c.mean - b.mean),
            _ => "—".into(),
        };
        let candidate_ci = m
            .candidate
            .as_ref()
            .map(|a| format!("[{:.4}, {:.4}]", a.ci_lower, a.ci_upper))
            .unwrap_or_else(|| "—".into());
        let notes = match &m.paired_diff {
            Some(p) => format!(
                "paired Δ {:+.4} CI [{:.4}, {:.4}] · {}",
                p.mean_diff, p.ci_lower, p.ci_upper, m.reason
            ),
            None => m.reason.clone(),
        };
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} |",
            m.metric,
            metric_badge(m.verdict),
            baseline_mean,
            candidate_mean,
            delta,
            candidate_ci,
            notes,
        );
    }
    out
}

fn verdict_badge(v: GateVerdict) -> &'static str {
    match v {
        GateVerdict::Pass => "✅ PASS",
        GateVerdict::Fail => "❌ FAIL",
        GateVerdict::Inconclusive => "⚠️ INCONCLUSIVE",
    }
}

fn metric_badge(v: MetricVerdict) -> &'static str {
    match v {
        MetricVerdict::Pass => "✅",
        MetricVerdict::Fail => "❌",
        MetricVerdict::Inconclusive => "⚠️",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::gate::{compare_in_memory, MetricVerdict};
    use crate::storage::{CaseResult, MetricScore};
    use std::collections::HashMap;

    fn case(id: &str, value: f64) -> CaseResult {
        let mut scores = HashMap::new();
        scores.insert(
            "g_eval".into(),
            MetricScore {
                value,
                raw: serde_json::Value::Null,
                judge_model: "mock".into(),
            },
        );
        CaseResult {
            run_id: "r".into(),
            case_id: id.into(),
            case_cluster: None,
            plaw_response: String::new(),
            plaw_trace_id: None,
            metric_scores: scores,
            latency_ms: 0,
            tokens_in: 0,
            tokens_out: 0,
            cache_read_tokens: 0,
            error: None,
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn aggregate_table_lists_metrics_alphabetically() {
        use crate::storage::MetricAggregate;
        let mut metrics = HashMap::new();
        metrics.insert(
            "z_metric".into(),
            MetricAggregate {
                mean: 0.5,
                stderr: 0.0,
                stderr_clustered: None,
                ci_lower: 0.5,
                ci_upper: 0.5,
                n: 1,
                n_clusters: None,
            },
        );
        metrics.insert(
            "a_metric".into(),
            MetricAggregate {
                mean: 0.5,
                stderr: 0.0,
                stderr_clustered: None,
                ci_lower: 0.5,
                ci_upper: 0.5,
                n: 1,
                n_clusters: None,
            },
        );
        let report = AggregateReport {
            run_id: "r1".into(),
            metrics,
        };
        let md = render_aggregate(&report);
        let a_pos = md.find("a_metric").unwrap();
        let z_pos = md.find("z_metric").unwrap();
        assert!(a_pos < z_pos);
    }

    #[test]
    fn comparison_marks_pass_when_metrics_match() {
        let baseline: Vec<_> = (0..30).map(|i| case(&format!("c{i}"), 0.8)).collect();
        let candidate = baseline.clone();
        let report = compare_in_memory("base", &baseline, "cand", &candidate, 0.01, 0.05);
        let md = render_comparison(&report);
        assert!(md.contains("✅ PASS"));
        assert!(md.contains("g_eval"));
    }

    #[test]
    fn comparison_marks_fail_when_metric_regresses() {
        let baseline: Vec<_> = (0..30).map(|i| case(&format!("c{i}"), 0.8)).collect();
        let candidate: Vec<_> = (0..30).map(|i| case(&format!("c{i}"), 0.4)).collect();
        let report = compare_in_memory("base", &baseline, "cand", &candidate, 0.01, 0.05);
        let md = render_comparison(&report);
        assert!(md.contains("❌ FAIL"));
        assert!(md.contains("paired Δ"));
    }

    #[test]
    fn metric_badges_are_emoji() {
        assert_eq!(metric_badge(MetricVerdict::Pass), "✅");
        assert_eq!(metric_badge(MetricVerdict::Fail), "❌");
        assert_eq!(metric_badge(MetricVerdict::Inconclusive), "⚠️");
    }
}
