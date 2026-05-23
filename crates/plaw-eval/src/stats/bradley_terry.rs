//! Bradley-Terry MLE — estimates a strength score per model from pairwise
//! win/loss records, the way LMSYS Arena ranks chat models.
//!
//! Given a set of comparisons `(i, j, winner)`, the model
//!
//! ```text
//! P(i beats j) = p_i / (p_i + p_j)
//! ```
//!
//! has a unique MLE up to scaling. We solve it with the
//! Minorization-Maximization (MM) update from Hunter (2004),
//! "MM algorithms for generalized Bradley-Terry models":
//!
//! ```text
//! p_i^{(t+1)} = w_i / Σ_{j≠i} (n_{ij} / (p_i^{(t)} + p_j^{(t)}))
//! ```
//!
//! where w_i is i's total wins, n_{ij} the number of i-vs-j matches.
//!
//! At the end we normalise to Σ p_i = 1.

use std::collections::HashMap;

use crate::stats::ci::{bootstrap_ci, ConfidenceInterval};

/// Convergence tolerance for the iterative solver (max parameter delta).
pub const DEFAULT_TOLERANCE: f64 = 1e-8;
/// Hard iteration cap for the solver.
pub const DEFAULT_MAX_ITERS: usize = 1000;
/// Lower bound on parameter values to keep updates numerically stable.
const FLOOR: f64 = 1e-12;

/// One pairwise comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comparison<I> {
    pub a: I,
    pub b: I,
    pub winner: Winner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    A,
    B,
    Tie,
}

/// Result of the MLE: a map from each model's index to its estimated score
/// and (optionally) its bootstrap CI.
#[derive(Debug, Clone)]
pub struct BradleyTerryEstimate<I> {
    pub scores: HashMap<I, f64>,
    pub iterations: usize,
    pub converged: bool,
}

impl<I: Clone + Eq + std::hash::Hash> BradleyTerryEstimate<I> {
    /// Sorted list of (id, score) descending — the leaderboard view.
    pub fn ranking(&self) -> Vec<(I, f64)> {
        let mut v: Vec<(I, f64)> = self.scores.iter().map(|(k, v)| (k.clone(), *v)).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v
    }
}

/// Fit the Bradley-Terry model on a list of comparisons.
///
/// Ties are split as half a win to each side. Returns `None` if there are
/// fewer than 2 distinct entrants or no decided games at all.
pub fn bradley_terry_mle<I>(
    comparisons: &[Comparison<I>],
    max_iters: usize,
    tolerance: f64,
) -> Option<BradleyTerryEstimate<I>>
where
    I: Eq + std::hash::Hash + Clone,
{
    if comparisons.is_empty() {
        return None;
    }

    // Collect entrants and per-entrant win/match counts.
    let mut wins: HashMap<I, f64> = HashMap::new();
    let mut matches: HashMap<(I, I), f64> = HashMap::new();
    let mut entrants: Vec<I> = Vec::new();

    for c in comparisons {
        for k in [&c.a, &c.b] {
            if !wins.contains_key(k) {
                wins.insert(k.clone(), 0.0);
                entrants.push(k.clone());
            }
        }
        let pair_ab = (c.a.clone(), c.b.clone());
        let pair_ba = (c.b.clone(), c.a.clone());
        *matches.entry(pair_ab.clone()).or_insert(0.0) += 1.0;
        *matches.entry(pair_ba.clone()).or_insert(0.0) += 1.0;
        match c.winner {
            Winner::A => *wins.entry(c.a.clone()).or_insert(0.0) += 1.0,
            Winner::B => *wins.entry(c.b.clone()).or_insert(0.0) += 1.0,
            Winner::Tie => {
                *wins.entry(c.a.clone()).or_insert(0.0) += 0.5;
                *wins.entry(c.b.clone()).or_insert(0.0) += 0.5;
            }
        }
    }

    if entrants.len() < 2 {
        return None;
    }

    // Initialise parameters uniformly.
    let n = entrants.len();
    let mut params: HashMap<I, f64> = entrants
        .iter()
        .map(|k| (k.clone(), 1.0 / n as f64))
        .collect();

    let mut converged = false;
    let mut iters = 0;
    for it in 0..max_iters {
        iters = it + 1;
        let mut next: HashMap<I, f64> = HashMap::with_capacity(n);

        for i in &entrants {
            let mut denom = 0.0;
            for j in &entrants {
                if i == j {
                    continue;
                }
                let n_ij = matches.get(&(i.clone(), j.clone())).copied().unwrap_or(0.0);
                if n_ij == 0.0 {
                    continue;
                }
                let p_i = params[i];
                let p_j = params[j];
                denom += n_ij / (p_i + p_j);
            }
            let w = wins[i];
            let new_val = if denom > 0.0 { w / denom } else { params[i] };
            next.insert(i.clone(), new_val.max(FLOOR));
        }

        // Renormalise so Σ p_i = 1.
        let total: f64 = next.values().sum();
        if total > 0.0 {
            for v in next.values_mut() {
                *v /= total;
            }
        }

        // Check convergence (max absolute delta).
        let max_delta = entrants
            .iter()
            .map(|k| (next[k] - params[k]).abs())
            .fold(0.0_f64, f64::max);
        params = next;
        if max_delta < tolerance {
            converged = true;
            break;
        }
    }

    Some(BradleyTerryEstimate {
        scores: params,
        iterations: iters,
        converged,
    })
}

