//! Run-to-run comparison and gate logic.
//!
//! `compare_runs` figures out whether case IDs match across baseline and
//! candidate (paired analysis) or not (independent diff). Either way it
//! emits per-metric statistics + a [`GateVerdict`].
//!
//! Gate rule (matches `requirements.md` FR-5.4 / `vision.md` §three):
//!
//! ```text
//! Fail if  lower_CI_bound(candidate) < mean(baseline) - epsilon
//! ```

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::runner::aggregate::{aggregate, DEFAULT_AGGREGATE_ALPHA};
use crate::stats::paired_difference;
use crate::storage::{CaseResult, EvalRepo, MetricAggregate};

/// Default epsilon — 1 percentage-point tolerance on regressions.
pub const DEFAULT_EPSILON: f64 = 0.01;

/// Verdict for a single metric.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricVerdict {
    /// Candidate ≥ baseline−ε.
    Pass,
    /// Candidate's lower CI dipped below baseline−ε.
    Fail,
    /// Not enough data to judge (e.g. metric only present in one run).
    Inconclusive,
}

/// Top-level gate verdict — Pass iff every metric passes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    Pass,
    Fail,
    Inconclusive,
}

/// Per-metric comparison row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricComparison {
    pub metric: String,
    pub baseline: Option<MetricAggregate>,
    pub candidate: Option<MetricAggregate>,
    /// Paired mean(candidate − baseline) with CI, when case IDs align.
    pub paired_diff: Option<PairedDiffSummary>,
    pub verdict: MetricVerdict,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDiffSummary {
    pub mean_diff: f64,
    pub se: f64,
    pub ci_lower: f64,
    pub ci_upper: f64,
    pub n: usize,
}

/// Full comparison report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub baseline_run_id: String,
    pub candidate_run_id: String,
    pub epsilon: f64,
    pub alpha: f64,
    pub metrics: Vec<MetricComparison>,
    pub verdict: GateVerdict,
    /// Number of cases shared between the two runs (for paired analysis).
    pub paired_case_count: usize,
    pub baseline_case_count: usize,
    pub candidate_case_count: usize,
}

/// Compare two stored runs.
pub fn compare_runs(
    repo: &EvalRepo,
    baseline_run_id: &str,
    candidate_run_id: &str,
    epsilon: f64,
    alpha: f64,
) -> Result<ComparisonReport> {
    let baseline_cases = repo.load_case_results(baseline_run_id)?;
    let candidate_cases = repo.load_case_results(candidate_run_id)?;
    Ok(compare_in_memory(
        baseline_run_id,
        &baseline_cases,
        candidate_run_id,
        &candidate_cases,
        epsilon,
        alpha,
    ))
}

/// Pure-function variant. Useful when one side comes from a JSON report
/// loaded off disk (e.g. CI runs the candidate, fetches the baseline).
pub fn compare_in_memory(
    baseline_run_id: &str,
    baseline_cases: &[CaseResult],
    candidate_run_id: &str,
    candidate_cases: &[CaseResult],
    epsilon: f64,
    alpha: f64,
) -> ComparisonReport {
    let baseline =
        crate::runner::aggregate::aggregate_in_memory(baseline_run_id, baseline_cases, alpha);
    let candidate =
        crate::runner::aggregate::aggregate_in_memory(candidate_run_id, candidate_cases, alpha);

    // Index successful cases by id, per side, for paired analysis.
    let baseline_by_id: HashMap<&str, &CaseResult> = baseline_cases
        .iter()
        .filter(|c| c.error.is_none())
        .map(|c| (c.case_id.as_str(), c))
        .collect();
    let candidate_by_id: HashMap<&str, &CaseResult> = candidate_cases
        .iter()
        .filter(|c| c.error.is_none())
        .map(|c| (c.case_id.as_str(), c))
        .collect();
    let shared_ids: Vec<&str> = baseline_by_id
        .keys()
        .filter(|id| candidate_by_id.contains_key(*id))
        .copied()
        .collect();

    let mut metric_names: std::collections::BTreeSet<String> =
        baseline.metrics.keys().cloned().collect();
    metric_names.extend(candidate.metrics.keys().cloned());

    let mut comparisons = Vec::with_capacity(metric_names.len());
    let mut overall = GateVerdict::Pass;

    for name in metric_names {
        let baseline_m = baseline.metrics.get(&name).cloned();
        let candidate_m = candidate.metrics.get(&name).cloned();

        let paired = compute_paired(&shared_ids, &baseline_by_id, &candidate_by_id, &name, alpha);
        let (verdict, reason) = decide(&baseline_m, &candidate_m, epsilon);
        if matches!(verdict, MetricVerdict::Fail) {
            overall = GateVerdict::Fail;
        } else if matches!(verdict, MetricVerdict::Inconclusive)
            && matches!(overall, GateVerdict::Pass)
        {
            overall = GateVerdict::Inconclusive;
        }

        comparisons.push(MetricComparison {
            metric: name,
            baseline: baseline_m,
            candidate: candidate_m,
            paired_diff: paired,
            verdict,
            reason,
        });
    }

    ComparisonReport {
        baseline_run_id: baseline_run_id.to_string(),
        candidate_run_id: candidate_run_id.to_string(),
        epsilon,
        alpha,
        metrics: comparisons,
        verdict: overall,
        paired_case_count: shared_ids.len(),
        baseline_case_count: baseline_by_id.len(),
        candidate_case_count: candidate_by_id.len(),
    }
}

