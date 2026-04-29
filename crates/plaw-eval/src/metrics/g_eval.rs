//! G-Eval — chain-of-thought judge with auto-generated evaluation steps,
//! form-filling, and (when available) log-probability-weighted scoring.
//!
//! Based on Liu et al., "G-EVAL: NLG Evaluation using GPT-4 with Better
//! Human Alignment" (EMNLP 2023, arXiv:2303.16634). The Anthropic-compat
//! transports plaw uses don't expose token logprobs, so we fall back to
//! a `confidence` field the judge writes in JSON, weighted into the score.
//!
//! Output is normalised to `[0, 1]` regardless of the underlying scale.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::judges::client::JudgeClient;

/// Configuration for one G-Eval invocation.
#[derive(Debug, Clone)]
pub struct GEvalConfig {
    /// What aspect to evaluate (e.g. "factual accuracy", "helpfulness").
    pub dimension: String,
    /// Inclusive max score (1..=scale). Anything ≥ 5 works; 5 is the
    /// canonical default.
    pub scale: u8,
    /// Domain hint included in the system prompt. Empty by default.
    pub task_context: String,
}

impl Default for GEvalConfig {
    fn default() -> Self {
        Self {
            dimension: "overall_quality".into(),
            scale: 5,
            task_context: String::new(),
        }
    }
}

/// One G-Eval scoring outcome.
#[derive(Debug, Clone)]
pub struct GEvalScore {
    /// Normalised value in `[0, 1]`.
    pub value: f64,
    /// Raw integer score the judge picked, in `[1, scale]`.
    pub raw_score: u8,
    /// Self-reported judge confidence, `[0, 1]`. Used to weight the score
    /// in the absence of true logprobs.
    pub confidence: f64,
    /// Full text the judge wrote (kept for audit).
    pub raw_text: String,
}

/// Score a single response using the configured judge.
pub async fn score(
    judge: &dyn JudgeClient,
    cfg: &GEvalConfig,
    question: &str,
    response: &str,
) -> Result<GEvalScore> {
    let system = render_system(cfg);
    let user = render_user(question, response, cfg.scale);
    let completion = judge
        .complete(&system, &user)
        .await
        .context("calling judge for G-Eval")?;
    let parsed = parse_judgement(&completion.text, cfg.scale)
        .with_context(|| format!("parsing G-Eval output: {}", completion.text))?;
    let normalised = normalise(parsed.score, cfg.scale, parsed.confidence);
    Ok(GEvalScore {
        value: normalised,
        raw_score: parsed.score,
        confidence: parsed.confidence,
        raw_text: completion.text,
    })
}

#[derive(Debug, Deserialize)]
struct Judgement {
    score: u8,
    #[serde(default = "default_confidence")]
    confidence: f64,
    #[serde(default)]
    #[allow(dead_code)]
    rationale: Option<String>,
}

fn default_confidence() -> f64 {
    1.0
}

/// System prompt instructs the judge to think step-by-step against an
/// **anchored rubric** with hard penalty rules. Earlier versions used a
/// vague "what would make a great response" framing that let judges
/// concentrate scores in the top 1-2 bands (baseline mean 0.92 vs
/// keyword_coverage 0.77 — see docs/eval/baseline-2026-Q2.md). Strict
/// anchors + hard rules force more spread.
fn render_system(cfg: &GEvalConfig) -> String {
    let mut s = String::from(
        "You are a strict evaluator. **Default to lower scores.** Reserve top \
         scores for responses that are genuinely exceptional on the dimension \
         below. Most decent answers should land at 3, not 5.\n\
         \n\
         === RUBRIC (anchored) ===\n\
         5 - Exemplary: directly answers, well-calibrated (admits limits when \
             unknown), concise, zero hallucinated specifics, follows EVERY \
             output constraint exactly.\n\
         4 - Good: correct answer with minor flaws — slightly verbose, light \
             preamble, or one missed nicety. No content errors.\n\
         3 - Adequate: answer is present but with notable issues — partial \
             coverage, tangents, missed an output constraint, OR over-hedging.\n\
         2 - Poor: significant flaws — partial wrongness, hallucinated specifics, \
             ignored a clear instruction, refused something reasonable.\n\
         1 - Failure: wrong answer, fabricated entire response, or refused a \
             clearly reasonable request.\n\
         \n\
         === HARD RULES (apply BEFORE rubric — these cap the score) ===\n\
         - Hallucinated concrete facts (dates, times, real-time data the model \
           cannot have, fake citations, fabricated numbers): cap at 2.\n\
         - Refusing a reasonable, harmless request: cap at 2.\n\
         - Ignoring an explicit user constraint (e.g. \"in one sentence\", \
           \"exactly 3 bullets\", \"no preamble\"): cap at 3.\n\
         - Unsolicited preamble (\"Great question!\", \"Let me help…\"), trailing \
           summary (\"Hope that helps!\"), or excessive markdown decoration when \
           the user just asked a simple question: cannot score 5.\n\
         - Verbose answer to a question that called for concision: cap at 4.\n\
         \n\
         === STEPS ===\n\
         1. Read the user request.\n\
         2. Apply each HARD RULE — note any that fire (these cap the score).\n\
         3. Pick the rubric anchor that fits.\n\
         4. Reply with JSON only.\n\
         \n\
         Evaluation dimension: ",
    );
    s.push_str(&cfg.dimension);
    if !cfg.task_context.is_empty() {
        s.push_str("\n\nTask context: ");
        s.push_str(&cfg.task_context);
    }
    s.push_str(
        "\n\nReply with valid JSON only:\n\
         {\"score\": <integer 1..=scale>, \"confidence\": <float 0..=1>, \
         \"rationale\": \"<one sentence — name the anchor or hard rule>\"}\n",
    );
    s
}

