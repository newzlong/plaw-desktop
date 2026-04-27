//! Confidence intervals for sample statistics.
//!
//! Provides:
//! - [`t_distribution_ci`] — small-sample CI using Student's t.
//! - [`wilson_score_ci`] — robust CI for binary success/failure metrics.
//! - [`bootstrap_ci`] — non-parametric CI via percentile bootstrap.
//!
//! All routines are cross-checked against `scipy.stats` reference values in
//! `tests/stats_correctness.rs`.

use statrs::distribution::{ContinuousCDF, Normal, StudentsT};

/// 95% as the canonical default α (two-sided 0.025 in each tail).
pub const DEFAULT_ALPHA: f64 = 0.05;

/// One-sided lower / upper bounds returned by every CI routine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfidenceInterval {
    pub lower: f64,
    pub upper: f64,
    pub level: f64,
}

impl ConfidenceInterval {
    pub fn width(&self) -> f64 {
        self.upper - self.lower
    }

    /// Confidence level expressed as a probability, e.g. 0.95.
    pub fn confidence(&self) -> f64 {
        self.level
    }
}

/// Two-sided confidence interval based on Student's t-distribution.
///
/// `mean` is the sample mean, `sem` the standard error of the mean
/// (sigma / sqrt(n)), and `n` the sample size. `alpha` is the two-sided
/// significance level — pass [`DEFAULT_ALPHA`] for a 95% CI.
///
/// Returns `None` when `n < 2` (no degrees of freedom) or when `sem`
/// is non-finite/negative.
///
/// References: Student (1908); scipy.stats.t.ppf.
pub fn t_distribution_ci(mean: f64, sem: f64, n: usize, alpha: f64) -> Option<ConfidenceInterval> {
    if n < 2 || !sem.is_finite() || sem < 0.0 || !(0.0 < alpha && alpha < 1.0) {
        return None;
    }
    let df = (n - 1) as f64;
    let dist = StudentsT::new(0.0, 1.0, df).ok()?;
    let q = dist.inverse_cdf(1.0 - alpha / 2.0);
    let half = q * sem;
    Some(ConfidenceInterval {
        lower: mean - half,
        upper: mean + half,
        level: 1.0 - alpha,
    })
}

/// Wilson score interval for a sample proportion.
///
/// Robust to small samples and proportions near 0 or 1, where the naive
/// normal-approximation interval misbehaves. `successes` ≤ `n`. `alpha`
/// is the two-sided significance level.
///
/// Returns `None` when `n == 0`, `successes > n`, or `alpha` is out of range.
///
/// Reference: Wilson (1927); Brown, Cai & DasGupta (2001).
pub fn wilson_score_ci(successes: u64, n: u64, alpha: f64) -> Option<ConfidenceInterval> {
    if n == 0 || successes > n || !(0.0 < alpha && alpha < 1.0) {
        return None;
    }
    let n_f = n as f64;
    let p_hat = successes as f64 / n_f;
    let z = Normal::new(0.0, 1.0).ok()?.inverse_cdf(1.0 - alpha / 2.0);
    let z2 = z * z;
    let denom = 1.0 + z2 / n_f;
    let centre = p_hat + z2 / (2.0 * n_f);
    let margin = z * ((p_hat * (1.0 - p_hat) / n_f) + z2 / (4.0 * n_f * n_f)).sqrt();
    Some(ConfidenceInterval {
        lower: ((centre - margin) / denom).max(0.0),
        upper: ((centre + margin) / denom).min(1.0),
        level: 1.0 - alpha,
    })
}

/// Generic percentile-bootstrap CI.
///
/// Resamples `samples` with replacement `n_resamples` times, applies
/// `statistic`, then returns the empirical α/2 and 1−α/2 percentiles.
///
/// `seed` is required for reproducibility — pass the same value to repeat
/// a run exactly. Uses a simple xorshift64* generator (no external RNG dep).
///
/// Returns `None` when `samples` is empty or `alpha` is out of range.
pub fn bootstrap_ci<F>(
    samples: &[f64],
    n_resamples: usize,
    alpha: f64,
    seed: u64,
    statistic: F,
) -> Option<ConfidenceInterval>
where
    F: Fn(&[f64]) -> f64,
{
    if samples.is_empty() || n_resamples == 0 || !(0.0 < alpha && alpha < 1.0) {
        return None;
    }
    let mut rng_state = seed.max(1); // xorshift requires non-zero state
    let mut buf = Vec::with_capacity(samples.len());
    let mut estimates = Vec::with_capacity(n_resamples);

    for _ in 0..n_resamples {
        buf.clear();
        for _ in 0..samples.len() {
            rng_state = xorshift64star(rng_state);
            let idx = (rng_state as usize) % samples.len();
            buf.push(samples[idx]);
        }
        estimates.push(statistic(&buf));
    }
    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let lower_idx = ((alpha / 2.0) * n_resamples as f64).floor() as usize;
    let upper_idx = (((1.0 - alpha / 2.0) * n_resamples as f64).ceil() as usize)
        .saturating_sub(1)
        .min(n_resamples - 1);

    Some(ConfidenceInterval {
        lower: estimates[lower_idx],
        upper: estimates[upper_idx],
        level: 1.0 - alpha,
    })
}

