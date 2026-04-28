//! Pairwise judge — compares two responses side-by-side and returns a
//! verdict. The implementation enforces **dual-pass position swap**: the
//! same judge is asked twice with the responses in opposite orders, and
//! the comparison only counts if the verdicts agree. This neutralises the
//! position bias documented in Shi et al. 2025 (60–75% on single-pass).
//!
//! Returned verdicts use the calling-side names directly (`"a"` / `"b"`),
//! the judge never sees those names — internally the responses are
//! presented as `Response 1` and `Response 2`.

use anyhow::{Context, Result};

use crate::judges::client::JudgeClient;

/// Outcome of one pairwise comparison after dual-pass reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairwiseDecision {
    AWins,
    BWins,
    Tie,
    /// Position swap produced inconsistent verdicts — treated as Tie for
    /// scoring but flagged separately so we can report position-bias rate.
    PositionInconsistent,
}

/// One round of dual-pass comparison plus the raw text the judge wrote.
#[derive(Debug, Clone)]
pub struct PairwiseRecord {
    pub decision: PairwiseDecision,
    pub forward_raw: String,
    pub swapped_raw: String,
    pub judge_model: String,
}

/// Default rubric — concise, mirrors LMSYS Arena's pairwise template.
pub const DEFAULT_PAIRWISE_SYSTEM: &str = "\
You are an impartial judge comparing two AI assistant responses to the same user request.
Pick whichever response is better overall, considering helpfulness, accuracy, and clarity.
Reply with EXACTLY one of these tokens (no extra text): \
[[1]] if Response 1 is better, [[2]] if Response 2 is better, [[T]] if they are equally good.
";

/// Render the user-side prompt the judge actually grades.
pub fn render_pairwise_prompt(question: &str, response_1: &str, response_2: &str) -> String {
    format!(
        "User question:\n\n{question}\n\n\
         Response 1:\n\n{response_1}\n\n\
         Response 2:\n\n{response_2}\n\n\
         Which is better? Reply with one of [[1]], [[2]], or [[T]] only."
    )
}

/// Internal verdict returned by parsing one judge response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SinglePassVerdict {
    R1,
    R2,
    Tie,
    Unparseable,
}

/// Parse the judge's reply into one of the three tokens. Tolerant of
/// extra prose around the marker.
fn parse_single_pass(text: &str) -> SinglePassVerdict {
    let upper = text.to_ascii_uppercase();
    let mut found = None;
    for marker in ["[[1]]", "[[2]]", "[[T]]"] {
        if upper.contains(marker) {
            // Honour the *last* marker the judge wrote — chain-of-thought
            // judges sometimes mention earlier candidates before deciding.
            let idx = upper.rfind(marker).unwrap();
            if found.map(|(i, _)| idx > i).unwrap_or(true) {
                found = Some((idx, marker));
            }
        }
    }
    match found.map(|(_, m)| m) {
        Some("[[1]]") => SinglePassVerdict::R1,
        Some("[[2]]") => SinglePassVerdict::R2,
        Some("[[T]]") => SinglePassVerdict::Tie,
        _ => SinglePassVerdict::Unparseable,
    }
}

/// Run the dual-pass comparison.
///
/// `question` is the user-facing task; `response_a` and `response_b` are
/// the candidate outputs the eval is scoring. The names "a"/"b" are used
/// only on the caller side — the judge sees them as Response 1 / 2 in
/// each pass.
pub async fn compare_dual_pass(
    judge: &dyn JudgeClient,
    question: &str,
    response_a: &str,
    response_b: &str,
) -> Result<PairwiseRecord> {
    // Forward: a as Response 1, b as Response 2.
    let forward_user = render_pairwise_prompt(question, response_a, response_b);
    let forward = judge
        .complete(DEFAULT_PAIRWISE_SYSTEM, &forward_user)
        .await
        .context("forward pairwise pass")?;

    // Swapped: b as Response 1, a as Response 2.
    let swapped_user = render_pairwise_prompt(question, response_b, response_a);
    let swapped = judge
        .complete(DEFAULT_PAIRWISE_SYSTEM, &swapped_user)
        .await
        .context("swapped pairwise pass")?;

    let v1 = parse_single_pass(&forward.text);
    let v2 = parse_single_pass(&swapped.text);

    // Map to caller-side a/b: in the swapped pass, R1 is b and R2 is a.
    let decision = reconcile(v1, v2);

    Ok(PairwiseRecord {
        decision,
        forward_raw: forward.text,
        swapped_raw: swapped.text,
        judge_model: forward.model,
    })
}