/// Bootstrap CI for each entrant's score by resampling the comparisons with
/// replacement and re-fitting. Returns `None` when the underlying MLE fails.
pub fn bradley_terry_bootstrap_ci<I>(
    comparisons: &[Comparison<I>],
    n_resamples: usize,
    alpha: f64,
    seed: u64,
) -> Option<HashMap<I, ConfidenceInterval>>
where
    I: Eq + std::hash::Hash + Clone,
{
    if comparisons.is_empty() || n_resamples == 0 {
        return None;
    }

    // Build a complete list of entrants up front so every bootstrap pass
    // produces scores keyed by the same set.
    let mut entrants: Vec<I> = Vec::new();
    for c in comparisons {
        if !entrants.iter().any(|e| e == &c.a) {
            entrants.push(c.a.clone());
        }
        if !entrants.iter().any(|e| e == &c.b) {
            entrants.push(c.b.clone());
        }
    }

    // For each entrant, collect a vector of bootstrap-replicate scores.
    let mut score_samples: HashMap<I, Vec<f64>> = entrants
        .iter()
        .map(|k| (k.clone(), Vec::with_capacity(n_resamples)))
        .collect();

    let mut rng_state = seed.max(1);
    let mut buf: Vec<Comparison<I>> = Vec::with_capacity(comparisons.len());

    for _ in 0..n_resamples {
        buf.clear();
        for _ in 0..comparisons.len() {
            rng_state = xorshift64star(rng_state);
            let idx = (rng_state as usize) % comparisons.len();
            buf.push(comparisons[idx].clone());
        }
        if let Some(est) = bradley_terry_mle(&buf, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE) {
            for k in &entrants {
                let v = est.scores.get(k).copied().unwrap_or(FLOOR);
                score_samples.get_mut(k).unwrap().push(v);
            }
        }
    }

    let mut out = HashMap::new();
    for (k, mut samples) in score_samples {
        if samples.is_empty() {
            continue;
        }
        // Use bootstrap_ci's percentile machinery indirectly: the "statistic"
        // is the identity, so we just take the percentiles ourselves.
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = samples.len();
        let lower = samples[((alpha / 2.0) * n as f64).floor() as usize];
        let upper = samples[(((1.0 - alpha / 2.0) * n as f64).ceil() as usize)
            .saturating_sub(1)
            .min(n - 1)];
        out.insert(
            k,
            ConfidenceInterval {
                lower,
                upper,
                level: 1.0 - alpha,
            },
        );
    }

    // Silence unused-import warnings — we keep the symbol available for
    // callers wanting a quick percentile CI on derived quantities.
    let _ = bootstrap_ci::<fn(&[f64]) -> f64>;
    Some(out)
}

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

    fn comp<I: Clone>(a: I, b: I, w: Winner) -> Comparison<I> {
        Comparison { a, b, winner: w }
    }

    #[test]
    fn three_player_round_robin_ranks_by_wins() {
        // Round-robin where A beats B and C, B beats C: ranking A > B > C.
        let cs = vec![
            comp("A", "B", Winner::A),
            comp("A", "B", Winner::A),
            comp("A", "C", Winner::A),
            comp("A", "C", Winner::A),
            comp("B", "C", Winner::A),
            comp("B", "C", Winner::A),
        ];
        let est = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).unwrap();
        let ranking = est.ranking();
        assert_eq!(ranking[0].0, "A");
        assert_eq!(ranking[1].0, "B");
        assert_eq!(ranking[2].0, "C");
        // Scores form a strictly decreasing sequence.
        assert!(ranking[0].1 > ranking[1].1);
        assert!(ranking[1].1 > ranking[2].1);
    }

    #[test]
    fn equal_strength_yields_equal_scores() {
        let cs = vec![
            comp("A", "B", Winner::A),
            comp("A", "B", Winner::B),
            comp("A", "B", Winner::A),
            comp("A", "B", Winner::B),
        ];
        let est = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).unwrap();
        let a = est.scores["A"];
        let b = est.scores["B"];
        assert!(
            (a - b).abs() < 1e-3,
            "expected ≈ equal scores, got A={a}, B={b}"
        );
        assert!((a + b - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ties_split_credit() {
        // 4 ties between A and B should leave them ≈ equal.
        let cs = vec![
            comp("A", "B", Winner::Tie),
            comp("A", "B", Winner::Tie),
            comp("A", "B", Winner::Tie),
            comp("A", "B", Winner::Tie),
        ];
        let est = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).unwrap();
        assert!((est.scores["A"] - est.scores["B"]).abs() < 1e-6);
    }

    #[test]
    fn rejects_empty_or_singleton() {
        assert!(bradley_terry_mle::<&str>(&[], DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).is_none());
        let only_self = vec![comp("A", "A", Winner::A)];
        assert!(bradley_terry_mle(&only_self, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).is_none());
    }

    #[test]
    fn bootstrap_ci_brackets_point_estimate() {
        let cs: Vec<Comparison<&str>> = (0..50)
            .flat_map(|i| {
                if i % 3 == 0 {
                    vec![comp("A", "B", Winner::A)]
                } else if i % 3 == 1 {
                    vec![comp("A", "B", Winner::B)]
                } else {
                    vec![comp("A", "B", Winner::A)]
                }
            })
            .collect();

        let est = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).unwrap();
        let cis = bradley_terry_bootstrap_ci(&cs, 200, 0.05, 42).unwrap();

        for k in ["A", "B"] {
            let p = est.scores[k];
            let ci = cis[k];
            assert!(
                ci.lower <= p + 1e-9 && p <= ci.upper + 1e-9,
                "score {p} should lie inside CI [{}, {}]",
                ci.lower,
                ci.upper
            );
        }
    }

    // ── Property-based tests (proptest) ───────────────────────────────────
    //
    // Hand-tests above pin specific numerical examples. proptest scales
    // those checks across thousands of arbitrary tournaments and exercises
    // four invariants the MLE must satisfy:
    //
    //   1. probabilities sum to 1 (normalisation contract)
    //   2. all scores are strictly positive (FLOOR > 0 enforcement)
    //   3. ranking is invariant under shuffling the comparisons input
    //   4. strict dominator: an entrant that wins every comparison it
    //      participates in (and loses none) outranks the rest
    //
    // A broken MLE would silently mis-rank model configurations downstream
    // — the whole purpose of Bradley-Terry is to extract a stable ranking
    // from noisy pairwise data, so the ranking-invariance properties are
    // load-bearing for any consumer that reads `BradleyTerryEstimate::ranking()`.

    use proptest::prelude::*;

    /// Generate an arbitrary tournament between 2-5 entrants with 3-30
    /// comparisons. The generator never produces a degenerate single-
    /// entrant set (MLE requires ≥2 distinct entrants).
    fn arb_tournament() -> impl Strategy<Value = Vec<Comparison<u8>>> {
        (2u8..=5u8).prop_flat_map(|n_entrants| {
            prop::collection::vec(
                (
                    0u8..n_entrants,
                    0u8..n_entrants,
                    prop_oneof![
                        Just(Winner::A),
                        Just(Winner::B),
                        Just(Winner::Tie),
                    ],
                )
                    .prop_filter("a != b", |(a, b, _)| a != b)
                    .prop_map(|(a, b, winner)| Comparison { a, b, winner }),
                3..=30,
            )
        })
    }

    proptest! {
        /// Normalisation contract: Σ scores == 1 (within float tolerance).
        /// A consumer iterating over `ranking()` expects scores to be
        /// directly comparable as probabilities of head-to-head wins.
        #[test]
        fn scores_sum_to_one(cs in arb_tournament()) {
            let Some(est) = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE) else {
                // Some tournaments are degenerate (all ties, or every
                // comparison between the same single pair where the MLE
                // can't separate). Skip — the public API documents None
                // for these.
                return Ok(());
            };
            let total: f64 = est.scores.values().sum();
            prop_assert!(
                (total - 1.0).abs() < 1e-6,
                "scores must sum to 1.0; got {} from {:?}",
                total, est.scores
            );
        }

        /// All scores are strictly positive (FLOOR > 0 enforcement). A
        /// zero or negative score would break log-odds calculations and
        /// crash bootstrap CI computation downstream.
        #[test]
        fn all_scores_strictly_positive(cs in arb_tournament()) {
            let Some(est) = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE) else {
                return Ok(());
            };
            for (id, score) in &est.scores {
                prop_assert!(*score > 0.0, "score for {id:?} must be > 0, got {score}");
            }
        }

        /// Shuffle invariance: the same comparisons in a different order
        /// produce the same ranking (modulo ties). Catches a regression
        /// where the MLE initialisation or iteration order accidentally
        /// becomes input-order-dependent.
        #[test]
        fn ranking_is_invariant_under_input_shuffle(
            cs in arb_tournament(),
            permutation_seed in any::<u64>(),
        ) {
            let Some(est_orig) = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE) else {
                return Ok(());
            };

            // Deterministic shuffle using the permutation_seed.
            let mut shuffled = cs.clone();
            let mut rng_state = permutation_seed;
            for i in (1..shuffled.len()).rev() {
                rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let j = (rng_state >> 33) as usize % (i + 1);
                shuffled.swap(i, j);
            }

            let est_shuffled = bradley_terry_mle(&shuffled, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).unwrap();

            // Compare entry by entry; the same MLE optimum should be
            // reached up to convergence tolerance.
            for (id, score_orig) in &est_orig.scores {
                let score_shuf = est_shuffled.scores[id];
                prop_assert!(
                    (score_orig - score_shuf).abs() < 1e-4,
                    "shuffled MLE diverged for {id:?}: orig={score_orig}, shuffled={score_shuf}"
                );
            }
        }

        /// Strict dominator: an entrant that wins every comparison it
        /// participates in (no ties, no losses) ranks strictly higher
        /// than every other entrant.
        #[test]
        fn strict_dominator_outranks_rest(
            n_other in 1u8..=4u8,
            wins_per_other in 1usize..=5usize,
        ) {
            // Build a tournament where entrant 0 beats every other entrant
            // `wins_per_other` times. No ties, no losses for entrant 0.
            let mut cs = Vec::new();
            for other in 1..=n_other {
                for _ in 0..wins_per_other {
                    cs.push(Comparison {
                        a: 0u8,
                        b: other,
                        winner: Winner::A,
                    });
                }
            }
            // Add some between-others comparisons so the rest are also
            // ranked relative to each other (not just unrankable noise).
            if n_other >= 2 {
                cs.push(Comparison {
                    a: 1u8,
                    b: 2u8,
                    winner: Winner::A,
                });
            }

            let est = bradley_terry_mle(&cs, DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE).unwrap();
            let dominator_score = est.scores[&0u8];
            for other in 1..=n_other {
                let other_score = est.scores[&other];
                prop_assert!(
                    dominator_score > other_score,
                    "dominator score {dominator_score} must exceed entrant-{other} score {other_score}"
                );
            }
        }
    }
}
