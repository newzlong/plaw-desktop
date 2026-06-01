//! Built-in [`HookHandler`] implementing Chain-of-Verification.
//!
//! Registered in `gateway::mod` when `[chain_of_verification].enabled = true`.
//! Reads the classified intent from the [`crate::agent::loop_::current_turn_intent`]
//! task-local set by the gateway before each agent loop run. Calls
//! [`crate::agent::grounding_verifier::run_grounding_verifier`] only when
//! `intent == Some(Intent::FactualLookup)` — all other intents short-circuit
//! to `Continue(text)` unchanged.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::agent::grounding_verifier::{run_grounding_verifier, VerifierOutcome};
use crate::agent::intent::Intent;
use crate::agent::loop_::current_turn_intent;
use crate::hooks::traits::{HookHandler, HookResult};
use crate::providers::{ChatMessage, Provider};

/// Hook handler that runs Chain-of-Verification on `Intent::FactualLookup`
/// turns. Constructed by the gateway when the feature is enabled in config.
pub struct ChainOfVerificationHook {
    provider: Arc<dyn Provider>,
    verifier_model: String,
    max_claims: usize,
    timeout: Duration,
}

impl ChainOfVerificationHook {
    pub fn new(
        provider: Arc<dyn Provider>,
        verifier_model: impl Into<String>,
        max_claims: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            provider,
            verifier_model: verifier_model.into(),
            max_claims,
            timeout,
        }
    }

    /// Extract the user's most recent user-role message from history.
    /// Returns the empty string if no user message is present (verifier
    /// degrades gracefully without crashing).
    fn last_user_message(history: &[ChatMessage]) -> String {
        history
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl HookHandler for ChainOfVerificationHook {
    fn name(&self) -> &str {
        "chain_of_verification"
    }

    fn priority(&self) -> i32 {
        100
    }

    async fn after_final_response(
        &self,
        text: String,
        history: &[ChatMessage],
    ) -> HookResult<String> {
        // Intent gate. When intent_routing is disabled or the classifier
        // returned anything other than FactualLookup, return early — no
        // verifier call, no extra latency.
        match current_turn_intent() {
            Some(Intent::FactualLookup) => {}
            _ => return HookResult::Continue(text),
        }
        if text.trim().is_empty() {
            return HookResult::Continue(text);
        }

        let user_q = Self::last_user_message(history);
        let outcome = run_grounding_verifier(
            self.provider.as_ref(),
            &self.verifier_model,
            &user_q,
            &text,
            self.max_claims,
            self.timeout,
        )
        .await;

        match outcome {
            Ok(VerifierOutcome::NoFooter) => HookResult::Continue(text),
            Ok(VerifierOutcome::Footer(footer)) => {
                tracing::info!(
                    footer_chars = footer.len(),
                    "chain_of_verification appended footer to final response"
                );
                let mut combined = text;
                combined.push_str(&footer);
                HookResult::Continue(combined)
            }
            Err(e) => {
                // run_grounding_verifier returns Ok on graceful-degradation
                // paths (timeout, provider error), so an Err here is truly
                // unexpected. Still degrade gracefully — return original.
                tracing::warn!(error = %e, "chain_of_verification unexpected error; returning original text");
                HookResult::Continue(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_::with_turn_intent;
    use crate::providers::traits::{ChatRequest, ChatResponse};

    /// Mock provider that records its model+text responses, used to
    /// verify the hook routes / skips correctly without a real LLM.
    struct MockProvider {
        canned_response: String,
        call_count: std::sync::Mutex<usize>,
    }

    impl MockProvider {
        fn new(canned_response: &str) -> Self {
            Self {
                canned_response: canned_response.to_string(),
                call_count: std::sync::Mutex::new(0),
            }
        }
        fn calls(&self) -> usize {
            *self.call_count.lock().unwrap()
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            *self.call_count.lock().unwrap() += 1;
            Ok(self.canned_response.clone())
        }
        async fn chat(
            &self,
            _request: ChatRequest<'_>,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<ChatResponse> {
            *self.call_count.lock().unwrap() += 1;
            Ok(ChatResponse {
                text: Some(self.canned_response.clone()),
                tool_calls: Vec::new(),
                usage: None,
                reasoning_content: None,
            })
        }
    }

    fn hook_with(provider: Arc<dyn Provider>) -> ChainOfVerificationHook {
        ChainOfVerificationHook::new(provider, "mock-model", 5, Duration::from_secs(5))
    }

    #[tokio::test]
    async fn skips_verifier_when_intent_missing() {
        let mock = Arc::new(MockProvider::new("[VERIFY] some claim"));
        let hook = hook_with(mock.clone());
        let history = vec![ChatMessage::user("question")];
        // No with_turn_intent scope — task_local returns None
        let out = hook
            .after_final_response("draft answer".into(), &history)
            .await;
        match out {
            HookResult::Continue(t) => assert_eq!(t, "draft answer"),
            HookResult::Cancel(_) => panic!("should not cancel"),
        }
        assert_eq!(
            mock.calls(),
            0,
            "verifier must not run without FactualLookup intent"
        );
    }

    #[tokio::test]
    async fn skips_verifier_when_intent_not_factual_lookup() {
        let mock = Arc::new(MockProvider::new("[VERIFY] some claim"));
        let hook = hook_with(mock.clone());
        let history = vec![ChatMessage::user("question")];
        let result = with_turn_intent(Some(Intent::TaskRequest), async {
            hook.after_final_response("draft answer".into(), &history)
                .await
        })
        .await;
        match result {
            HookResult::Continue(t) => assert_eq!(t, "draft answer"),
            HookResult::Cancel(_) => panic!("should not cancel"),
        }
        assert_eq!(mock.calls(), 0);
    }

    #[tokio::test]
    async fn runs_verifier_on_factual_lookup_and_appends_footer() {
        let canned = "\
[VERIFY] HTTP/2 was published in 2018
CONCLUSION: NEEDS_CHECK";
        let mock = Arc::new(MockProvider::new(canned));
        let hook = hook_with(mock.clone());
        let history = vec![ChatMessage::user("When was HTTP/2 published?")];
        let result = with_turn_intent(Some(Intent::FactualLookup), async {
            hook.after_final_response("HTTP/2 was published in 2018.".into(), &history)
                .await
        })
        .await;
        match result {
            HookResult::Continue(t) => {
                assert!(t.contains("HTTP/2 was published in 2018."));
                assert!(t.contains("**Verification (auto):**"));
                assert!(t.contains("HTTP/2 was published in 2018"));
            }
            HookResult::Cancel(_) => panic!("should not cancel"),
        }
        assert_eq!(mock.calls(), 1);
    }

    #[tokio::test]
    async fn skips_footer_when_verifier_says_all_ok() {
        let mock = Arc::new(MockProvider::new("[OK] foo\nCONCLUSION: ALL_OK"));
        let hook = hook_with(mock.clone());
        let history = vec![ChatMessage::user("question")];
        let result = with_turn_intent(Some(Intent::FactualLookup), async {
            hook.after_final_response("answer".into(), &history).await
        })
        .await;
        match result {
            HookResult::Continue(t) => assert_eq!(t, "answer"),
            HookResult::Cancel(_) => panic!("should not cancel"),
        }
        assert_eq!(mock.calls(), 1);
    }

    #[tokio::test]
    async fn skips_verifier_when_draft_is_empty() {
        let mock = Arc::new(MockProvider::new("[VERIFY] claim"));
        let hook = hook_with(mock.clone());
        let history = vec![ChatMessage::user("question")];
        let result = with_turn_intent(Some(Intent::FactualLookup), async {
            hook.after_final_response("   ".into(), &history).await
        })
        .await;
        match result {
            HookResult::Continue(t) => assert_eq!(t, "   "),
            HookResult::Cancel(_) => panic!("should not cancel"),
        }
        assert_eq!(mock.calls(), 0, "empty draft must not invoke verifier");
    }
}
