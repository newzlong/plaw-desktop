//! Judge LLM clients — abstracts the difference between Anthropic
//! Messages, OpenAI-compat Chat Completions, and Kimi (which speaks
//! Anthropic-compat at api.moonshot.cn).
//!
//! All clients implement [`JudgeClient`] so the rest of the eval system
//! can talk to any of them through a single trait object.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Family the judge belongs to. Used by `Jury` to enforce cross-family
/// composition — the same family can't be used to grade itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JudgeFamily {
    Anthropic,
    OpenAi,
    Kimi,
    DeepSeek,
    Qwen,
    Other,
}

impl JudgeFamily {
    pub fn from_provider(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => JudgeFamily::Anthropic,
            "openai" | "gpt" => JudgeFamily::OpenAi,
            "kimi" | "moonshot" => JudgeFamily::Kimi,
            "deepseek" => JudgeFamily::DeepSeek,
            "qwen" | "alibaba" | "tongyi" => JudgeFamily::Qwen,
            _ => JudgeFamily::Other,
        }
    }
}

/// Result of a single judge completion call.
#[derive(Debug, Clone)]
pub struct JudgeCompletion {
    pub text: String,
    pub family: JudgeFamily,
    pub model: String,
}

/// What every judge backend must do: take a `system + user` prompt and
/// return the text the model wrote, plus identifying metadata.
#[async_trait]
pub trait JudgeClient: Send + Sync {
    fn family(&self) -> JudgeFamily;
    fn model(&self) -> &str;
    async fn complete(&self, system: &str, user: &str) -> Result<JudgeCompletion>;
}

/// Default per-call timeout if `with_timeout` isn't set.
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(120);

// ---------- OpenAI-compatible client (used for OpenAI, Kimi via OpenAI mode, Qwen, DeepSeek) ----------

/// Generic OpenAI-compatible Chat Completions client. Many vendors expose
/// this protocol under a different `base_url` (`api.openai.com/v1`,
/// `api.moonshot.cn/v1`, `dashscope-intl.aliyuncs.com/...`).
pub struct OpenAiCompatClient {
    pub family: JudgeFamily,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub temperature: f32,
    pub timeout: Duration,
    http: Client,
}

impl OpenAiCompatClient {
    pub fn new(
        family: JudgeFamily,
        model: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        temperature: f32,
    ) -> Self {
        Self {
            family,
            model: model.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            temperature,
            timeout: DEFAULT_HTTP_TIMEOUT,
            http: Client::builder()
                .timeout(DEFAULT_HTTP_TIMEOUT)
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self.http = Client::builder()
            .timeout(t)
            .build()
            .expect("reqwest client");
        self
    }
}

#[async_trait]
impl JudgeClient for OpenAiCompatClient {
    fn family(&self) -> JudgeFamily {
        self.family
    }
    fn model(&self) -> &str {
        &self.model
    }
    async fn complete(&self, system: &str, user: &str) -> Result<JudgeCompletion> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let body = serde_json::json!({
            "model": self.model,
            "temperature": self.temperature,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user",   "content": user   },
            ],
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("posting to OpenAI-compat endpoint")?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("judge HTTP {status}: {raw}"));
        }
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&raw).with_context(|| format!("decoding response: {raw}"))?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        Ok(JudgeCompletion {
            text,
            family: self.family,
            model: self.model.clone(),
        })
    }
}

// ---------- Anthropic Messages client ----------

/// Anthropic Messages API. Plaw's Kimi configuration uses the
/// Anthropic-compatible variant (`api.moonshot.cn`); use this client for
/// that path too — set `family = Kimi` accordingly.
pub struct AnthropicClient {
    pub family: JudgeFamily,
    pub model: String,
    pub base_url: String, // e.g. https://api.anthropic.com or https://api.moonshot.cn
    pub api_key: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout: Duration,
    http: Client,
}

