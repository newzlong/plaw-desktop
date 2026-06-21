//! Shared OpenAI Chat Completions SSE stream accumulator.
//!
//! Folds a `data: {...}` Server-Sent-Events byte stream — the standard OpenAI
//! `/chat/completions` streaming wire format — into one [`ChatResponse`]. Every
//! provider that speaks this format (openai, openrouter, glm, telnyx, copilot,
//! ...) only has to build its request and feed the response bytes here; the line
//! buffering, `[DONE]` handling, token forwarding, and tool-call fragment
//! reassembly live in one place.

use super::traits::{ChatResponse, TokenUsage, ToolCall};
use serde::Deserialize;
use std::collections::BTreeMap;
use tokio::sync::mpsc;

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<StreamUsage>,
}

#[derive(Debug, Deserialize)]
struct StreamUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    /// Tool calls stream as incremental fragments keyed by `index`; each may
    /// carry the id (once), the function name (once), and a partial `arguments`
    /// string. Consumers accumulate across fragments.
    #[serde(default)]
    tool_calls: Option<Vec<StreamToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct StreamToolCallDelta {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct StreamFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

/// Folds an OpenAI Chat Completions SSE byte stream into one [`ChatResponse`].
///
/// Feed raw response bytes (decoded to text) via [`Self::process_chunk`] as they
/// arrive; text deltas are forwarded to `on_token` (`try_send`, drop-on-full per
/// the `Provider::chat_streaming` contract) and tool-call fragments reassembled
/// by `index`. Call [`Self::finish`] once the stream ends to flush a trailing
/// line lacking a terminating newline, then [`Self::finalize`].
#[derive(Default)]
pub(crate) struct SseAccumulator {
    buffer: String,
    text: String,
    reasoning: String,
    tool_calls: BTreeMap<u32, ToolCallBuilder>,
    usage: Option<TokenUsage>,
    done: bool,
}

impl SseAccumulator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn process_chunk(&mut self, text: &str, on_token: Option<&mpsc::Sender<String>>) {
        self.buffer.push_str(text);
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..=pos].to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            self.process_line(&line, on_token);
        }
    }

    /// Flush a trailing line that lacked a terminating newline (some servers
    /// omit `\n` after the final `data: [DONE]`). Call once after the stream
    /// ends, before [`Self::finalize`].
    pub(crate) fn finish(&mut self, on_token: Option<&mpsc::Sender<String>>) {
        if !self.buffer.trim().is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.process_line(&line, on_token);
        }
    }

    fn process_line(&mut self, raw: &str, on_token: Option<&mpsc::Sender<String>>) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(':') {
            return;
        }
        let Some(data) = line.strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data == "[DONE]" {
            self.done = true;
            return;
        }
        if self.done {
            return;
        }
        let chunk: StreamChunk = match serde_json::from_str(data) {
            Ok(c) => c,
            Err(_) => return, // ignore keep-alive / heartbeat lines
        };
        if let Some(u) = chunk.usage {
            self.usage = Some(TokenUsage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                ..Default::default()
            });
        }
        for choice in &chunk.choices {
            let delta = &choice.delta;
            if let Some(content) = delta.content.as_deref() {
                if !content.is_empty() {
                    self.text.push_str(content);
                    if let Some(tx) = on_token {
                        let _ = tx.try_send(content.to_string());
                    }
                }
            }
            if let Some(reasoning) = delta.reasoning_content.as_deref() {
                if !reasoning.is_empty() {
                    self.reasoning.push_str(reasoning);
                }
            }
            if let Some(tcds) = &delta.tool_calls {
                for tcd in tcds {
                    let builder = self.tool_calls.entry(tcd.index).or_default();
                    if let Some(id) = &tcd.id {
                        if !id.is_empty() {
                            builder.id = id.clone();
                        }
                    }
                    if let Some(f) = &tcd.function {
                        if let Some(name) = &f.name {
                            if !name.is_empty() {
                                builder.name = name.clone();
                            }
                        }
                        if let Some(args) = &f.arguments {
                            builder.arguments.push_str(args);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn finalize(self) -> ChatResponse {
        let text = if self.text.is_empty() {
            None
        } else {
            Some(self.text)
        };
        let reasoning_content = if self.reasoning.is_empty() {
            None
        } else {
            Some(self.reasoning)
        };
        let tool_calls = self
            .tool_calls
            .into_values()
            .filter(|b| !b.name.is_empty())
            .map(|b| ToolCall {
                id: b.id,
                name: b.name,
                arguments: b.arguments,
            })
            .collect();
        ChatResponse {
            text,
            tool_calls,
            usage: self.usage,
            reasoning_content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sse(value: serde_json::Value) -> String {
        format!("data: {value}\n")
    }

    #[test]
    fn folds_content_and_forwards_tokens() {
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let mut acc = SseAccumulator::new();
        acc.process_chunk(
            &sse(serde_json::json!({"choices":[{"delta":{"content":"Hel"}}]})),
            Some(&tx),
        );
        acc.process_chunk(
            &sse(serde_json::json!({"choices":[{"delta":{"content":"lo"}}]})),
            Some(&tx),
        );
        // Lines after [DONE] are ignored.
        acc.process_chunk("data: [DONE]\n", Some(&tx));
        acc.process_chunk(
            &sse(serde_json::json!({"choices":[{"delta":{"content":"!"}}]})),
            Some(&tx),
        );
        acc.finish(Some(&tx));

        let resp = acc.finalize();
        assert_eq!(resp.text.as_deref(), Some("Hello"));
        assert_eq!(rx.try_recv().unwrap(), "Hel");
        assert_eq!(rx.try_recv().unwrap(), "lo");
        assert!(rx.try_recv().is_err(), "no token forwarded after [DONE]");
    }

    #[test]
    fn reassembles_tool_call_fragments() {
        let mut acc = SseAccumulator::new();
        acc.process_chunk(
            &sse(serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"id":"call_1","function":{"name":"shell"}}]}}]})),
            None,
        );
        acc.process_chunk(
            &sse(serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"function":{"arguments":"{\"cmd\":"}}]}}]})),
            None,
        );
        acc.process_chunk(
            &sse(serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"function":{"arguments":"\"date\"}"}}]}}]})),
            None,
        );

        let resp = acc.finalize();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[0].name, "shell");
        assert_eq!(resp.tool_calls[0].arguments, r#"{"cmd":"date"}"#);
    }

    #[test]
    fn buffers_partial_lines_across_chunks() {
        // A `data:` line split across two TCP chunks must parse once whole.
        let mut acc = SseAccumulator::new();
        acc.process_chunk("data: {\"choices\":[{\"delta\":{\"content\":\"par", None);
        acc.process_chunk("tial\"}}]}\n", None);
        acc.finish(None);
        assert_eq!(acc.finalize().text.as_deref(), Some("partial"));
    }

    #[test]
    fn captures_usage_from_terminal_chunk() {
        let mut acc = SseAccumulator::new();
        acc.process_chunk(
            &sse(serde_json::json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":4}})),
            None,
        );
        let usage = acc.finalize().usage.expect("usage captured");
        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.output_tokens, Some(4));
    }
}
