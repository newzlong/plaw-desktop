//! Power analysis — sample-size requirements for detecting an effect.
//!
//! Implements the standard two-sided one-sample / paired-sample formula:
//!
//! ```text
//! n = ((z_{α/2} + z_{β}) * σ / δ)^2
//! ```
//!
//! where δ is the effect to detect, σ the standard deviation, α the
//! significance level, and 1 − β the desired power.

use statrs::distribution::{ContinuousCDF, Normal};

/// Computes the minimum sample size required to detect `effect` with the
/// given `sigma`, significance level `alpha`, and `power`.
///
/// Inputs: `effect` and `sigma` are on the same scale (both percentage
/// points, both probabilities, etc.). Effects of zero return `None`.
///
/// Output is rounded **up** to the next integer — the smallest n for which
/// the test attains at least the requested power.
pub fn required_sample_size(effect: f64, sigma: f64, alpha: f64, power: f64) -> Option<usize> {
    if !(effect.is_finite() && sigma.is_finite())
        || effect == 0.0
        || sigma <= 0.0
        || !(0.0 < alpha && alpha < 1.0)
        || !(0.0 < power && power < 1.0)
    {
        return None;
    }
    let z_alpha = standard_normal_inverse(1.0 - alpha / 2.0)?;
    let z_beta = standard_normal_inverse(power)?;
    let n = ((z_alpha + z_beta) * sigma / effect.abs()).powi(2);
    Some(n.ceil() as usize)
}

/// Inverse CDF of the standard normal.
fn standard_normal_inverse(p: f64) -> Option<f64> {
    if !(0.0 < p && p < 1.0) {
        return None;
    }
    Some(Normal::new(0.0, 1.0).ok()?.inverse_cdf(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_canonical_example() {
        // From design.md §三 NF: detect 2pp at α=0.05, β=0.2, σ=0.4.
        // z_{.975} ≈ 1.96, z_{.80} ≈ 0.842.
        // n = ((1.96 + 0.842) * 0.4 / 0.02)^2 ≈ 3138 → spec says ~250 paired
        // (variance shrinks for paired). Here we report the unpaired figure,
        // which is the conservative single-sample bound.
        let n = required_sample_size(0.02, 0.4, 0.05, 0.80).unwrap();
        assert!((3100..=3200).contains(&n), "got {n}");
    }

    #[test]
    fn paired_design_with_smaller_sigma_needs_far_fewer() {
        // Paired difference σ ≈ 0.1 (matches Anthropic's paired example).
        let n = required_sample_size(0.02, 0.1, 0.05, 0.80).unwrap();
        // ((1.96+0.842)*0.1/0.02)^2 ≈ 196.
        assert!((190..=210).contains(&n), "got {n}");
    }

    #[test]
    fn rejects_invalid_inputs() {
        assert!(required_sample_size(0.0, 0.4, 0.05, 0.80).is_none());
        assert!(required_sample_size(0.02, 0.0, 0.05, 0.80).is_none());
        assert!(required_sample_size(0.02, 0.4, 0.0, 0.80).is_none());
        assert!(required_sample_size(0.02, 0.4, 0.05, 1.0).is_none());
        assert!(required_sample_size(f64::NAN, 0.4, 0.05, 0.8).is_none());
    }

    #[test]
    fn larger_effect_requires_fewer_samples() {
        let n_small = required_sample_size(0.01, 0.4, 0.05, 0.80).unwrap();
        let n_big = required_sample_size(0.05, 0.4, 0.05, 0.80).unwrap();
        assert!(n_big < n_small);
    }
}