fn compute_paired(
    shared_ids: &[&str],
    baseline_by_id: &HashMap<&str, &CaseResult>,
    candidate_by_id: &HashMap<&str, &CaseResult>,
    metric: &str,
    alpha: f64,
) -> Option<PairedDiffSummary> {
    let mut baseline_values = Vec::new();
    let mut candidate_values = Vec::new();
    for id in shared_ids {
        let b = baseline_by_id.get(id)?;
        let c = candidate_by_id.get(id)?;
        let bv = b.metric_scores.get(metric)?.value;
        let cv = c.metric_scores.get(metric)?.value;
        baseline_values.push(bv);
        candidate_values.push(cv);
    }
    if baseline_values.len() < 2 {
        return None;
    }
    let result = paired_difference(&candidate_values, &baseline_values, alpha)?;
    Some(PairedDiffSummary {
        mean_diff: result.mean_diff,
        se: result.se,
        ci_lower: result.ci.lower,
        ci_upper: result.ci.upper,
        n: result.n,
    })
}

fn decide(
    baseline: &Option<MetricAggregate>,
    candidate: &Option<MetricAggregate>,
    epsilon: f64,
) -> (MetricVerdict, String) {
    let (b, c) = match (baseline, candidate) {
        (Some(b), Some(c)) => (b, c),
        _ => {
            return (
                MetricVerdict::Inconclusive,
                "metric missing from one side of the comparison".into(),
            )
        }
    };
    let threshold = b.mean - epsilon;
    if c.ci_lower >= threshold {
        (
            MetricVerdict::Pass,
            format!(
                "candidate lower CI {:.4} ≥ baseline mean {:.4} − ε {:.4}",
                c.ci_lower, b.mean, epsilon
            ),
        )
    } else {
        (
            MetricVerdict::Fail,
            format!(
                "candidate lower CI {:.4} < baseline mean {:.4} − ε {:.4}",
                c.ci_lower, b.mean, epsilon
            ),
        )
    }
}

/// Convenience wrapper using default epsilon / alpha.
pub fn compare_runs_default(
    repo: &EvalRepo,
    baseline_run_id: &str,
    candidate_run_id: &str,
) -> Result<ComparisonReport> {
    compare_runs(
        repo,
        baseline_run_id,
        candidate_run_id,
        DEFAULT_EPSILON,
        DEFAULT_AGGREGATE_ALPHA,
    )
}