fn render_user(question: &str, response: &str, scale: u8) -> String {
    format!(
        "Scale: 1 (failure) to {scale} (exemplary). Be strict — the median \
         response should score around the middle of the scale, not the top.\n\n\
         === User question ===\n\n{question}\n\n\
         === Candidate response ===\n\n{response}\n\n\
         Reply with JSON only."
    )
}

/// Pull the JSON object out of the judge's reply (which sometimes includes
/// stray prose) and validate the score range.
fn parse_judgement(text: &str, scale: u8) -> Result<Judgement> {
    let json_slice = extract_json_object(text)
        .ok_or_else(|| anyhow::anyhow!("no JSON object found in judge reply"))?;
    let mut judgement: Judgement = serde_json::from_str(json_slice)
        .with_context(|| format!("decoding judge JSON: {json_slice}"))?;
    if judgement.score < 1 || judgement.score > scale {
        return Err(anyhow::anyhow!(
            "score {} outside valid range 1..={scale}",
            judgement.score
        ));
    }
    if !judgement.confidence.is_finite() {
        judgement.confidence = 1.0;
    }
    judgement.confidence = judgement.confidence.clamp(0.0, 1.0);
    Ok(judgement)
}

/// Find the first balanced `{...}` JSON object in `text`. Naive but
/// adequate for the structured outputs we ask judges to produce — we don't
/// need to parse arbitrarily nested strings here.
fn extract_json_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'"' => in_string = !in_string,
            b'\\' if in_string => escaped = true,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Map an integer score to `[0, 1]`, weighting by self-reported confidence
/// (low-confidence judgements pull toward the scale midpoint, since a
/// random guess would centre there in expectation).
fn normalise(raw: u8, scale: u8, confidence: f64) -> f64 {
    if scale == 0 {
        return 0.0;
    }
    let scale_f = scale as f64;
    // Linear normalisation to [0, 1] with the lowest score mapped to 0
    // (rather than 1/scale) so a "1/5" doesn't read as 0.2 baseline.
    let normalised = if scale > 1 {
        (raw as f64 - 1.0) / (scale_f - 1.0)
    } else {
        1.0
    };
    let midpoint = 0.5;
    confidence * normalised + (1.0 - confidence) * midpoint
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judges::client::{JudgeFamily, MockJudgeClient};

    #[test]
    fn normalise_endpoints() {
        // Full confidence: linear scaling from scale=5.
        assert!((normalise(1, 5, 1.0)).abs() < 1e-12);
        assert!((normalise(5, 5, 1.0) - 1.0).abs() < 1e-12);
        assert!((normalise(3, 5, 1.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn normalise_low_confidence_pulls_toward_midpoint() {
        // confidence=0 collapses everything to 0.5.
        assert!((normalise(5, 5, 0.0) - 0.5).abs() < 1e-12);
        assert!((normalise(1, 5, 0.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn extract_json_handles_surrounding_prose() {
        let text = "Reasoning: looks fine.\n{\"score\":4,\"confidence\":0.8}\nDone.";
        assert_eq!(
            extract_json_object(text).unwrap(),
            "{\"score\":4,\"confidence\":0.8}"
        );
    }

    #[test]
    fn extract_json_handles_nested_quotes() {
        let text = r#"{"score":3,"confidence":0.7,"rationale":"says \"hi\""}"#;
        assert_eq!(extract_json_object(text).unwrap(), text);
    }

    #[test]
    fn parse_rejects_out_of_range_score() {
        let bad = r#"{"score":7,"confidence":1.0}"#;
        assert!(parse_judgement(bad, 5).is_err());
        let neg = r#"{"score":0,"confidence":1.0}"#;
        assert!(parse_judgement(neg, 5).is_err());
    }

    #[tokio::test]
    async fn score_e2e_with_mock_judge() {
        let judge = MockJudgeClient::new(
            JudgeFamily::Kimi,
            "kimi-k2.5",
            vec![r#"Reasoning: solid answer.
{"score": 4, "confidence": 0.9, "rationale": "good"}"#
                .into()],
        );
        let cfg = GEvalConfig {
            dimension: "helpfulness".into(),
            scale: 5,
            task_context: String::new(),
        };
        let result = score(&judge, &cfg, "Hello?", "Hi there!").await.unwrap();
        assert_eq!(result.raw_score, 4);
        assert!((result.confidence - 0.9).abs() < 1e-9);
        // 4/5 ≈ 0.75 at full confidence; with 0.9 confidence, slightly
        // pulled toward 0.5: 0.9*0.75 + 0.1*0.5 = 0.725
        assert!((result.value - 0.725).abs() < 1e-9);
    }

    #[tokio::test]
    async fn score_propagates_parse_errors() {
        let judge = MockJudgeClient::new(
            JudgeFamily::Kimi,
            "kimi-k2.5",
            vec!["I refuse to provide a score.".into()],
        );
        let cfg = GEvalConfig::default();
        let err = score(&judge, &cfg, "Q?", "A.").await.unwrap_err();
        assert!(format!("{err:#}").contains("no JSON"));
    }
}