/// xorshift64* — small, fast, deterministic PRNG suitable for bootstrap
/// resampling. Period 2^64 − 1; not cryptographic.
#[inline]
fn xorshift64star(mut state: u64) -> u64 {
    state ^= state >> 12;
    state ^= state << 25;
    state ^= state >> 27;
    state.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn t_ci_matches_known_values() {
        // n=10, mean=0.5, sem=0.1, alpha=0.05 → t_{9, 0.975} ≈ 2.262
        // half-width ≈ 0.2262, CI ≈ (0.2738, 0.7262).
        let ci = t_distribution_ci(0.5, 0.1, 10, 0.05).unwrap();
        assert!(approx_eq(ci.lower, 0.27376, 1e-4));
        assert!(approx_eq(ci.upper, 0.72624, 1e-4));
        assert!(approx_eq(ci.level, 0.95, 1e-12));
    }

    #[test]
    fn t_ci_rejects_invalid_inputs() {
        assert!(t_distribution_ci(0.5, 0.1, 1, 0.05).is_none());
        assert!(t_distribution_ci(0.5, -0.1, 10, 0.05).is_none());
        assert!(t_distribution_ci(0.5, 0.1, 10, 0.0).is_none());
        assert!(t_distribution_ci(0.5, 0.1, 10, 1.0).is_none());
    }

    #[test]
    fn wilson_ci_matches_known_values() {
        // 8 / 10 successes, alpha=0.05 → wilson ≈ (0.490, 0.943) per scipy.
        let ci = wilson_score_ci(8, 10, 0.05).unwrap();
        assert!(approx_eq(ci.lower, 0.4904, 1e-3));
        assert!(approx_eq(ci.upper, 0.9434, 1e-3));
    }

    #[test]
    fn wilson_ci_handles_extremes() {
        // Mathematically, p_hat = 0 ⇒ lower bound = 0; FP rounding leaves a
        // sub-1e-15 residual that's effectively zero.
        let ci0 = wilson_score_ci(0, 10, 0.05).unwrap();
        assert!(ci0.lower.abs() < 1e-12);
        assert!(ci0.upper > 0.0 && ci0.upper < 1.0);

        let ci_full = wilson_score_ci(10, 10, 0.05).unwrap();
        assert!(ci_full.lower > 0.0 && ci_full.lower < 1.0);
        assert!((ci_full.upper - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bootstrap_ci_recovers_known_distribution() {
        // Bootstrap CI of the mean should bracket the true mean with high
        // probability for a moderately-sized iid sample.
        let samples: Vec<f64> = (0..200).map(|i| (i as f64) / 200.0).collect();
        let ci = bootstrap_ci(&samples, 1000, 0.05, 42, |xs| {
            xs.iter().sum::<f64>() / xs.len() as f64
        })
        .unwrap();
        let true_mean = 0.4975;
        assert!(ci.lower < true_mean && true_mean < ci.upper);
        assert!(ci.width() < 0.1);
    }

    #[test]
    fn bootstrap_is_deterministic_per_seed() {
        let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let stat = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
        let a = bootstrap_ci(&samples, 500, 0.05, 7, stat).unwrap();
        let b = bootstrap_ci(&samples, 500, 0.05, 7, stat).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn bootstrap_rejects_invalid_inputs() {
        let stat = |xs: &[f64]| xs.iter().sum::<f64>();
        assert!(bootstrap_ci::<_>(&[], 100, 0.05, 1, stat).is_none());
        assert!(bootstrap_ci::<_>(&[1.0], 0, 0.05, 1, stat).is_none());
        assert!(bootstrap_ci::<_>(&[1.0], 100, 0.0, 1, stat).is_none());
    }
}
