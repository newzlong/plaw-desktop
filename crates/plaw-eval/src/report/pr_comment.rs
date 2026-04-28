//! PR-comment renderer. Produces a self-contained Markdown body suitable
//! for posting via `gh pr comment` or marocchino/sticky-pull-request-comment.
//!
//! Layout:
//! 1. Headline — overall gate verdict.
//! 2. Run metadata (IDs, case counts, ε, α).
//! 3. Per-metric table (reuses [`super::markdown::render_comparison`]).
//! 4. Optional `<details>` block with the failing cases — case ID, baseline
//!    score, candidate score, delta, judge model. Collapsed by default to
//!    keep PR pages skimmable.

use std::fmt::Write;

use crate::report::gate::{ComparisonReport, MetricVerdict};
use crate::report::markdown::render_comparison;
use crate::storage::CaseResult;

/// Build the comment body. `failing_case_details` is a slice of
/// `(metric, case_id, baseline_score, candidate_score, judge)` tuples the
/// caller has already filtered for relevance — the renderer doesn't
/// re-aggregate.
pub fn render(
    report: &ComparisonReport,
    failing_case_details: &[FailingCaseRow],
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "<!-- plaw-eval-comment -->");
    let _ = writeln!(out, "{}", render_comparison(report));

    let n_failed = report
        .metrics
        .iter()
        .filter(|m| m.verdict == MetricVerdict::Fail)
        .count();
    if n_failed > 0 && !failing_case_details.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "<details><summary>Failing cases ({} rows)</summary>",
            failing_case_details.len()
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "| Metric | Case | Baseline | Candidate | Δ | Judge |"
        );
        let _ = writeln!(out, "|---|---|---:|---:|---:|---|");
        for r in failing_case_details {
            let _ = writeln!(
                out,
                "| `{}` | `{}` | {:.4} | {:.4} | {:+.4} | `{}` |",
                r.metric, r.case_id, r.baseline_value, r.candidate_value, r.delta(), r.judge,
            );
        }
        let _ = writeln!(out, "</details>");
    }
    out
}

/// One row in the "failing cases" detail block.
#[derive(Debug, Clone)]
pub struct FailingCaseRow {
    pub metric: String,
    pub case_id: String,
    pub baseline_value: f64,
    pub candidate_value: f64,
    pub judge: String,
}

impl FailingCaseRow {
    pub fn delta(&self) -> f64 {
        self.candidate_value - self.baseline_value
    }
}

/// Helper: extract the worst-regressing rows for each failing metric. The
/// caller passes the case-result slices used to compute the comparison;
/// we surface up to `per_metric` rows per metric, sorted by largest drop.
pub fn extract_failing_rows(
    report: &ComparisonReport,
    baseline_cases: &[CaseResult],
    candidate_cases: &[CaseResult],
    per_metric: usize,
) -> Vec<FailingCaseRow> {
    let mut out = Vec::new();
    let baseline_by_id: std::collections::HashMap<&str, &CaseResult> = baseline_cases
        .iter()
        .map(|c| (c.case_id.as_str(), c))
        .collect();
    let candidate_by_id: std::collections::HashMap<&str, &CaseResult> = candidate_cases
        .iter()
        .map(|c| (c.case_id.as_str(), c))
        .collect();

    for m in &report.metrics {
        if m.verdict != MetricVerdict::Fail {
            continue;
        }
        let mut rows: Vec<FailingCaseRow> = candidate_by_id
            .iter()
            .filter_map(|(id, cand)| {
                let baseline = baseline_by_id.get(id)?;
                let cand_score = cand.metric_scores.get(&m.metric)?;
                let base_score = baseline.metric_scores.get(&m.metric)?;
                Some(FailingCaseRow {
                    metric: m.metric.clone(),
                    case_id: (*id).into(),
                    baseline_value: base_score.value,
                    candidate_value: cand_score.value,
                    judge: cand_score.judge_model.clone(),
                })
            })
            .filter(|r| r.delta() < 0.0)
            .collect();
        rows.sort_by(|a, b| {
            a.delta()
                .partial_cmp(&b.delta())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(per_metric);
        out.extend(rows);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::gate::compare_in_memory;
    use crate::storage::MetricScore;
    use std::collections::HashMap;

    fn case(id: &str, score: f64) -> CaseResult {
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
        }
    }

    #[test]
    fn passing_run_omits_details_block() {
        let baseline: Vec<_> = (0..10).map(|i| case(&format!("c{i}"), 0.8)).collect();
        let candidate = baseline.clone();
        let report = compare_in_memory("b", &baseline, "c", &candidate, 0.01, 0.05);
        let body = render(&report, &[]);
        assert!(body.contains("✅ PASS"));
        assert!(!body.contains("<details>"));
    }

    #[test]
    fn failing_run_includes_collapsible_details() {
        let baseline: Vec<_> = (0..30).map(|i| case(&format!("c{i}"), 0.8)).collect();
        let candidate: Vec<_> = (0..30).map(|i| case(&format!("c{i}"), 0.4)).collect();
        let report = compare_in_memory("b", &baseline, "c", &candidate, 0.01, 0.05);
        let rows = extract_failing_rows(&report, &baseline, &candidate, 5);
        let body = render(&report, &rows);
        assert!(body.contains("❌ FAIL"));
        assert!(body.contains("<details>"));
        assert!(body.contains("Failing cases"));
        // Top regressors should appear in the table — every case dropped 0.4.
        assert!(body.contains("-0.4000"));
    }

    #[test]
    fn extract_failing_rows_caps_per_metric() {
        let baseline: Vec<_> = (0..50).map(|i| case(&format!("c{i}"), 0.8)).collect();
        let candidate: Vec<_> = (0..50).map(|i| case(&format!("c{i}"), 0.4)).collect();
        let report = compare_in_memory("b", &baseline, "c", &candidate, 0.01, 0.05);
        let rows = extract_failing_rows(&report, &baseline, &candidate, 5);
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|r| r.delta() < 0.0));
    }
}
