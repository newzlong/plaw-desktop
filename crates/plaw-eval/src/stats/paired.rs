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
        (self.ci.lower > 0.0 && self.ci.upper > 0.0) || (self.ci.lower < 0.0 && self.ci.upper < 0.0)
    }
}

/// Compute mean(a - b), SE, and CI for paired samples.
///
/// `samples_a` and `samples_b` must have the same length, and entry `i` of
/// each must correspond to the same eval case. Returns `None` when the
/// inputs are mismatched, empty, or smaller than 2.
pub fn paired_difference(samples_a: &[f64], samples_b: &[f64], alpha: f64) -> Option<PairedResult> {
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
    let var = diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0);
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
        assert!(paired_difference(&[], &[], 0.05).is_none());
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

    // ── Property-based tests (proptest) ───────────────────────────────────
    //
    // The hand-tests above pin specific numerical examples (textbook
    // paired-t, constant improvement, identity-no-diff). proptest scales
    // those checks to thousands of arbitrary sample pairs and exercises
    // four invariants that any correct paired-difference implementation
    // must satisfy:
    //
    //   1. anti-symmetry: paired(a, b).mean_diff == -paired(b, a).mean_diff
    //   2. identity: paired(a, a) has mean_diff == 0 and is never significant
    //   3. translation invariance: adding constant c to both samples doesn't
    //      change the statistic
    //   4. CI containment: ci.lower <= mean_diff <= ci.upper, ci.lower <=
    //      ci.upper (basic CI well-formedness)
    //
    // If any of these breaks, a downstream eval reading the paired
    // result silently mis-ranks two configurations — the whole point of
    // paired analysis is that the difference is detectable, so a broken
    // detector serves the wrong call/no-call decision under noise.

    use proptest::prelude::*;

    /// Bound generated values away from extreme magnitudes so floating-
    /// point arithmetic stays stable; eval scores are normally in [0, 1]
    /// or low integer counts anyway.
    fn finite_score() -> impl Strategy<Value = f64> {
        -1000.0_f64..=1000.0_f64
    }

    proptest! {
        /// Anti-symmetry: swapping a and b negates mean_diff but keeps |SE|
        /// and CI width identical (subject to floating-point rounding).
        #[test]
        fn paired_difference_is_antisymmetric_in_inputs(
            samples in prop::collection::vec((finite_score(), finite_score()), 2..50),
        ) {
            let a: Vec<f64> = samples.iter().map(|(x, _)| *x).collect();
            let b: Vec<f64> = samples.iter().map(|(_, y)| *y).collect();
            let r_ab = paired_difference(&a, &b, 0.05).unwrap();
            let r_ba = paired_difference(&b, &a, 0.05).unwrap();
            // mean_diff flips sign.
            prop_assert!(
                (r_ab.mean_diff + r_ba.mean_diff).abs() < 1e-9,
                "mean_diff(a,b)={} should be -mean_diff(b,a)={}",
                r_ab.mean_diff, r_ba.mean_diff
            );
            // SE is non-negative and equal.
            prop_assert!((r_ab.se - r_ba.se).abs() < 1e-9);
            // CI width is invariant; bounds flip sign.
            let width_ab = r_ab.ci.upper - r_ab.ci.lower;
            let width_ba = r_ba.ci.upper - r_ba.ci.lower;
            prop_assert!((width_ab - width_ba).abs() < 1e-9);
        }

        /// Identity: paired(a, a) for any a has zero mean_diff and is
        /// never significant. Catches a regression where the "no real
        /// difference" case starts spuriously firing a positive call.
        #[test]
        fn paired_difference_of_identity_is_zero_and_insignificant(
            a in prop::collection::vec(finite_score(), 2..50),
        ) {
            let r = paired_difference(&a, &a, 0.05).unwrap();
            prop_assert_eq!(r.mean_diff, 0.0);
            prop_assert_eq!(r.se, 0.0);
            prop_assert!(!r.is_significant(),
                "paired(a, a) must never be significant; CI was {:?}", r.ci);
        }

        /// Translation invariance: adding the same constant to every
        /// sample of a AND b doesn't change the statistic. This is the
        /// algebraic statement that "raising the baseline scale equally
        /// on both arms shouldn't change whether they differ".
        #[test]
        fn paired_difference_is_translation_invariant(
            samples in prop::collection::vec((finite_score(), finite_score()), 2..50),
            shift in -100.0_f64..=100.0_f64,
        ) {
            let a: Vec<f64> = samples.iter().map(|(x, _)| *x).collect();
            let b: Vec<f64> = samples.iter().map(|(_, y)| *y).collect();
            let a_shifted: Vec<f64> = a.iter().map(|x| x + shift).collect();
            let b_shifted: Vec<f64> = b.iter().map(|y| y + shift).collect();

            let r_orig = paired_difference(&a, &b, 0.05).unwrap();
            let r_shift = paired_difference(&a_shifted, &b_shifted, 0.05).unwrap();

            prop_assert!(
                (r_orig.mean_diff - r_shift.mean_diff).abs() < 1e-6,
                "translation should preserve mean_diff: orig={}, shifted={}",
                r_orig.mean_diff, r_shift.mean_diff
            );
            prop_assert!((r_orig.se - r_shift.se).abs() < 1e-6);
        }

        /// CI well-formedness: lower <= mean_diff <= upper, lower <= upper.
        /// A CI that doesn't contain its own point estimate would be a
        /// catastrophic statistical bug — the "confidence" semantic
        /// requires the estimate to be inside.
        #[test]
        fn paired_difference_ci_contains_mean_diff(
            samples in prop::collection::vec((finite_score(), finite_score()), 2..50),
        ) {
            let a: Vec<f64> = samples.iter().map(|(x, _)| *x).collect();
            let b: Vec<f64> = samples.iter().map(|(_, y)| *y).collect();
            let r = paired_difference(&a, &b, 0.05).unwrap();
            // CI is well-ordered.
            prop_assert!(
                r.ci.lower <= r.ci.upper,
                "CI lower ({}) must be <= upper ({})", r.ci.lower, r.ci.upper
            );
            // mean_diff is inside the CI (with a tiny tolerance for the
            // degenerate SE=0 case where CI collapses to the point).
            prop_assert!(
                r.ci.lower - 1e-9 <= r.mean_diff && r.mean_diff <= r.ci.upper + 1e-9,
                "mean_diff ({}) must be in CI [{}, {}]",
                r.mean_diff, r.ci.lower, r.ci.upper
            );
        }
    }
}
