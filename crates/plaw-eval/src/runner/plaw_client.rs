//! WebSocket client for talking to a running plaw instance.
//!
//! Plaw's gateway exposes `ws://127.0.0.1:{port}/ws/chat`. The protocol is
//! described in `plaw-desktop/CLAUDE.md`:
//!
//! Frontend → plaw:
//! ```json
//! {"type": "message", "content": "..."}
//! {"type": "cancel"}
//! ```
//!
//! Plaw → frontend (one event per WS frame, JSON):
//! - `chunk`        — `content: string` (streaming text delta)
//! - `thinking`     — `content: string` (reasoning summary)
//! - `tool_call`    — `name: string, args: object`
//! - `tool_result`  — `name: string, output: string`
//! - `done`         — `full_response: string, usage: object`
//! - `error`        — `message: string`

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

use crate::suite::{CaseInput, ChatRole};

/// Default per-request timeout. Override via `PlawClient::with_timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Configuration for a single plaw connection.
#[derive(Debug, Clone)]
pub struct PlawClient {
    pub ws_url: String,
    pub bearer: Option<String>,
    pub request_timeout: Duration,
}

impl PlawClient {
    pub fn new(ws_url: impl Into<String>) -> Self {
        Self {
            ws_url: ws_url.into(),
            bearer: None,
            request_timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_bearer(mut self, bearer: impl Into<String>) -> Self {
        self.bearer = Some(bearer.into());
        self
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.request_timeout = t;
        self
    }

    /// Send a single eval-case input and collect plaw's response. The call
    /// drives one full WebSocket session: connect → send → consume events
    /// until `done` / `error` / timeout / disconnect.
    pub async fn send(&self, input: &CaseInput) -> Result<PlawResponse> {
        let payload = render_payload(input)?;
        timeout(self.request_timeout, self.run_session(&payload))
            .await
            .map_err(|_| anyhow!("plaw response timed out after {:?}", self.request_timeout))?
    }

    async fn run_session(&self, payload: &str) -> Result<PlawResponse> {
        let request = self
            .build_request()
            .with_context(|| format!("building WS request for {}", self.ws_url))?;
        let (mut ws, _) = connect_async(request)
            .await
            .with_context(|| format!("connecting to {}", self.ws_url))?;
        let started = Instant::now();
        ws.send(Message::Text(payload.to_string()))
            .await
            .context("sending plaw request frame")?;

        let mut full_response = String::new();
        let mut tool_calls: Vec<ToolCallEvent> = Vec::new();
        let mut tool_results: Vec<ToolResultEvent> = Vec::new();
        let mut thinking: Vec<String> = Vec::new();
        let mut usage = Usage::default();

        while let Some(frame) = ws.next().await {
            let msg = frame.context("receiving WS frame from plaw")?;
            match msg {
                Message::Text(text) => match serde_json::from_str::<PlawEvent>(&text) {
                    Ok(PlawEvent::Chunk { content }) => full_response.push_str(&content),
                    Ok(PlawEvent::Thinking { content }) => thinking.push(content),
                    Ok(PlawEvent::ToolCall { name, args }) => tool_calls.push(ToolCallEvent {
                        name,
                        args,
                    }),
                    Ok(PlawEvent::ToolResult { name, output }) => {
                        tool_results.push(ToolResultEvent { name, output })
                    }
                    Ok(PlawEvent::Done {
                        full_response: fr,
                        usage: u,
                    }) => {
                        if !fr.is_empty() {
                            full_response = fr;
                        }
                        if let Some(u) = u {
                            usage = u;
                        }
                        let _ = ws.close(None).await;
                        return Ok(PlawResponse {
                            text: full_response,
                            tool_calls,
                            tool_results,
                            thinking,
                            usage,
                            latency_ms: started.elapsed().as_millis() as u64,
                        });
                    }
                    Ok(PlawEvent::Error { message }) => {
                        let _ = ws.close(None).await;
                        return Err(anyhow!("plaw error: {message}"));
                    }
                    Ok(PlawEvent::Other) => {} // unknown event types are ignored
                    Err(e) => tracing::debug!(?e, raw = %text, "could not parse plaw event"),
                },
                Message::Close(_) => break,
                Message::Ping(p) => {
                    let _ = ws.send(Message::Pong(p)).await;
                }
                _ => {}
            }
        }
        Err(anyhow!(
            "plaw WS closed before emitting `done` event (collected {} chars)",
            full_response.len()
        ))
    }

    fn build_request(&self) -> Result<Request<()>> {
        let mut req = self
            .ws_url
            .as_str()
            .into_client_request()
            .context("invalid WS URL")?;
        if let Some(bearer) = &self.bearer {
            let header_value = format!("Bearer {bearer}").parse()
                .context("bearer token contains invalid header characters")?;
            req.headers_mut().insert("authorization", header_value);
        }
        Ok(req)
    }
}

/// Aggregated outcome of a single plaw run.
#[derive(Debug, Clone, Default)]
pub struct PlawResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCallEvent>,
    pub tool_results: Vec<ToolResultEvent>,
    pub thinking: Vec<String>,
    pub usage: Usage,
    pub latency_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolResultEvent {
    pub name: String,
    pub output: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
}

/// Plaw → eval events, untagged so unknown variants degrade to `Other`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PlawEvent {
    Chunk {
        #[serde(default)]
        content: String,
    },
    Thinking {
        #[serde(default)]
        content: String,
    },
    ToolCall {
        name: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    ToolResult {
        name: String,
        #[serde(default)]
        output: String,
    },
    Done {
        #[serde(default)]
        full_response: String,
        #[serde(default)]
        usage: Option<Usage>,
    },
    Error {
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Other,
}

/// Render a `CaseInput` into the JSON payload plaw expects on its WS.
fn render_payload(input: &CaseInput) -> Result<String> {
    let content = match input {
        CaseInput::Chat { messages } => {
            let mut s = String::new();
            for m in messages {
                if !s.is_empty() {
                    s.push_str("\n\n");
                }
                let role = match m.role {
                    ChatRole::System => "[system] ",
                    ChatRole::User => "",
                    ChatRole::Assistant => "[assistant] ",
                };
                s.push_str(role);
                s.push_str(&m.content);
            }
            s
        }
        CaseInput::Agent { task, .. } => task.clone(),
        CaseInput::Rag { question, .. } => question.clone(),
    };
    let payload = serde_json::json!({"type": "message", "content": content});
    Ok(payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suite::ChatMsg;

    #[test]
    fn renders_chat_payload() {
        let input = CaseInput::Chat {
            messages: vec![
                ChatMsg {
                    role: ChatRole::System,
                    content: "Be brief.".into(),
                },
                ChatMsg {
                    role: ChatRole::User,
                    content: "Hi".into(),
                },
            ],
        };
        let payload = render_payload(&input).unwrap();
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["type"], "message");
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("[system] Be brief."));
        assert!(content.contains("Hi"));
    }

    #[test]
    fn renders_agent_payload() {
        let input = CaseInput::Agent {
            task: "list files".into(),
            max_steps: 5,
        };
        let payload = render_payload(&input).unwrap();
        assert!(payload.contains("list files"));
    }

    #[test]
    fn parses_known_events_and_falls_back_for_unknown() {
        let chunk: PlawEvent = serde_json::from_str(r#"{"type":"chunk","content":"hi"}"#).unwrap();
        assert!(matches!(chunk, PlawEvent::Chunk { .. }));

        let unk: PlawEvent =
            serde_json::from_str(r#"{"type":"undocumented_future_event","x":1}"#).unwrap();
        assert!(matches!(unk, PlawEvent::Other));
    }

    #[test]
    fn done_event_with_usage_parses() {
        let raw = r#"{"type":"done","full_response":"abc",
                      "usage":{"input_tokens":12,"output_tokens":3}}"#;
        let ev: PlawEvent = serde_json::from_str(raw).unwrap();
        match ev {
            PlawEvent::Done { full_response, usage } => {
                assert_eq!(full_response, "abc");
                let u = usage.unwrap();
                assert_eq!(u.input_tokens, 12);
                assert_eq!(u.output_tokens, 3);
            }
            _ => panic!("expected done"),
        }
    }
}
