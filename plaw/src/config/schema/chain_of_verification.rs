//! `[chain_of_verification]` config section — opt-in post-response
//! Chain-of-Verification (CoV) gate.
//!
//! When `enabled = true` AND `agent.intent_routing_enabled = true` AND
//! the HybridRouter classifies the user message as
//! `Intent::FactualLookup`, the agent loop runs a second LLM call after
//! producing its final response. The verifier extracts up to
//! `max_claims` factual claims, assesses each, and (if any look
//! suspicious) appends a `[Verification]` markdown footer to the
//! assistant reply BEFORE token streaming begins.
//!
//! Default off. Validate with a baseline-vs-CoV eval suite before
//! flipping. Cost: roughly one extra LLM call per factual-lookup turn,
//! capped by `max_claims` output tokens.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the Chain-of-Verification post-response gate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainOfVerificationConfig {
    /// Master switch. `false` skips the verifier entirely regardless of
    /// intent_routing state.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum number of claims the verifier is asked to extract and
    /// assess. Caps verifier output length. Default 5 — reasonable for
    /// typical chat answers; raise for longform factual content.
    #[serde(default = "default_max_claims")]
    pub max_claims: usize,
    /// Optional override: use a different (typically cheaper) model
    /// for verification. When `None`, reuses the main `default_model`.
    /// The verifier doesn't need the strongest model — it just needs
    /// to be calibrated about uncertainty.
    #[serde(default)]
    pub verifier_model: Option<String>,
    /// Hard timeout on the verifier call, in seconds. When exceeded
    /// the verifier degrades gracefully (no footer, log warning) so
    /// it never blocks a turn from completing.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_max_claims() -> usize {
    5
}

fn default_timeout_secs() -> u64 {
    30
}

impl Default for ChainOfVerificationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_claims: default_max_claims(),
            verifier_model: None,
            timeout_secs: default_timeout_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_disabled_with_sensible_max_claims() {
        let cfg = ChainOfVerificationConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.max_claims, 5);
        assert!(cfg.verifier_model.is_none());
        assert_eq!(cfg.timeout_secs, 30);
    }

    #[test]
    fn toml_partial_fills_defaults() {
        let parsed: ChainOfVerificationConfig =
            toml::from_str("enabled = true\n").expect("toml parse");
        assert!(parsed.enabled);
        assert_eq!(parsed.max_claims, 5);
        assert_eq!(parsed.timeout_secs, 30);
    }

    #[test]
    fn toml_full_round_trip() {
        let src = r#"
enabled = true
max_claims = 3
verifier_model = "deepseek-v4-pro"
timeout_secs = 60
"#;
        let parsed: ChainOfVerificationConfig = toml::from_str(src).expect("toml parse");
        assert!(parsed.enabled);
        assert_eq!(parsed.max_claims, 3);
        assert_eq!(parsed.verifier_model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(parsed.timeout_secs, 60);
    }
}
