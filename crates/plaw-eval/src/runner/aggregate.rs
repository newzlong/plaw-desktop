//! Statistical aggregation of stored case results into [`AggregateReport`].
//!
//! For each metric present in `case_results`, we compute:
//! - sample mean
//! - standard error of the mean (`σ / sqrt(n)`)
//! - cluster-robust SE when [`should_use_cluster_se`] fires (n_clusters
//!   small relative to n)
//! - 95% confidence interval based on whichever SE we used
//!
//! Implementations live here, in `runner/`, so callers can run an
//! aggregation directly off a finished run without going through the CLI.

use std::collections::HashMap;

use anyhow::Result;

use crate::stats::{
    cluster_robust_se, count_clusters, should_use_cluster_se, t_distribution_ci, ConfidenceInterval,
};
use crate::storage::{AggregateReport, CaseResult, EvalRepo, MetricAggregate};

/// 95% CI by default, matching the `00-vision.md` north-star metric format.
pub const DEFAULT_AGGREGATE_ALPHA: f64 = 0.05;

/// Aggregate every metric in the run's stored case results. Cases marked
/// as failed (with an `error`) are excluded — they shouldn't pull the
/// quality average down to zero on infra failures.
pub fn aggregate(repo: &EvalRepo, run_id: &str, alpha: f64) -> Result<AggregateReport> {
    let cases = repo.load_case_results(run_id)?;
    Ok(aggregate_in_memory(run_id, &cases, alpha))
}

/// Pure-function variant — useful for tests and for re-aggregating data
/// already in memory (e.g. reading a JSON report from disk).
pub fn aggregate_in_memory(run_id: &str, cases: &[CaseResult], alpha: f64) -> AggregateReport {
    let mut by_metric: HashMap<String, Vec<MetricObservation>> = HashMap::new();
    for c in cases {
        if c.error.is_some() {
            // Skip failed cases — they shouldn't artificially deflate the
            // mean. (M5 will revisit whether failures should count as 0
            // for some metrics.)
            continue;
        }
        for (name, score) in &c.metric_scores {
            by_metric
                .entry(name.clone())
                .or_default()
                .push(MetricObservation {
                    value: score.value,
                    cluster: c.case_cluster.clone(),
                });
        }
    }

    let mut metrics = HashMap::new();
    for (name, obs) in by_metric {
        if let Some(agg) = aggregate_one(&obs, alpha) {
            metrics.insert(name, agg);
        }
    }

    AggregateReport {
        run_id: run_id.to_string(),
        metrics,
        suite_name: None,
    }
}

#[derive(Debug, Clone)]
struct MetricObservation {
    value: f64,
    cluster: Option<String>,
}

fn aggregate_one(obs: &[MetricObservation], alpha: f64) -> Option<MetricAggregate> {
    if obs.is_empty() {
        return None;
    }
    let n = obs.len();
    let values: Vec<f64> = obs.iter().map(|o| o.value).collect();
    let mean = values.iter().sum::<f64>() / n as f64;

    // Naive standard error of the mean (sample SD / sqrt(n)).
    let stderr = if n > 1 {
        let var = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0);
        (var / n as f64).sqrt()
    } else {
        0.0
    };

    // Cluster-robust SE when the cluster_id labels look meaningful.
    let cluster_labels: Vec<String> = obs.iter().filter_map(|o| o.cluster.clone()).collect();
    let (stderr_clustered, n_clusters) = if cluster_labels.len() == n {
        let n_clusters = count_clusters(&cluster_labels);
        if should_use_cluster_se(n, n_clusters) {
            let se = cluster_robust_se(&values, &cluster_labels);
            (se, Some(n_clusters))
        } else {
            (None, Some(n_clusters))
        }
    } else {
        (None, None)
    };

    let effective_se = stderr_clustered.unwrap_or(stderr);
    let ci = if n >= 2 && effective_se.is_finite() {
        t_distribution_ci(mean, effective_se, n, alpha).unwrap_or(ConfidenceInterval {
            lower: mean,
            upper: mean,
            level: 1.0 - alpha,
        })
    } else {
        ConfidenceInterval {
            lower: mean,
            upper: mean,
            level: 1.0 - alpha,
        }
    };

    Some(MetricAggregate {
        mean,
        stderr,
        stderr_clustered,
        ci_lower: ci.lower,
        ci_upper: ci.upper,
        n,
        n_clusters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MetricScore;

    fn case(id: &str, cluster: Option<&str>, metric: &str, value: f64) -> CaseResult {
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
            case_cluster: cluster.map(|s| s.into()),
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
    fn computes_mean_and_t_ci() {
        let cases: Vec<_> = (0..30)
            .map(|i| case(&format!("c{i}"), None, "g_eval", (i as f64) / 30.0))
            .collect();
        let agg = aggregate_in_memory("r", &cases, 0.05);
        let m = agg.metrics.get("g_eval").unwrap();
        assert_eq!(m.n, 30);
        assert!((m.mean - 14.5 / 30.0).abs() < 1e-12);
        assert!(m.ci_lower < m.mean);
        assert!(m.ci_upper > m.mean);
        assert!(m.stderr_clustered.is_none()); // no cluster_ids supplied
    }

    #[test]
    fn cluster_se_kicks_in_when_threshold_met() {
        // 30 cases in 3 clusters → n_clusters * 5 < n? 3*5=15 < 30 ✔
        let cases: Vec<_> = (0..30)
            .map(|i| {
                let cluster = format!("topic-{}", i / 10);
                case(
                    &format!("c{i}"),
                    Some(&cluster),
                    "g_eval",
                    (i as f64) / 30.0,
                )
            })
            .collect();
        let agg = aggregate_in_memory("r", &cases, 0.05);
        let m = agg.metrics.get("g_eval").unwrap();
        assert_eq!(m.n_clusters, Some(3));
        assert!(m.stderr_clustered.is_some());
    }

    #[test]
    fn cluster_se_skipped_when_too_many_clusters() {
        // 30 cases, 30 clusters → n_clusters * 5 = 150 > 30 ✘ — skip cluster SE.
        let cases: Vec<_> = (0..30)
            .map(|i| {
                let cluster = format!("c-{i}");
                case(&format!("c{i}"), Some(&cluster), "g_eval", 0.5)
            })
            .collect();
        let agg = aggregate_in_memory("r", &cases, 0.05);
        let m = agg.metrics.get("g_eval").unwrap();
        assert_eq!(m.n_clusters, Some(30));
        assert!(m.stderr_clustered.is_none());
    }

    #[test]
    fn failed_cases_are_excluded() {
        let mut cases: Vec<_> = (0..5)
            .map(|i| case(&format!("c{i}"), None, "g_eval", 1.0))
            .collect();
        cases[0].error = Some("boom".into());
        let agg = aggregate_in_memory("r", &cases, 0.05);
        let m = agg.metrics.get("g_eval").unwrap();
        assert_eq!(m.n, 4);
    }

    #[test]
    fn handles_metric_with_missing_observations_gracefully() {
        let mut cases = vec![case("c1", None, "g_eval", 0.8)];
        cases.push(case("c2", None, "tool_selection_f1", 0.5));
        let agg = aggregate_in_memory("r", &cases, 0.05);
        assert_eq!(agg.metrics.len(), 2);
        for m in agg.metrics.values() {
            assert_eq!(m.n, 1);
            assert_eq!(m.ci_lower, m.mean); // n<2 fallback
            assert_eq!(m.ci_upper, m.mean);
        }
    }
}
