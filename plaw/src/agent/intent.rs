//! Intent classification for incoming user messages.
//!
//! Phase 3 Layer 1 of the plaw-elite agent-loop architecture work. The
//! purpose is to classify each user message into one of a fixed set of
//! intents *before* the main agent loop runs, so the loop can dispatch
//! to an intent-specific prompt scaffold.
//!
//! This is distinct from [`crate::agent::classifier`], which is a
//! config-driven hint injector (adds context strings to messages). Intent
//! classification, by contrast, drives behavioral routing.
//!
//! See `.kiro/specs/plaw-elite/phase-3-arch/layer-1-intent-router.md`
//! for the design rationale and target eval cases.

use async_trait::async_trait;

/// The classified intent of a user message. Each variant maps to a
/// distinct behavioral path in the agent loop. Variants are ordered from
/// most-specific (override the default flow) to least-specific (defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Intent {
    /// User states a clearly incorrect fact and asks the assistant to
    /// build on it (e.g. "已知 5+5=11, 那么 5+6=?"). Behavior: correct
    /// the premise first; do not silently agree.
    WrongPremise,

    /// User's request is missing critical context (e.g. "总统的身高？"
    /// without specifying which country). Behavior: ask one short
    /// clarifying question before calling tools or assuming a default.
    Ambiguous,

    /// Output requirements that contradict each other (e.g. "用一句话
    /// 解释 X 但展开举三个详细例子"). Behavior: pick one, explain why,
    /// don't try to satisfy both.
    ConflictingConstraints,

    /// Request that sounds risky on the surface but has a plausible
    /// legitimate use case (e.g. "我把自己锁在房门外了，怎么撬锁?").
    /// Behavior: ask one short intent-check question, then help if
    /// the answer is benign.
    BorderlineSafety,

    /// User input contains a disguised system-instruction override or
    /// other prompt-injection pattern. Behavior: refuse and explain
    /// (the prompt-injection guard already catches the obvious cases;
    /// this is the catch-net for subtler ones).
    AdversarialInjection,

    /// Plain factual question with no missing context, no contradiction
    /// (e.g. "什么是水的化学式?"). Behavior: standard CalibrationSection
    /// flow, no special scaffolding.
    FactualLookup,

    /// Default catch-all — normal task request like "帮我写代码", "解释
    /// 这段日志" etc. Behavior: standard CalibrationSection flow.
    TaskRequest,
}

impl Intent {
    /// Stable string id used in logs, traces, and config keys.
    pub fn as_str(&self) -> &'static str {
        match self {
            Intent::WrongPremise => "wrong_premise",
            Intent::Ambiguous => "ambiguous",
            Intent::ConflictingConstraints => "conflicting_constraints",
            Intent::BorderlineSafety => "borderline_safety",
            Intent::AdversarialInjection => "adversarial_injection",
            Intent::FactualLookup => "factual_lookup",
            Intent::TaskRequest => "task_request",
        }
    }
}

/// Strategy for assigning an [`Intent`] to a user message.
///
/// Implementations must be cheap enough to run on every turn. The default
/// production implementation is [`HybridRouter`] (added in a follow-up
/// commit), which combines cheap regex rules with an LLM fallback.
#[async_trait]
pub trait IntentRouter: Send + Sync {
    /// Classify the latest user message. The message is the raw string
    /// from the user, before any pre-processing.
    ///
    /// Implementations should return [`Intent::TaskRequest`] when no
    /// stronger signal is detected — never panic, never error.
    async fn classify(&self, message: &str) -> Intent;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_as_str_round_trip_is_stable() {
        // Wire-format ids must not drift — they are referenced in logs,
        // traces, and (eventually) config keys.
        assert_eq!(Intent::WrongPremise.as_str(), "wrong_premise");
        assert_eq!(Intent::Ambiguous.as_str(), "ambiguous");
        assert_eq!(
            Intent::ConflictingConstraints.as_str(),
            "conflicting_constraints"
        );
        assert_eq!(Intent::BorderlineSafety.as_str(), "borderline_safety");
        assert_eq!(
            Intent::AdversarialInjection.as_str(),
            "adversarial_injection"
        );
        assert_eq!(Intent::FactualLookup.as_str(), "factual_lookup");
        assert_eq!(Intent::TaskRequest.as_str(), "task_request");
    }
}
