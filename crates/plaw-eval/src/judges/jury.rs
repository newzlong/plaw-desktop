//! Multi-judge jury — runs N judges in parallel, then aggregates their
//! verdicts. Enforces **cross-family composition**: at construction time
//! we refuse a jury if every member belongs to the same model family,
//! mitigating self-preference bias (Liu 2024).
//!
//! Aggregation strategies:
//! - [`JuryAggregator::Majority`] — strict ≥ ceil(n/2) agreement; otherwise
//!   tie. The default for production gating.
//! - [`JuryAggregator::ConfidenceWeighted`] — Liu et al. "LLM-as-a-Fuser"
//!   style: each verdict carries an implicit confidence (1.0 for clear
//!   verdicts, 0.5 for inconsistent passes). The verdict with highest
//!   summed weight wins.

use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::judges::client::{JudgeClient, JudgeFamily};
use crate::judges::pairwise::{compare_dual_pass, PairwiseDecision, PairwiseRecord};
use crate::suite::JuryAggregator;

/// One judge's record from a jury session. Kept around for audit / drift
/// analysis even after the jury collapses to a single verdict.
#[derive(Debug, Clone)]
pub struct JuryMemberRecord {
    pub family: JudgeFamily,
    pub model: String,
    pub record: PairwiseRecord,
}

/// Final jury output.
#[derive(Debug, Clone)]
pub struct JuryVerdict {
    pub decision: PairwiseDecision,
    pub members: Vec<JuryMemberRecord>,
    /// True if the aggregator could not form a clear majority — useful for
    /// pushing borderline cases into the manual-review queue.
    pub inconclusive: bool,
}

/// A panel of judges. Construct via [`Jury::new`] which validates the
/// cross-family invariant.
pub struct Jury {
    members: Vec<Arc<dyn JudgeClient>>,
    aggregator: JuryAggregator,
}

impl std::fmt::Debug for Jury {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jury")
            .field("members", &self.members.len())
            .field("aggregator", &self.aggregator)
            .finish()
    }
}

impl Jury {
    /// Build a jury, refusing single-family panels.
    ///
    /// `min_distinct_families` is the minimum number of *distinct* model
    /// families the jury must contain. Two is the production minimum;
    /// callers can raise it for higher-stakes evals.
    pub fn new(
        members: Vec<Arc<dyn JudgeClient>>,
        aggregator: JuryAggregator,
        min_distinct_families: usize,
    ) -> Result<Self> {
        if members.is_empty() {
            return Err(anyhow!("jury requires at least one member"));
        }
        let distinct: std::collections::HashSet<_> =
            members.iter().map(|m| m.family()).collect();
        if distinct.len() < min_distinct_families {
            return Err(anyhow!(
                "jury must include at least {} distinct families (got {} from {} members)",
                min_distinct_families,
                distinct.len(),
                members.len()
            ));
        }
        Ok(Self { members, aggregator })
    }

