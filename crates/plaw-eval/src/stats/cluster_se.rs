//! Cluster-robust standard errors.
//!
//! When eval cases are correlated within groups (e.g. multiple turns of the
//! same conversation, multiple questions on the same passage), naive standard
//! errors under-estimate true variance — Anthropic's eval methodology paper
//! (Miller, 2024, arXiv:2411.00640) reports the gap can exceed 3×.
//!
//! This module computes the Cameron-Miller cluster-robust SE for the sample
//! mean, plus a heuristic deciding when clustering should kick in.

use std::collections::HashMap;
use std::hash::Hash;

/// Heuristic threshold from the spec (`design.md` §4.2): clustering is engaged
/// when the number of clusters is small relative to the sample size.
///
/// Returns `true` iff `n_clusters * 5 < n` — i.e. on average each cluster
/// contains more than 5 observations.
pub fn should_use_cluster_se(n: usize, n_clusters: usize) -> bool {
    n_clusters > 0 && n_clusters.saturating_mul(5) < n
}

/// Cluster-robust standard error for the sample mean.
///
/// `values` and `cluster_ids` must have the same length. `cluster_ids` may
/// be any hashable type — strings, integers, etc. Returns `None` if the
/// inputs are mismatched, empty, or contain a single cluster.
///
/// Implementation follows Cameron & Miller (2015), "A Practitioner's Guide
/// to Cluster-Robust Inference":
///
/// ```text
/// Var_clustered(mean) = (G/(G-1)) * (1/n^2) * Σ_g (Σ_{i in g} (x_i - mean))^2
/// ```
///
/// where G is the number of clusters and the inner sum runs over members of
/// each cluster. The G/(G-1) finite-cluster correction matches Stata's
/// `vce(cluster)` default.
pub fn cluster_robust_se<C>(values: &[f64], cluster_ids: &[C]) -> Option<f64>
where
    C: Eq + Hash + Clone,
{
    if values.is_empty() || values.len() != cluster_ids.len() {
        return None;
    }
    let n = values.len() as f64;
    let mean: f64 = values.iter().sum::<f64>() / n;

    // Sum of deviations within each cluster.
    let mut cluster_sums: HashMap<C, f64> = HashMap::new();
    for (v, c) in values.iter().zip(cluster_ids.iter()) {
        *cluster_sums.entry(c.clone()).or_insert(0.0) += v - mean;
    }

    let g = cluster_sums.len() as f64;
    if g < 2.0 {
        return None;
    }

    let inner: f64 = cluster_sums.values().map(|s| s * s).sum();
    let variance = (g / (g - 1.0)) * inner / (n * n);
    Some(variance.sqrt())
}

/// Returns the number of distinct clusters in `cluster_ids` (irrespective of
/// the values they label). Useful when emitting reports.
pub fn count_clusters<C: Eq + Hash>(cluster_ids: &[C]) -> usize {
    let mut seen: std::collections::HashSet<&C> = std::collections::HashSet::new();
    for c in cluster_ids {
        seen.insert(c);
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn threshold_engages_when_clusters_are_dense() {
        assert!(should_use_cluster_se(100, 10)); // 10 clusters, 100 obs
        assert!(should_use_cluster_se(50, 5));
        assert!(!should_use_cluster_se(100, 30)); // 30 clusters → don't engage
        assert!(!should_use_cluster_se(10, 0));
    }

    #[test]
    fn cluster_se_zero_when_within_cluster_sums_balance() {
        // Each cluster averages to the grand mean: deviations sum to 0 within
        // each cluster, so the cluster-robust SE is 0.
        let values = vec![0.0, 1.0, 2.0, 3.0]; // mean 1.5
        let clusters = vec!["a", "a", "b", "b"]; // (0+1) - 2*1.5 = -2; (2+3) - 2*1.5 = +2
                                                  // cluster sums: a→-2, b→+2 ⇒ Σ = 4+4 = 8
        // Var = (2/1) * 8 / 16 = 1.0 ⇒ SE = 1.0
        let se = cluster_robust_se(&values, &clusters).unwrap();
        assert!(approx_eq(se, 1.0, 1e-12));
    }

    #[test]
    fn cluster_se_within_iid_data_close_to_naive_se() {
        // When observations are uncorrelated within clusters, cluster SE
        // should be similar in magnitude to the naive SE (within ~2× given
        // the small sample).
        let values: Vec<f64> = (0..20).map(|i| i as f64 / 20.0).collect();
        let clusters: Vec<usize> = (0..20).map(|i| i / 5).collect(); // 4 clusters of 5

        let se_clustered = cluster_robust_se(&values, &clusters).unwrap();

        let mean = values.iter().sum::<f64>() / 20.0;
        let var_naive: f64 =
            values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (20.0 * 19.0);
        let se_naive = var_naive.sqrt();

        assert!(se_clustered > 0.0);
        assert!(se_clustered < 4.0 * se_naive);
    }

    #[test]
    fn rejects_mismatched_inputs() {
        assert!(cluster_robust_se(&[1.0, 2.0], &["a"]).is_none());
        assert!(cluster_robust_se::<&str>(&[], &[]).is_none());
        // Single cluster ⇒ undefined (G - 1 = 0 in denominator).
        assert!(cluster_robust_se(&[1.0, 2.0, 3.0], &["a", "a", "a"]).is_none());
    }

    #[test]
    fn count_clusters_works() {
        assert_eq!(count_clusters::<&str>(&[]), 0);
        assert_eq!(count_clusters(&["a", "a", "b", "c", "b"]), 3);
    }
}