/// Combine forward and swapped verdicts.
///
/// In the forward pass: `R1 = a`, `R2 = b`.
/// In the swapped pass: `R1 = b`, `R2 = a`.
///
/// The judge consistently prefers `a` iff forward = R1 and swapped = R2.
/// Likewise consistently prefers `b` iff forward = R2 and swapped = R1.
/// Anything else is treated as position bias / inconsistency.
fn reconcile(forward: SinglePassVerdict, swapped: SinglePassVerdict) -> PairwiseDecision {
    use SinglePassVerdict::*;
    match (forward, swapped) {
        (R1, R2) => PairwiseDecision::AWins, // both passes pick a
        (R2, R1) => PairwiseDecision::BWins, // both passes pick b
        (Tie, Tie) => PairwiseDecision::Tie,
        (Unparseable, _) | (_, Unparseable) => PairwiseDecision::PositionInconsistent,
        // Any other combination = position bias.
        _ => PairwiseDecision::PositionInconsistent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judges::client::{JudgeFamily, MockJudgeClient};

    #[test]
    fn parses_each_marker() {
        assert_eq!(parse_single_pass("[[1]]"), SinglePassVerdict::R1);
        assert_eq!(parse_single_pass("[[2]]"), SinglePassVerdict::R2);
        assert_eq!(parse_single_pass("[[T]]"), SinglePassVerdict::Tie);
        assert_eq!(parse_single_pass("verdict: [[1]]."), SinglePassVerdict::R1);
        assert_eq!(parse_single_pass("nothing"), SinglePassVerdict::Unparseable);
    }

    #[test]
    fn parses_last_marker_when_judge_thinks_aloud() {
        // CoT judge mentions option 1 first then settles on 2.
        let text = "Considering [[1]] would be acceptable, but actually [[2]] is more accurate.";
        assert_eq!(parse_single_pass(text), SinglePassVerdict::R2);
    }

    #[test]
    fn reconcile_consistent_a() {
        assert_eq!(
            reconcile(SinglePassVerdict::R1, SinglePassVerdict::R2),
            PairwiseDecision::AWins
        );
    }

    #[test]
    fn reconcile_consistent_b() {
        assert_eq!(
            reconcile(SinglePassVerdict::R2, SinglePassVerdict::R1),
            PairwiseDecision::BWins
        );
    }

    #[test]
    fn reconcile_position_bias() {
        // Judge picks R1 in both passes → swap reveals position preference.
        assert_eq!(
            reconcile(SinglePassVerdict::R1, SinglePassVerdict::R1),
            PairwiseDecision::PositionInconsistent
        );
        assert_eq!(
            reconcile(SinglePassVerdict::R2, SinglePassVerdict::R2),
            PairwiseDecision::PositionInconsistent
        );
    }

    #[test]
    fn reconcile_unparseable_falls_through() {
        assert_eq!(
            reconcile(SinglePassVerdict::Unparseable, SinglePassVerdict::R1),
            PairwiseDecision::PositionInconsistent
        );
    }

    #[tokio::test]
    async fn dual_pass_resolves_consistent_preference() {
        // Mock returns [[1]] forward then [[2]] swapped → both passes pick a.
        let judge = MockJudgeClient::new(
            JudgeFamily::Kimi,
            "kimi-k2.5",
            vec!["[[1]]".into(), "[[2]]".into()],
        );
        let rec = compare_dual_pass(&judge, "Q?", "answer-a", "answer-b")
            .await
            .unwrap();
        assert_eq!(rec.decision, PairwiseDecision::AWins);
    }

    #[tokio::test]
    async fn dual_pass_detects_position_bias() {
        // Mock always returns [[1]] regardless of order → position bias.
        let judge = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec!["[[1]]".into()]);
        let rec = compare_dual_pass(&judge, "Q?", "answer-a", "answer-b")
            .await
            .unwrap();
        assert_eq!(rec.decision, PairwiseDecision::PositionInconsistent);
    }

    #[tokio::test]
    async fn dual_pass_recognises_real_tie() {
        let judge = MockJudgeClient::new(
            JudgeFamily::Kimi,
            "kimi-k2.5",
            vec!["[[T]]".into(), "[[T]]".into()],
        );
        let rec = compare_dual_pass(&judge, "Q?", "answer-a", "answer-b")
            .await
            .unwrap();
        assert_eq!(rec.decision, PairwiseDecision::Tie);
    }
}