/// Re-aggregate then compare against a fresh baseline lookup. Used by the
/// CLI's `compare` subcommand.
pub fn aggregate_and_compare(
    repo: &EvalRepo,
    baseline_run_id: &str,
    candidate_run_id: &str,
    epsilon: f64,
    alpha: f64,
) -> Result<ComparisonReport> {
    // Force fresh aggregations (in case they weren't computed yet).
    let _ = aggregate(repo, baseline_run_id, alpha)?;
    let _ = aggregate(repo, candidate_run_id, alpha)?;
    compare_runs(repo, baseline_run_id, candidate_run_id, epsilon, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MetricScore;

    fn case(id: &str, metric: &str, value: f64) -> CaseResult {
        let mut scores = HashMap::new();
        scores.insert(
            metric.into(),
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
        }
    }

    #[test]
    fn pass_when_candidate_matches_baseline() {
        let baseline: Vec<_> = (0..30)
            .map(|i| case(&format!("c{i}"), "g_eval", 0.8))
            .collect();
        let candidate = baseline.clone();
        let report = compare_in_memory("base", &baseline, "cand", &candidate, 0.01, 0.05);
        assert_eq!(report.verdict, GateVerdict::Pass);
        assert_eq!(report.paired_case_count, 30);
        let m = report
            .metrics
            .iter()
            .find(|m| m.metric == "g_eval")
            .unwrap();
        assert_eq!(m.verdict, MetricVerdict::Pass);
        assert!(m.paired_diff.is_some());
    }

    #[test]
    fn fail_when_candidate_regresses_below_threshold() {
        let baseline: Vec<_> = (0..30)
            .map(|i| case(&format!("c{i}"), "g_eval", 0.8))
            .collect();
        // Drop every case to 0.4 — way more than ε=0.01 below baseline.
        let candidate: Vec<_> = (0..30)
            .map(|i| case(&format!("c{i}"), "g_eval", 0.4))
            .collect();
        let report = compare_in_memory("base", &baseline, "cand", &candidate, 0.01, 0.05);
        assert_eq!(report.verdict, GateVerdict::Fail);
        let m = &report.metrics[0];
        assert_eq!(m.verdict, MetricVerdict::Fail);
        let pd = m.paired_diff.as_ref().unwrap();
        // Paired diff = candidate − baseline = 0.4 − 0.8 = −0.4
        assert!((pd.mean_diff + 0.4).abs() < 1e-12);
    }

    #[test]
    fn small_dip_within_epsilon_still_passes() {
        // baseline mean 0.80; candidate mean 0.795 → 0.005 below, within ε=0.01.
        let baseline: Vec<_> = (0..50)
            .map(|i| case(&format!("c{i}"), "g_eval", 0.80))
            .collect();
        let candidate: Vec<_> = (0..50)
            .map(|i| case(&format!("c{i}"), "g_eval", 0.795))
            .collect();
        let report = compare_in_memory("base", &baseline, "cand", &candidate, 0.01, 0.05);
        // With zero variance the candidate's lower CI equals its mean.
        // 0.795 ≥ 0.80 − 0.01 = 0.79 → Pass.
        assert_eq!(report.verdict, GateVerdict::Pass);
    }

    #[test]
    fn metric_missing_on_one_side_is_inconclusive() {
        let baseline: Vec<_> = (0..3)
            .map(|i| case(&format!("c{i}"), "g_eval", 0.8))
            .collect();
        let candidate: Vec<_> = (0..3)
            .map(|i| case(&format!("c{i}"), "tool_selection_f1", 0.6))
            .collect();
        let report = compare_in_memory("base", &baseline, "cand", &candidate, 0.01, 0.05);
        assert_eq!(report.verdict, GateVerdict::Inconclusive);
        for m in &report.metrics {
            assert_eq!(m.verdict, MetricVerdict::Inconclusive);
        }
    }

    #[test]
    fn paired_diff_dropped_when_no_shared_ids() {
        let baseline: Vec<_> = (0..30)
            .map(|i| case(&format!("a{i}"), "g_eval", 0.8))
            .collect();
        let candidate: Vec<_> = (0..30)
            .map(|i| case(&format!("b{i}"), "g_eval", 0.8))
            .collect();
        let report = compare_in_memory("base", &baseline, "cand", &candidate, 0.01, 0.05);
        assert_eq!(report.paired_case_count, 0);
        let m = &report.metrics[0];
        assert!(m.paired_diff.is_none());
        assert_eq!(m.verdict, MetricVerdict::Pass); // independent diff still works
    }
}