    /// For ad-hoc use cases (single-judge sanity checks etc.) where the
    /// cross-family rule should be bypassed. Logs a warning so callers
    /// don't accidentally silence the safety net in production.
    pub fn new_unchecked(
        members: Vec<Arc<dyn JudgeClient>>,
        aggregator: JuryAggregator,
    ) -> Self {
        if members.iter().map(|m| m.family()).collect::<std::collections::HashSet<_>>().len() < 2 {
            tracing::warn!(
                "Jury::new_unchecked: single-family jury created with {} members — \
                 self-preference bias is unmitigated",
                members.len()
            );
        }
        Self { members, aggregator }
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Run a pairwise dual-pass on every member in parallel, then collapse.
    pub async fn pairwise(
        &self,
        question: &str,
        response_a: &str,
        response_b: &str,
    ) -> Result<JuryVerdict> {
        let futures: Vec<_> = self
            .members
            .iter()
            .map(|m| {
                let m = m.clone();
                let q = question.to_string();
                let a = response_a.to_string();
                let b = response_b.to_string();
                async move {
                    let record = compare_dual_pass(&*m, &q, &a, &b).await?;
                    Ok::<_, anyhow::Error>(JuryMemberRecord {
                        family: m.family(),
                        model: m.model().to_string(),
                        record,
                    })
                }
            })
            .collect();

        let results = futures_util::future::join_all(futures).await;
        let mut members = Vec::with_capacity(results.len());
        for r in results {
            members.push(r?);
        }

        let (decision, inconclusive) = match self.aggregator {
            JuryAggregator::Majority => aggregate_majority(&members),
            JuryAggregator::ConfidenceWeighted => aggregate_confidence_weighted(&members),
        };
        Ok(JuryVerdict {
            decision,
            members,
            inconclusive,
        })
    }
}

/// Strict majority: a verdict needs ≥ ceil(n/2) votes. Inconsistent /
/// ambiguous member verdicts count as half a vote toward `Tie`.
fn aggregate_majority(members: &[JuryMemberRecord]) -> (PairwiseDecision, bool) {
    let n = members.len();
    let mut a = 0;
    let mut b = 0;
    let mut tie = 0;
    let mut incon = 0;
    for m in members {
        match m.record.decision {
            PairwiseDecision::AWins => a += 1,
            PairwiseDecision::BWins => b += 1,
            PairwiseDecision::Tie => tie += 1,
            PairwiseDecision::PositionInconsistent => incon += 1,
        }
    }
    // Strict majority: more than half. For n=2 both must agree; for n=3
    // two-of-three; etc.
    let needed = n / 2 + 1;
    if a >= needed {
        (PairwiseDecision::AWins, false)
    } else if b >= needed {
        (PairwiseDecision::BWins, false)
    } else if tie >= needed {
        (PairwiseDecision::Tie, false)
    } else if incon == n {
        // Every judge showed position bias — the comparison is unsalvageable.
        (PairwiseDecision::PositionInconsistent, true)
    } else {
        (PairwiseDecision::Tie, true)
    }
}

/// Confidence-weighted: confident verdicts (AWins/BWins/Tie) contribute 1.0,
/// PositionInconsistent contributes 0.5 toward Tie. The verdict with the
/// largest summed weight wins; ties between weight totals → inconclusive.
fn aggregate_confidence_weighted(
    members: &[JuryMemberRecord],
) -> (PairwiseDecision, bool) {
    let mut a: f64 = 0.0;
    let mut b: f64 = 0.0;
    let mut tie: f64 = 0.0;
    for m in members {
        match m.record.decision {
            PairwiseDecision::AWins => a += 1.0,
            PairwiseDecision::BWins => b += 1.0,
            PairwiseDecision::Tie => tie += 1.0,
            PairwiseDecision::PositionInconsistent => tie += 0.5,
        }
    }
    let max = a.max(b).max(tie);
    let inconclusive = (a == max && b == max && a > 0.0)
        || (a == max && tie == max && a > 0.0)
        || (b == max && tie == max && b > 0.0);
    if a == max && a >= b {
        (PairwiseDecision::AWins, inconclusive)
    } else if b == max {
        (PairwiseDecision::BWins, inconclusive)
    } else {
        (PairwiseDecision::Tie, inconclusive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judges::client::MockJudgeClient;

    fn mk(family: JudgeFamily, name: &str, responses: Vec<String>) -> Arc<dyn JudgeClient> {
        Arc::new(MockJudgeClient::new(family, name, responses))
    }

    #[test]
    fn rejects_single_family_jury() {
        let members: Vec<Arc<dyn JudgeClient>> = vec![
            mk(JudgeFamily::Kimi, "k1", vec!["[[1]]".into()]),
            mk(JudgeFamily::Kimi, "k2", vec!["[[1]]".into()]),
        ];
        let err = Jury::new(members, JuryAggregator::Majority, 2).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("distinct families"));
    }

    #[test]
    fn accepts_cross_family_jury() {
        let members: Vec<Arc<dyn JudgeClient>> = vec![
            mk(JudgeFamily::Kimi, "k", vec!["[[1]]".into()]),
            mk(JudgeFamily::Anthropic, "c", vec!["[[1]]".into()]),
        ];
        assert!(Jury::new(members, JuryAggregator::Majority, 2).is_ok());
    }

    #[test]
    fn unchecked_constructor_emits_warning_but_works() {
        let members: Vec<Arc<dyn JudgeClient>> = vec![
            mk(JudgeFamily::Kimi, "k1", vec!["[[1]]".into()]),
        ];
        let jury = Jury::new_unchecked(members, JuryAggregator::Majority);
        assert_eq!(jury.member_count(), 1);
    }

    #[tokio::test]
    async fn majority_resolves_when_two_of_three_agree() {
        // Three members; two consistently pick a, one picks b.
        // Each member's dual-pass needs forward + swapped responses.
        let members: Vec<Arc<dyn JudgeClient>> = vec![
            mk(JudgeFamily::Kimi, "k", vec!["[[1]]".into(), "[[2]]".into()]),       // → AWins
            mk(JudgeFamily::Anthropic, "c", vec!["[[1]]".into(), "[[2]]".into()]),  // → AWins
            mk(JudgeFamily::OpenAi, "g", vec!["[[2]]".into(), "[[1]]".into()]),     // → BWins
        ];
        let jury = Jury::new(members, JuryAggregator::Majority, 3).unwrap();
        let verdict = jury.pairwise("Q?", "a", "b").await.unwrap();
        assert_eq!(verdict.decision, PairwiseDecision::AWins);
        assert!(!verdict.inconclusive);
        assert_eq!(verdict.members.len(), 3);
    }

    #[tokio::test]
    async fn majority_marks_inconclusive_on_split_vote() {
        let members: Vec<Arc<dyn JudgeClient>> = vec![
            mk(JudgeFamily::Kimi, "k", vec!["[[1]]".into(), "[[2]]".into()]),    // AWins
            mk(JudgeFamily::Anthropic, "c", vec!["[[2]]".into(), "[[1]]".into()]), // BWins
        ];
        let jury = Jury::new(members, JuryAggregator::Majority, 2).unwrap();
        let verdict = jury.pairwise("Q?", "a", "b").await.unwrap();
        assert_eq!(verdict.decision, PairwiseDecision::Tie);
        assert!(verdict.inconclusive);
    }

    #[tokio::test]
    async fn confidence_weighted_aggregates_with_partial_weight() {
        // Two clear AWins, one position-inconsistent. The inconsistent one
        // contributes 0.5 to Tie; AWins (2.0) still wins decisively.
        let members: Vec<Arc<dyn JudgeClient>> = vec![
            mk(JudgeFamily::Kimi, "k", vec!["[[1]]".into(), "[[2]]".into()]),
            mk(JudgeFamily::Anthropic, "c", vec!["[[1]]".into(), "[[2]]".into()]),
            mk(JudgeFamily::OpenAi, "g", vec!["[[1]]".into(), "[[1]]".into()]), // bias
        ];
        let jury = Jury::new(members, JuryAggregator::ConfidenceWeighted, 3).unwrap();
        let verdict = jury.pairwise("Q?", "a", "b").await.unwrap();
        assert_eq!(verdict.decision, PairwiseDecision::AWins);
    }
}