impl AnthropicClient {
    pub fn new(
        family: JudgeFamily,
        model: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        temperature: f32,
    ) -> Self {
        Self {
            family,
            model: model.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            temperature,
            max_tokens: 1024,
            timeout: DEFAULT_HTTP_TIMEOUT,
            http: Client::builder()
                .timeout(DEFAULT_HTTP_TIMEOUT)
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self.http = Client::builder()
            .timeout(t)
            .build()
            .expect("reqwest client");
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
}

#[async_trait]
impl JudgeClient for AnthropicClient {
    fn family(&self) -> JudgeFamily {
        self.family
    }
    fn model(&self) -> &str {
        &self.model
    }
    async fn complete(&self, system: &str, user: &str) -> Result<JudgeCompletion> {
        let url = format!(
            "{}/v1/messages",
            self.base_url.trim_end_matches('/')
        );
        let body = serde_json::json!({
            "model": self.model,
            "system": system,
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
            "messages": [
                { "role": "user", "content": user }
            ],
        });
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("posting to Anthropic Messages endpoint")?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("judge HTTP {status}: {raw}"));
        }
        let parsed: AnthropicResponse =
            serde_json::from_str(&raw).with_context(|| format!("decoding response: {raw}"))?;
        let text = parsed
            .content
            .into_iter()
            .filter_map(|c| match c {
                AnthropicContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        Ok(JudgeCompletion {
            text,
            family: self.family,
            model: self.model.clone(),
        })
    }
}

// ---------- Wire types ----------

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text { text: String },
    #[serde(other)]
    Other,
}

// ---------- Mock client for tests ----------

/// Deterministic in-process judge that returns canned responses keyed by
/// (system, user) prefix. Used by tests to avoid hitting real APIs.
pub struct MockJudgeClient {
    pub family: JudgeFamily,
    pub model: String,
    pub responses: Vec<String>,
    counter: std::sync::atomic::AtomicUsize,
}

impl MockJudgeClient {
    pub fn new(family: JudgeFamily, model: &str, responses: Vec<String>) -> Self {
        Self {
            family,
            model: model.into(),
            responses,
            counter: 0.into(),
        }
    }
}

#[async_trait]
impl JudgeClient for MockJudgeClient {
    fn family(&self) -> JudgeFamily {
        self.family
    }
    fn model(&self) -> &str {
        &self.model
    }
    async fn complete(&self, _system: &str, _user: &str) -> Result<JudgeCompletion> {
        let i = self.counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let text = self
            .responses
            .get(i % self.responses.len())
            .cloned()
            .unwrap_or_default();
        Ok(JudgeCompletion {
            text,
            family: self.family,
            model: self.model.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_resolution_is_case_insensitive() {
        assert_eq!(JudgeFamily::from_provider("anthropic"), JudgeFamily::Anthropic);
        assert_eq!(JudgeFamily::from_provider("Claude"), JudgeFamily::Anthropic);
        assert_eq!(JudgeFamily::from_provider("OPENAI"), JudgeFamily::OpenAi);
        assert_eq!(JudgeFamily::from_provider("kimi"), JudgeFamily::Kimi);
        assert_eq!(JudgeFamily::from_provider("Moonshot"), JudgeFamily::Kimi);
        assert_eq!(JudgeFamily::from_provider("Qwen"), JudgeFamily::Qwen);
        assert_eq!(JudgeFamily::from_provider("custom"), JudgeFamily::Other);
    }

    #[tokio::test]
    async fn mock_client_rotates_through_responses() {
        let mock = MockJudgeClient::new(
            JudgeFamily::Kimi,
            "kimi-k2.5",
            vec!["A".into(), "B".into()],
        );
        let r1 = mock.complete("sys", "u").await.unwrap();
        let r2 = mock.complete("sys", "u").await.unwrap();
        let r3 = mock.complete("sys", "u").await.unwrap();
        assert_eq!(r1.text, "A");
        assert_eq!(r2.text, "B");
        assert_eq!(r3.text, "A"); // wraps
    }
}
