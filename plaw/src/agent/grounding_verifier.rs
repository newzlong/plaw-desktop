//! Chain-of-Verification (CoV) post-response verifier.
//!
//! Given the assistant's final response, runs a SECOND LLM call against
//! the same (or a configured cheaper) model asking it to extract up to
//! `max_claims` verifiable factual claims and tag each `[OK]` (looks
//! correct) or `[VERIFY]` (suspicious / unverifiable). If any claim
//! comes back `[VERIFY]`, a markdown `[Verification]` footer is built
//! and returned. Otherwise returns `Ok(None)` — caller appends nothing.
//!
//! Pure async function — no state, no trait. Caller owns the
//! [`Provider`] handle (typically the gateway-wide resilient provider).
//!
//! Failure semantics: any provider error or timeout returns
//! `Ok(None)` (graceful degradation). The user gets their un-verified
//! answer; the warning lives in the tracing log. NEVER propagates as a
//! turn failure — verification is a quality-of-life feature, not a
//! correctness gate.

use std::time::Duration;

use anyhow::Result;
use tokio::time::timeout;

use crate::providers::{ChatMessage, ChatRequest, Provider};

const VERIFIER_SYSTEM_PROMPT: &str = "\
You are a fact-checking auditor. Below is a user question and the assistant's draft response.

Extract up to N verifiable factual claims from the draft (specific dates, statistics, URLs, \
proper nouns, numerical values, version numbers). For each claim emit ONE line:

  [OK] <claim>            — looks plausible/internally consistent
  [VERIFY] <claim>        — looks suspicious, contradicts common knowledge, or is unverifiable from context

Then on the last line emit one of:
  CONCLUSION: ALL_OK     — every claim is [OK]
  CONCLUSION: NEEDS_CHECK — at least one claim is [VERIFY]

If the draft contains zero verifiable factual claims (it's subjective, conversational, or pure code), \
emit a single line:
  NO_CLAIMS

Be conservative — flag any specific factual assertion you cannot independently verify. Do not flag \
opinions, code, or generic statements. Use the same language as the draft.";

const FOOTER_HEADER: &str = "\n\n---\n**Verification (auto):**\n";

/// Result of the verifier pass — either a footer markdown string to
/// append to the assistant response, or `None` (no verifiable claims
/// found, or all claims looked OK).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifierOutcome {
    NoFooter,
    Footer(String),
}

/// Run the verifier and produce an optional footer.
///
/// `provider` — typically the gateway-wide resilient provider (so we
/// inherit retries and rate-limit handling). `verifier_model` — the
/// model name; when `None` callers should pass the main `default_model`.
/// `max_claims` — caps the verifier's output length (passed into the
/// system prompt as `N`). `timeout` — hard wall on the LLM call;
/// exceeding it returns `Ok(VerifierOutcome::NoFooter)` with a warn log.
pub async fn run_grounding_verifier(
    provider: &dyn Provider,
    verifier_model: &str,
    user_question: &str,
    draft_response: &str,
    max_claims: usize,
    timeout_dur: Duration,
) -> Result<VerifierOutcome> {
    let system = VERIFIER_SYSTEM_PROMPT.replace("N", &max_claims.to_string());
    let user = format!(
        "USER QUESTION:\n{user_question}\n\nASSISTANT DRAFT:\n{draft_response}\n\nProduce the [OK]/[VERIFY] list now."
    );
    let messages = vec![ChatMessage::system(system), ChatMessage::user(user)];
    let request = ChatRequest {
        messages: &messages,
        tools: None,
    };

    let response_result = timeout(timeout_dur, provider.chat(request, verifier_model, 0.0)).await;
    let response = match response_result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "grounding verifier provider call failed; skipping footer");
            return Ok(VerifierOutcome::NoFooter);
        }
        Err(_) => {
            tracing::warn!(
                timeout_secs = timeout_dur.as_secs(),
                "grounding verifier timed out; skipping footer"
            );
            return Ok(VerifierOutcome::NoFooter);
        }
    };

    let text = response.text.unwrap_or_default();
    Ok(footer_from_verifier_text(&text))
}

/// Pure parser: given the verifier's raw response text, decide whether
/// to emit a footer and (if so) format it. Exposed for unit testing.
pub fn footer_from_verifier_text(verifier_text: &str) -> VerifierOutcome {
    let trimmed = verifier_text.trim();
    if trimmed.is_empty() || trimmed.contains("NO_CLAIMS") {
        return VerifierOutcome::NoFooter;
    }
    if trimmed.contains("CONCLUSION: ALL_OK") {
        return VerifierOutcome::NoFooter;
    }

    let mut flagged: Vec<&str> = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if let Some(claim) = line.strip_prefix("[VERIFY]") {
            let claim = claim.trim();
            if !claim.is_empty() {
                flagged.push(claim);
            }
        }
    }

    if flagged.is_empty() {
        return VerifierOutcome::NoFooter;
    }

    let mut footer = String::from(FOOTER_HEADER);
    for claim in flagged {
        footer.push_str("- ⚠️ ");
        footer.push_str(claim);
        footer.push('\n');
    }
    footer.push_str(
        "\n*Auto-generated by Chain-of-Verification. Treat flagged claims as low-confidence.*",
    );
    VerifierOutcome::Footer(footer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_returns_no_footer_for_no_claims() {
        let out = footer_from_verifier_text("NO_CLAIMS");
        assert_eq!(out, VerifierOutcome::NoFooter);
    }

    #[test]
    fn parser_returns_no_footer_for_all_ok() {
        let out =
            footer_from_verifier_text("[OK] Paris is the capital of France.\nCONCLUSION: ALL_OK");
        assert_eq!(out, VerifierOutcome::NoFooter);
    }

    #[test]
    fn parser_returns_no_footer_for_empty() {
        assert_eq!(footer_from_verifier_text(""), VerifierOutcome::NoFooter);
        assert_eq!(
            footer_from_verifier_text("   \n  "),
            VerifierOutcome::NoFooter
        );
    }

    #[test]
    fn parser_returns_footer_with_verified_claims() {
        let text = "\
[OK] HTTP/2 was published in 2015
[VERIFY] Rust 1.95 added const-generic specialization
[VERIFY] Tree-sitter version 0.30 ships next week
CONCLUSION: NEEDS_CHECK";
        match footer_from_verifier_text(text) {
            VerifierOutcome::Footer(f) => {
                assert!(f.starts_with("\n\n---\n**Verification (auto):**\n"));
                assert!(f.contains("Rust 1.95 added const-generic specialization"));
                assert!(f.contains("Tree-sitter version 0.30 ships next week"));
                assert!(f.contains("low-confidence"));
            }
            other => panic!("expected Footer, got {other:?}"),
        }
    }

    #[test]
    fn parser_strips_empty_verify_lines() {
        let text = "[VERIFY]\n[VERIFY] real claim\nCONCLUSION: NEEDS_CHECK";
        match footer_from_verifier_text(text) {
            VerifierOutcome::Footer(f) => {
                // Only one bullet — the empty [VERIFY] line is skipped.
                assert_eq!(f.matches("⚠️").count(), 1);
                assert!(f.contains("real claim"));
            }
            other => panic!("expected Footer, got {other:?}"),
        }
    }

    #[test]
    fn parser_handles_no_explicit_conclusion() {
        // Even without the CONCLUSION line, [VERIFY] lines still emit a footer.
        let text = "[VERIFY] suspicious claim";
        match footer_from_verifier_text(text) {
            VerifierOutcome::Footer(_) => {}
            other => panic!("expected Footer, got {other:?}"),
        }
    }
}
