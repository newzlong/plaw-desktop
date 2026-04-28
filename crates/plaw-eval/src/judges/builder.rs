//! Build a [`JudgeClient`] from a [`JudgeSpec`] declared in suite TOML.
//!
//! Maps `provider` → wire protocol + default base URL. API keys are
//! sourced from environment variables; missing keys produce an error
//! pointing at which env var to set.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use crate::judges::client::{AnthropicClient, JudgeClient, JudgeFamily, OpenAiCompatClient};
use crate::suite::JudgeSpec;

/// Convert a suite-level judge spec into a runtime-ready client. Picks
/// the wire protocol from the provider name (Anthropic Messages vs
/// OpenAI Chat Completions) and reads the API key from the environment.
pub fn build_from_spec(spec: &JudgeSpec) -> Result<Arc<dyn JudgeClient>> {
    let provider = spec.provider.to_ascii_lowercase();
    let family = JudgeFamily::from_provider(&provider);
    let key_env = api_key_env_var(&provider);
    let api_key = std::env::var(key_env).with_context(|| {
        format!(
            "{key_env} not set — required to build judge for provider '{}'",
            spec.provider
        )
    })?;

    match provider.as_str() {
        "anthropic" | "claude" => {
            let base_url = std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".into());
            Ok(Arc::new(AnthropicClient::new(
                family,
                spec.model.clone(),
                base_url,
                api_key,
                spec.temperature,
            )))
        }
        "kimi" | "moonshot" => {
            // Default to the Anthropic-compatible Kimi endpoint, matching
            // plaw's own provider config (CLAUDE.md).
            let base_url =
                std::env::var("KIMI_BASE_URL").unwrap_or_else(|_| "https://api.moonshot.cn".into());
            Ok(Arc::new(AnthropicClient::new(
                family,
                spec.model.clone(),
                base_url,
                api_key,
                spec.temperature,
            )))
        }
        "kimi-coder" | "kimi_coder" | "kimicoder" => {
            // Kimi Coder API — what plaw-desktop uses by default with
            // model `k2p5`. Anthropic-compatible wire protocol.
            let base_url = std::env::var("KIMI_CODER_BASE_URL")
                .unwrap_or_else(|_| "https://api.kimi.com/coding".into());
            Ok(Arc::new(AnthropicClient::new(
                family,
                spec.model.clone(),
                base_url,
                api_key,
                spec.temperature,
            )))
        }
        "openai" | "gpt" => {
            let base_url = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into());
            Ok(Arc::new(OpenAiCompatClient::new(
                family,
                spec.model.clone(),
                base_url,
                api_key,
                spec.temperature,
            )))
        }
        "deepseek" => {
            let base_url = std::env::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| "https://api.deepseek.com".into());
            Ok(Arc::new(OpenAiCompatClient::new(
                family,
                spec.model.clone(),
                base_url,
                api_key,
                spec.temperature,
            )))
        }
        "qwen" | "alibaba" | "tongyi" => {
            let base_url = std::env::var("DASHSCOPE_BASE_URL")
                .unwrap_or_else(|_| "https://dashscope.aliyuncs.com/compatible-mode".into());
            Ok(Arc::new(OpenAiCompatClient::new(
                family,
                spec.model.clone(),
                base_url,
                api_key,
                spec.temperature,
            )))
        }
        other => Err(anyhow!(
            "unknown judge provider '{other}'. Supported: anthropic, openai, kimi, kimi-coder, deepseek, qwen"
        )),
    }
}

/// Map a provider name to the env var holding its API key. Used by the
/// CLI's doctor subcommand to surface missing keys.
pub fn api_key_env_var(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => "ANTHROPIC_API_KEY",
        "openai" | "gpt" => "OPENAI_API_KEY",
        "kimi" | "moonshot" => "KIMI_API_KEY",
        "kimi-coder" | "kimi_coder" | "kimicoder" => "KIMI_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "qwen" | "alibaba" | "tongyi" => "DASHSCOPE_API_KEY",
        _ => "PLAW_EVAL_API_KEY",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suite::JudgeMode;

    fn spec(model: &str, provider: &str) -> JudgeSpec {
        JudgeSpec {
            model: model.into(),
            provider: provider.into(),
            temperature: 0.0,
            mode: JudgeMode::default(),
        }
    }

    #[test]
    fn missing_api_key_returns_helpful_error() {
        // SAFETY: removing an env var inside a test is sound on this thread,
        // and other tests avoid touching this var.
        unsafe {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
        let result = build_from_spec(&spec("ds-r1", "deepseek"));
        let err = match result {
            Ok(_) => panic!("expected error when DEEPSEEK_API_KEY missing"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("DEEPSEEK_API_KEY"));
    }

    #[test]
    fn unknown_provider_error() {
        unsafe {
            std::env::set_var("PLAW_EVAL_API_KEY", "x");
        }
        let result = build_from_spec(&spec("m1", "made-up"));
        let err = match result {
            Ok(_) => panic!("expected error for unknown provider"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown judge provider") || msg.contains("PLAW_EVAL_API_KEY"));
    }

    #[test]
    fn api_key_env_var_mapping() {
        assert_eq!(api_key_env_var("anthropic"), "ANTHROPIC_API_KEY");
        assert_eq!(api_key_env_var("Claude"), "ANTHROPIC_API_KEY");
        assert_eq!(api_key_env_var("OpenAI"), "OPENAI_API_KEY");
        assert_eq!(api_key_env_var("kimi"), "KIMI_API_KEY");
        assert_eq!(api_key_env_var("moonshot"), "KIMI_API_KEY");
        assert_eq!(api_key_env_var("deepseek"), "DEEPSEEK_API_KEY");
        assert_eq!(api_key_env_var("Qwen"), "DASHSCOPE_API_KEY");
        assert_eq!(api_key_env_var("custom"), "PLAW_EVAL_API_KEY");
    }
}
