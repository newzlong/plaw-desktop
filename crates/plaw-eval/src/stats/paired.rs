//! Paired-difference analysis for A/B comparisons.
//!
//! When the same eval cases are run against two configurations (baseline vs
//! candidate), the paired analysis treats each case-level difference as a
//! single observation. Variance shrinks because the same case's noise
//! correlates across A and B — Anthropic's eval methodology paper reports
//! this can require 4–10× fewer samples than two independent runs.

use crate::stats::ci::{t_distribution_ci, ConfidenceInterval};

/// Result of a paired-difference comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairedResult {
    /// Mean of (a - b) over paired observations.
    pub mean_diff: f64,
    /// Standard error of the mean difference.
    pub se: f64,
    /// 95% (or `level`) CI on the mean difference.
    pub ci: ConfidenceInterval,
    /// Number of pairs analysed.
    pub n: usize,
}

impl PairedResult {
    /// `true` iff the CI excludes zero — i.e. the difference is significant
    /// at the given level.
    pub fn is_significant(&self) -> bool {
        (self.ci.lower > 0.0 && self.ci.upper > 0.0)
            || (self.ci.lower < 0.0 && self.ci.upper < 0.0)
    }
}

/// Compute mean(a - b), SE, and CI for paired samples.
///
/// `samples_a` and `samples_b` must have the same length, and entry `i` of
/// each must correspond to the same eval case. Returns `None` when the
/// inputs are mismatched, empty, or smaller than 2.
pub fn paired_difference(
    samples_a: &[f64],
    samples_b: &[f64],
    alpha: f64,
) -> Option<PairedResult> {
    if samples_a.len() != samples_b.len() || samples_a.len() < 2 {
        return None;
    }
    let diffs: Vec<f64> = samples_a
        .iter()
        .zip(samples_b.iter())
        .map(|(a, b)| a - b)
        .collect();
    let n = diffs.len();
    let mean = diffs.iter().sum::<f64>() / n as f64;
    let var = diffs
        .iter()
        .map(|d| (d - mean).powi(2))
        .sum::<f64>()
        / (n as f64 - 1.0);
    let se = (var / n as f64).sqrt();
    let ci = t_distribution_ci(mean, se, n, alpha)?;
    Some(PairedResult {
        mean_diff: mean,
        se,
        ci,
        n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn detects_constant_improvement() {
        // a = b + 0.1 for every case ⇒ mean_diff = 0.1, var = 0, SE = 0,
        // CI is degenerate. Reject the degenerate case explicitly.
        let a: Vec<f64> = (0..30).map(|i| 0.5 + i as f64 * 0.01).collect();
        let b: Vec<f64> = a.iter().map(|x| x - 0.1).collect();
        let r = paired_difference(&a, &b, 0.05).unwrap();
        assert!(approx_eq(r.mean_diff, 0.1, 1e-12));
        assert!(approx_eq(r.se, 0.0, 1e-12));
        assert_eq!(r.n, 30);
    }

    #[test]
    fn matches_known_paired_t_example() {
        // Classic textbook paired example.
        // Pairs: (10, 8), (12, 9), (9, 7), (11, 10), (10, 8) — diffs 2,3,2,1,2.
        // mean diff = 2.0, var = 0.5, n = 5, SE = sqrt(0.5/5) ≈ 0.31623
        // 95% CI ≈ (1.122, 2.878).
        let a = vec![10.0, 12.0, 9.0, 11.0, 10.0];
        let b = vec![8.0, 9.0, 7.0, 10.0, 8.0];
        let r = paired_difference(&a, &b, 0.05).unwrap();
        assert!(approx_eq(r.mean_diff, 2.0, 1e-12));
        assert!(approx_eq(r.se, 0.31623, 1e-4));
        assert!(approx_eq(r.ci.lower, 1.1221, 1e-3));
        assert!(approx_eq(r.ci.upper, 2.8779, 1e-3));
        assert!(r.is_significant());
    }

    #[test]
    fn rejects_mismatched_inputs() {
        assert!(paired_difference(&[1.0, 2.0], &[1.0], 0.05).is_none());
        assert!(paired_difference(&[1.0], &[1.0], 0.05).is_none());
        assert!(paired_difference::<>(&[], &[], 0.05).is_none());
    }

    #[test]
    fn ci_includes_zero_when_no_real_difference() {
        // Two identical sequences with small noise should yield a CI
        // straddling zero.
        let a: Vec<f64> = (0..20).map(|i| 0.5 + (i % 3) as f64 * 0.01).collect();
        let b = a.clone();
        let r = paired_difference(&a, &b, 0.05).unwrap();
        assert_eq!(r.mean_diff, 0.0);
        assert!(!r.is_significant());
    }
}
