use crate::providers::traits::{
    ChatMessage, ChatRequest as ProviderChatRequest, ChatResponse as ProviderChatResponse,
    Provider, ProviderCapabilities, TokenUsage, ToolCall as ProviderToolCall,
};
use crate::tools::ToolSpec;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const DEFAULT_MAX_TOKENS: u32 = 16384;

pub struct AnthropicProvider {
    credential: Option<String>,
    base_url: String,
    max_tokens: u32,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Message>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct NativeChatRequest<'a> {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<SystemPrompt>,
    messages: Vec<NativeMessage>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<NativeToolSpec<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
struct NativeMessage {
    role: String,
    content: Vec<NativeContentOut>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum NativeContentOut {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Debug, Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    kind: &'static str,
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct NativeToolSpec<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: String,
}

impl CacheControl {
    fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SystemPrompt {
    String(String),
    Blocks(Vec<SystemBlock>),
}

#[derive(Debug, Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Deserialize)]
struct NativeChatResponse {
    #[serde(default)]
    content: Vec<NativeContentIn>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    /// Prefix-cache write — tokens billed at 1.25× input. Set when the
    /// request established a new cache entry.
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    /// Prefix-cache hit — tokens billed at 0.1× input. The metric users
    /// care about for prefix-cache cost savings.
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct NativeContentIn {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

impl AnthropicProvider {
    pub fn new(credential: Option<&str>) -> Self {
        Self::with_base_url(credential, None)
    }

    pub fn with_base_url(credential: Option<&str>, base_url: Option<&str>) -> Self {
        let base_url = base_url
            .map(|u| u.trim_end_matches('/'))
            .unwrap_or("https://api.anthropic.com")
            .to_string();
        Self {
            credential: credential
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(ToString::to_string),
            base_url,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    fn is_setup_token(token: &str) -> bool {
        token.starts_with("sk-ant-oat01-")
    }

    fn apply_auth(
        &self,
        request: reqwest::RequestBuilder,
        credential: &str,
    ) -> reqwest::RequestBuilder {
        if Self::is_setup_token(credential) {
            request
                .header("Authorization", format!("Bearer {credential}"))
                .header("anthropic-beta", "oauth-2025-04-20")
        } else {
            request.header("x-api-key", credential)
        }
    }

    /// Cache system prompts larger than ~1024 tokens (3KB of text)
    fn should_cache_system(text: &str) -> bool {
        text.len() > 3072
    }

    /// Cache conversations with more than 4 messages (excluding system)
    fn should_cache_conversation(messages: &[ChatMessage]) -> bool {
        messages.iter().filter(|m| m.role != "system").count() > 4
    }

    /// Apply cache control to the last message content block
    fn apply_cache_to_last_message(messages: &mut [NativeMessage]) {
        if let Some(last_msg) = messages.last_mut() {
            if let Some(last_content) = last_msg.content.last_mut() {
                match last_content {
                    NativeContentOut::Text { cache_control, .. }
                    | NativeContentOut::ToolResult { cache_control, .. } => {
                        *cache_control = Some(CacheControl::ephemeral());
                    }
                    NativeContentOut::ToolUse { .. } | NativeContentOut::Image { .. } => {}
                }
            }
        }
    }

    fn convert_tools<'a>(
        tools: Option<&'a [ToolSpec]>,
        use_cache: bool,
    ) -> Option<Vec<NativeToolSpec<'a>>> {
        let items = tools?;
        if items.is_empty() {
            return None;
        }
        let mut native_tools: Vec<NativeToolSpec<'a>> = items
            .iter()
            .map(|tool| NativeToolSpec {
                name: &tool.name,
                description: &tool.description,
                input_schema: &tool.parameters,
                cache_control: None,
            })
            .collect();

        // Cache the last tool definition (caches all tools) — only for native Anthropic
        if use_cache {
            if let Some(last_tool) = native_tools.last_mut() {
                last_tool.cache_control = Some(CacheControl::ephemeral());
            }
        }

        Some(native_tools)
    }

    fn parse_assistant_tool_call_message(content: &str) -> Option<Vec<NativeContentOut>> {
        let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
        let tool_calls = value
            .get("tool_calls")
            .and_then(|v| serde_json::from_value::<Vec<ProviderToolCall>>(v.clone()).ok())?;

        let mut blocks = Vec::new();
        if let Some(text) = value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            blocks.push(NativeContentOut::Text {
                text: text.to_string(),
                cache_control: None,
            });
        }
        for call in tool_calls {
            let input = serde_json::from_str::<serde_json::Value>(&call.arguments)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
            blocks.push(NativeContentOut::ToolUse {
                id: call.id,
                name: call.name,
                input,
                cache_control: None,
            });
        }
        Some(blocks)
    }

    fn parse_tool_result_message(content: &str) -> Option<NativeMessage> {
        let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
        let tool_use_id = value
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let result = value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Some(NativeMessage {
            role: "user".to_string(),
            content: vec![NativeContentOut::ToolResult {
                tool_use_id,
                content: result,
                cache_control: None,
            }],
        })
    }

    fn parse_inline_image(marker_content: &str) -> Option<NativeContentOut> {
        let rest = marker_content.strip_prefix("data:")?;
        let semi_pos = rest.find(';')?;
        let media_type = rest[..semi_pos].to_string();
        let after_semi = &rest[semi_pos + 1..];
        let data = after_semi.strip_prefix("base64,")?;
        Some(NativeContentOut::Image {
            source: ImageSource {
                kind: "base64",
                media_type,
                data: data.to_string(),
            },
        })
    }

    fn build_user_content_blocks(content: &str) -> Vec<NativeContentOut> {
        let (text_part, image_refs) = crate::multimodal::parse_image_markers(content);
        if image_refs.is_empty() {
            return vec![NativeContentOut::Text {
                text: content.to_string(),
                cache_control: None,
            }];
        }
        let mut blocks = Vec::new();
        if !text_part.trim().is_empty() {
            blocks.push(NativeContentOut::Text {
                text: text_part,
                cache_control: None,
            });
        }
        for marker_content in image_refs {
            if let Some(image_block) = Self::parse_inline_image(&marker_content) {
                blocks.push(image_block);
            }
        }
        blocks
    }

    /// Convert ChatMessage history to Anthropic native format.
    /// `use_cache`: enable prompt caching (Anthropic official only).
    /// `native_tool_msgs`: use tool_use/tool_result content blocks (Anthropic official).
    ///   When false, tool call history is kept as plain text — needed for providers
    ///   like Kimi that support tool_use in responses but not in request messages.
    fn convert_messages(
        messages: &[ChatMessage],
        use_cache: bool,
        native_tool_msgs: bool,
    ) -> (Option<SystemPrompt>, Vec<NativeMessage>) {
        let mut system_text = None;
        let mut native_messages = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    if system_text.is_none() {
                        system_text = Some(msg.content.clone());
                    }
                }
                "assistant" => {
                    if native_tool_msgs {
                        if let Some(blocks) = Self::parse_assistant_tool_call_message(&msg.content)
                        {
                            native_messages.push(NativeMessage {
                                role: "assistant".to_string(),
                                content: blocks,
                            });
                        } else {
                            native_messages.push(NativeMessage {
                                role: "assistant".to_string(),
                                content: vec![NativeContentOut::Text {
                                    text: msg.content.clone(),
                                    cache_control: None,
                                }],
                            });
                        }
                    } else {
                        // Non-native: downgrade tool call JSON to plain text
                        let text = Self::downgrade_assistant_tool_message(&msg.content);
                        native_messages.push(NativeMessage {
                            role: "assistant".to_string(),
                            content: vec![NativeContentOut::Text {
                                text,
                                cache_control: None,
                            }],
                        });
                    }
                }
                "tool" => {
                    if native_tool_msgs {
                        if let Some(tool_result) = Self::parse_tool_result_message(&msg.content) {
                            native_messages.push(tool_result);
                        } else {
                            native_messages.push(NativeMessage {
                                role: "user".to_string(),
                                content: vec![NativeContentOut::Text {
                                    text: msg.content.clone(),
                                    cache_control: None,
                                }],
                            });
                        }
                    } else {
                        // Non-native: convert tool result to plain text user message
                        let text = Self::downgrade_tool_result_message(&msg.content);
                        native_messages.push(NativeMessage {
                            role: "user".to_string(),
                            content: vec![NativeContentOut::Text {
                                text,
                                cache_control: None,
                            }],
                        });
                    }
                }
                _ => {
                    native_messages.push(NativeMessage {
                        role: "user".to_string(),
                        content: Self::build_user_content_blocks(&msg.content),
                    });
                }
            }
        }

        // Convert system text to SystemPrompt with cache control if large (native Anthropic only)
        let system_prompt = system_text.map(|text| {
            if use_cache && Self::should_cache_system(&text) {
                SystemPrompt::Blocks(vec![SystemBlock {
                    block_type: "text".to_string(),
                    text,
                    cache_control: Some(CacheControl::ephemeral()),
                }])
            } else {
                SystemPrompt::String(text)
            }
        });

        (system_prompt, native_messages)
    }

    fn parse_native_response(response: NativeChatResponse) -> ProviderChatResponse {
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        let usage = response.usage.map(|u| TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_input_tokens: u.cache_creation_input_tokens,
            cache_read_input_tokens: u.cache_read_input_tokens,
        });

        for block in response.content {
            match block.kind.as_str() {
                "text" => {
                    if let Some(text) = block.text.map(|t| t.trim().to_string()) {
                        if !text.is_empty() {
                            text_parts.push(text);
                        }
                    }
                }
                "tool_use" => {
                    let name = block.name.unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    let arguments = block
                        .input
                        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                    tool_calls.push(ProviderToolCall {
                        id: block.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        name,
                        arguments: arguments.to_string(),
                    });
                }
                _ => {}
            }
        }

        ProviderChatResponse {
            text: if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            },
            tool_calls,
            usage,
            reasoning_content: None,
        }
    }

    fn http_client(&self) -> Client {
        let builder = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10));
        let builder = crate::config::apply_runtime_proxy_to_builder(builder, "provider.anthropic");
        builder.build().unwrap_or_else(|e| {
            tracing::warn!("[anthropic] http_client build failed: {e}");
            reqwest::Client::new()
        })
    }

    /// Longer timeouts for streaming requests (10 min read, 30s connect)
    fn streaming_http_client(&self) -> Client {
        // Disable connection pooling — some Anthropic-compatible endpoints (e.g. Kimi)
        // return 400 on the second request when reusing a connection after SSE streaming.
        let builder = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .timeout(std::time::Duration::from_secs(600))
            .connect_timeout(std::time::Duration::from_secs(30));
        let builder = crate::config::apply_runtime_proxy_to_builder(builder, "provider.anthropic");
        builder.build().unwrap_or_else(|e| {
            tracing::warn!("[anthropic] streaming_http_client build failed: {e}");
            reqwest::Client::new()
        })
    }

    /// Returns true only for the official Anthropic API endpoint.
    /// Custom endpoints (Kimi, etc.) don't support prompt caching.
    fn supports_caching(&self) -> bool {
        self.base_url.contains("api.anthropic.com")
    }

    /// Downgrade an assistant message that may contain tool_use JSON to plain text.
    /// For providers like Kimi that return tool_use in responses but reject them in requests.
    fn downgrade_assistant_tool_message(content: &str) -> String {
        // Try to parse as our internal tool call JSON format
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
            let mut parts = Vec::new();
            // Extract any text content
            if let Some(text) = value.get("content").and_then(|v| v.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
            // Extract tool calls as readable text
            if let Some(calls) = value.get("tool_calls").and_then(|v| v.as_array()) {
                for call in calls {
                    let name = call
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let args = call
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    parts.push(format!("[Calling tool: {name}({args})]"));
                }
            }
            if !parts.is_empty() {
                return parts.join("\n");
            }
        }
        // Not parseable or empty — return as-is
        content.to_string()
    }

    /// Downgrade a tool result message to plain text user message.
    /// For providers like Kimi that reject tool_result content blocks in requests.
    fn downgrade_tool_result_message(content: &str) -> String {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
            let tool_id = value
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let result = value.get("content").and_then(|v| v.as_str()).unwrap_or("");
            return format!("[Tool result for {tool_id}]:\n{result}");
        }
        // Not parseable — return as-is
        content.to_string()
    }

    /// Consume an Anthropic SSE stream and accumulate into a NativeChatResponse.
    /// Falls back to JSON parsing if the response is not SSE formatted.
    ///
    /// Convenience wrapper around [`consume_sse_stream_with_tokens`] that
    /// drops per-token deltas (kept for backward compatibility with
    /// `chat_with_system`'s non-streaming caller). Use
    /// `consume_sse_stream_with_tokens(response, on_token)` directly when
    /// you want real-time token delivery.
    async fn consume_sse_stream(response: reqwest::Response) -> anyhow::Result<NativeChatResponse> {
        Self::consume_sse_stream_with_tokens(response, None).await
    }

    /// True-streaming variant: parses the SSE byte stream incrementally
    /// and pushes each `text_delta` to `on_token` the moment it arrives,
    /// while still accumulating the full `NativeChatResponse` for the
    /// final return (so tool_calls + usage survive).
    ///
    /// Falls back to JSON parsing if the response body turns out to be a
    /// plain JSON object (provider doesn't support streaming despite our
    /// `stream: true` request).
    ///
    /// `tool_use` `input_json_delta` chunks are NOT forwarded to
    /// `on_token` — they're not user-facing text and would corrupt the
    /// chat transcript. Only `text_delta` chunks flow through.
    ///
    /// When `on_token` is `None`, semantics match the original
    /// buffered implementation exactly (used by tests and the
    /// non-streaming `chat()` path).
    async fn consume_sse_stream_with_tokens(
        response: reqwest::Response,
        on_token: Option<&tokio::sync::mpsc::Sender<String>>,
    ) -> anyhow::Result<NativeChatResponse> {
        use futures_util::StreamExt;

        let mut bytes_stream = response.bytes_stream();
        let mut body = String::new();
        let mut pending = String::new();

        let mut content_blocks: Vec<NativeContentIn> = Vec::new();
        let mut current_text = String::new();
        let mut current_tool_id: Option<String> = None;
        let mut current_tool_name: Option<String> = None;
        let mut current_tool_input_json = String::new();
        let mut current_block_type = String::new();
        let mut input_tokens: Option<u64> = None;
        let mut output_tokens: Option<u64> = None;
        let mut cache_creation_input_tokens: Option<u64> = None;
        let mut cache_read_input_tokens: Option<u64> = None;
        let mut sse_detected = false;
        let mut format_decided = false;

        // Read chunks, splitting on the SSE event boundary ("\n\n").
        // `pending` carries the trailing partial event from one chunk to
        // the next; `body` accumulates the full payload for the
        // non-SSE-fallback JSON parse path.
        while let Some(item) = bytes_stream.next().await {
            let bytes = item?;
            let text = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    // Partial UTF-8 at chunk boundary — fall back to
                    // lossy decoding for the body buffer (we still need
                    // to read the rest of the stream to detect SSE vs
                    // JSON, but we can't safely incrementally parse this
                    // chunk). Future improvement: maintain a byte
                    // buffer and only decode at event boundaries.
                    String::from_utf8_lossy(&bytes).into_owned()
                }
            };
            body.push_str(&text);

            // First-chunk format detection: SSE starts with "event:" or
            // "data:"; anything else is treated as buffered JSON.
            if !format_decided {
                let head = body.trim_start();
                if head.starts_with("event:") || head.starts_with("data:") {
                    sse_detected = true;
                } else if !head.is_empty() && !head.starts_with(|c: char| c.is_whitespace()) {
                    // Non-empty, non-SSE prefix → JSON path. Drain the
                    // rest of the stream then fall through to JSON parse.
                    sse_detected = false;
                }
                // Empty head → wait for more bytes before deciding.
                if !head.is_empty() {
                    format_decided = true;
                }
            }

            if !sse_detected {
                continue;
            }

            pending.push_str(&text);

            // Process all complete events in `pending`. An event ends at
            // "\n\n"; the trailing partial event stays in `pending` for
            // the next chunk to complete.
            while let Some(boundary) = pending.find("\n\n") {
                let event_block: String = pending.drain(..boundary + 2).collect();
                Self::process_sse_event_block(
                    &event_block,
                    on_token,
                    &mut content_blocks,
                    &mut current_text,
                    &mut current_tool_id,
                    &mut current_tool_name,
                    &mut current_tool_input_json,
                    &mut current_block_type,
                    &mut input_tokens,
                    &mut output_tokens,
                    &mut cache_creation_input_tokens,
                    &mut cache_read_input_tokens,
                );
            }
        }

        // Drain any trailing event that didn't end with "\n\n" (Anthropic
        // always sends the trailing blank line, but a custom-compatible
        // endpoint might not).
        if sse_detected && !pending.is_empty() {
            Self::process_sse_event_block(
                &pending,
                on_token,
                &mut content_blocks,
                &mut current_text,
                &mut current_tool_id,
                &mut current_tool_name,
                &mut current_tool_input_json,
                &mut current_block_type,
                &mut input_tokens,
                &mut output_tokens,
                &mut cache_creation_input_tokens,
                &mut cache_read_input_tokens,
            );
        }

        if !sse_detected {
            // Non-SSE response: fall back to buffered JSON parse.
            tracing::info!(
                "[anthropic] non-SSE response ({}B), parsing as JSON",
                body.len(),
            );
            return match serde_json::from_str::<NativeChatResponse>(&body) {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    tracing::warn!(
                        "[anthropic] JSON parse failed: {e}, preview: {}",
                        &body[..body.len().min(500)],
                    );
                    anyhow::bail!("Failed to parse response as JSON or SSE: {e}")
                }
            };
        }

        if content_blocks.is_empty() {
            tracing::warn!(
                "[anthropic] SSE parsed but no content blocks, body preview: {}",
                &body[..body.len().min(500)],
            );
        }

        let usage = if input_tokens.is_some()
            || output_tokens.is_some()
            || cache_creation_input_tokens.is_some()
            || cache_read_input_tokens.is_some()
        {
            Some(AnthropicUsage {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            })
        } else {
            None
        };

        Ok(NativeChatResponse {
            content: content_blocks,
            usage,
        })
    }

    /// Parse a single SSE event block (one or more lines, possibly with
    /// trailing newlines) and update the streaming accumulator state.
    /// Pushes `text_delta` content to `on_token` immediately.
    #[allow(clippy::too_many_arguments)]
    fn process_sse_event_block(
        event_block: &str,
        on_token: Option<&tokio::sync::mpsc::Sender<String>>,
        content_blocks: &mut Vec<NativeContentIn>,
        current_text: &mut String,
        current_tool_id: &mut Option<String>,
        current_tool_name: &mut Option<String>,
        current_tool_input_json: &mut String,
        current_block_type: &mut String,
        input_tokens: &mut Option<u64>,
        output_tokens: &mut Option<u64>,
        cache_creation_input_tokens: &mut Option<u64>,
        cache_read_input_tokens: &mut Option<u64>,
    ) {
        for line in event_block.lines() {
            // SSE spec: space after colon is optional (Kimi sends "data:{json}")
            let data = match line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            {
                Some(d) if d != "[DONE]" => d,
                _ => continue,
            };
            let event: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            match event["type"].as_str() {
                Some("message_start") => {
                    if let Some(usage) = event.get("message").and_then(|m| m.get("usage")) {
                        *input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64());
                        // Anthropic puts cache_* counters on message_start
                        // (they describe the prompt that was just billed,
                        // not the output). Pick them up here so the final
                        // TokenUsage shows real hit rate.
                        if let Some(v) = usage
                            .get("cache_creation_input_tokens")
                            .and_then(serde_json::Value::as_u64)
                        {
                            *cache_creation_input_tokens = Some(v);
                        }
                        if let Some(v) = usage
                            .get("cache_read_input_tokens")
                            .and_then(serde_json::Value::as_u64)
                        {
                            *cache_read_input_tokens = Some(v);
                        }
                    }
                }
                Some("content_block_start") => {
                    let block = &event["content_block"];
                    *current_block_type = block["type"].as_str().unwrap_or("").to_string();
                    if current_block_type == "text" {
                        current_text.clear();
                    } else if current_block_type == "tool_use" {
                        *current_tool_id = block["id"].as_str().map(String::from);
                        *current_tool_name = block["name"].as_str().map(String::from);
                        current_tool_input_json.clear();
                    }
                }
                Some("content_block_delta") => {
                    let delta = &event["delta"];
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            if let Some(text) = delta["text"].as_str() {
                                current_text.push_str(text);
                                // Push to on_token IMMEDIATELY — this is the
                                // TTFB win. try_send + drop-on-full because
                                // back-pressuring the LLM stream is worse
                                // than dropping a visible token.
                                if let Some(sender) = on_token {
                                    if let Err(e) = sender.try_send(text.to_string()) {
                                        tracing::debug!(
                                            "[anthropic] on_token try_send failed (dropped chunk): {e}"
                                        );
                                    }
                                }
                            }
                        }
                        Some("input_json_delta") => {
                            // Tool argument JSON — accumulate, do NOT forward
                            // to on_token (not user-facing text).
                            if let Some(json) = delta["partial_json"].as_str() {
                                current_tool_input_json.push_str(json);
                            }
                        }
                        _ => {}
                    }
                }
                Some("content_block_stop") => {
                    if current_block_type == "text" {
                        content_blocks.push(NativeContentIn {
                            kind: "text".to_string(),
                            text: Some(current_text.clone()),
                            id: None,
                            name: None,
                            input: None,
                        });
                        current_text.clear();
                    } else if current_block_type == "tool_use" {
                        let input = serde_json::from_str(current_tool_input_json).ok();
                        content_blocks.push(NativeContentIn {
                            kind: "tool_use".to_string(),
                            text: None,
                            id: current_tool_id.take(),
                            name: current_tool_name.take(),
                            input,
                        });
                        current_tool_input_json.clear();
                    }
                }
                Some("message_delta") => {
                    if let Some(usage) = event.get("usage") {
                        *output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64());
                    }
                }
                _ => {}
            }
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let credential = self.credential.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Anthropic credentials not set. Set ANTHROPIC_API_KEY or ANTHROPIC_OAUTH_TOKEN (setup-token)."
            )
        })?;

        let chat_req = ChatRequest {
            model: model.to_string(),
            max_tokens: self.max_tokens,
            system: system_prompt.map(ToString::to_string),
            messages: vec![Message {
                role: "user".to_string(),
                content: message.to_string(),
            }],
            temperature,
            stream: Some(true),
        };

        let mut request = self
            .streaming_http_client()
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&chat_req);

        request = self.apply_auth(request, credential);

        let response = request.send().await?;

        if !response.status().is_success() {
            return Err(super::api_error("Anthropic", response).await);
        }

        // Use SSE stream consumption to avoid Privoxy tunnel timeout
        let native_response = Self::consume_sse_stream(response).await?;
        native_response
            .content
            .into_iter()
            .find(|c| c.kind == "text")
            .and_then(|c| c.text)
            .ok_or_else(|| anyhow::anyhow!("No text in chat_with_system response"))
    }

    async fn chat(
        &self,
        request: ProviderChatRequest<'_>,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ProviderChatResponse> {
        let credential = self.credential.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Anthropic credentials not set. Set ANTHROPIC_API_KEY or ANTHROPIC_OAUTH_TOKEN (setup-token)."
            )
        })?;

        let use_cache = self.supports_caching();
        let native_tool_msgs = true; // Kimi fully supports Anthropic tool_use/tool_result format (curl-verified)
        let (system_prompt, mut messages) =
            Self::convert_messages(request.messages, use_cache, native_tool_msgs);

        // Auto-cache last message if conversation is long (native Anthropic only)
        if use_cache && Self::should_cache_conversation(request.messages) {
            Self::apply_cache_to_last_message(&mut messages);
        }

        let native_request = NativeChatRequest {
            model: model.to_string(),
            max_tokens: self.max_tokens,
            system: system_prompt,
            messages,
            temperature,
            tools: Self::convert_tools(request.tools, use_cache),
            stream: Some(true),
        };

        let url = format!("{}/v1/messages", self.base_url);

        let mut request = self
            .streaming_http_client()
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&native_request);
        request = self.apply_auth(request, credential);

        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(super::api_error("Anthropic", response).await);
        }

        let native_response = Self::consume_sse_stream(response).await?;
        Ok(Self::parse_native_response(native_response))
    }

    /// True-streaming chat: pushes each Anthropic `text_delta` to
    /// `on_token` the moment it arrives over SSE, while still returning
    /// the full ChatResponse (tool_calls, usage) when the stream ends.
    /// This is the TTFB-improving override; `chat()` above remains
    /// available for callers that don't have a token sink.
    async fn chat_streaming(
        &self,
        request: ProviderChatRequest<'_>,
        model: &str,
        temperature: f64,
        on_token: Option<&tokio::sync::mpsc::Sender<String>>,
    ) -> anyhow::Result<ProviderChatResponse> {
        let credential = self.credential.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Anthropic credentials not set. Set ANTHROPIC_API_KEY or ANTHROPIC_OAUTH_TOKEN (setup-token)."
            )
        })?;

        let use_cache = self.supports_caching();
        let native_tool_msgs = true;
        let (system_prompt, mut messages) =
            Self::convert_messages(request.messages, use_cache, native_tool_msgs);

        if use_cache && Self::should_cache_conversation(request.messages) {
            Self::apply_cache_to_last_message(&mut messages);
        }

        let native_request = NativeChatRequest {
            model: model.to_string(),
            max_tokens: self.max_tokens,
            system: system_prompt,
            messages,
            temperature,
            tools: Self::convert_tools(request.tools, use_cache),
            stream: Some(true),
        };

        let url = format!("{}/v1/messages", self.base_url);

        let mut http_request = self
            .streaming_http_client()
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&native_request);
        http_request = self.apply_auth(http_request, credential);

        let response = http_request.send().await?;
        if !response.status().is_success() {
            return Err(super::api_error("Anthropic", response).await);
        }

        let native_response = Self::consume_sse_stream_with_tokens(response, on_token).await?;
        Ok(Self::parse_native_response(native_response))
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            native_tool_calling: true,
            vision: true,
        }
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ProviderChatResponse> {
        // Convert OpenAI-format tool JSON to ToolSpec so we can reuse the
        // existing `chat()` method which handles full message history,
        // system prompt extraction, caching, and Anthropic native formatting.
        let tool_specs: Vec<ToolSpec> = tools
            .iter()
            .filter_map(|t| {
                let func = t.get("function").or_else(|| {
                    tracing::warn!("Skipping malformed tool definition (missing 'function' key)");
                    None
                })?;
                let name = func.get("name").and_then(|n| n.as_str()).or_else(|| {
                    tracing::warn!("Skipping tool with missing or non-string 'name'");
                    None
                })?;
                Some(ToolSpec {
                    name: name.to_string(),
                    description: func
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    parameters: func
                        .get("parameters")
                        .cloned()
                        .unwrap_or(serde_json::json!({"type": "object"})),
                })
            })
            .collect();

        let request = ProviderChatRequest {
            messages,
            tools: if tool_specs.is_empty() {
                None
            } else {
                Some(&tool_specs)
            },
        };
        self.chat(request, model, temperature).await
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        if let Some(credential) = self.credential.as_ref() {
            let mut request = self
                .http_client()
                .post(format!("{}/v1/messages", self.base_url))
                .header("anthropic-version", "2023-06-01");
            request = self.apply_auth(request, credential);
            // Send a minimal request; the goal is TLS + HTTP/2 setup, not a valid response.
            // Anthropic has no lightweight GET endpoint, so we accept any non-network error.
            let _ = request.send().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::anthropic_token::{detect_auth_kind, AnthropicAuthKind};

    #[test]
    fn creates_with_key() {
        let p = AnthropicProvider::new(Some("anthropic-test-credential"));
        assert!(p.credential.is_some());
        assert_eq!(p.credential.as_deref(), Some("anthropic-test-credential"));
        assert_eq!(p.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn creates_without_key() {
        let p = AnthropicProvider::new(None);
        assert!(p.credential.is_none());
        assert_eq!(p.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn creates_with_empty_key() {
        let p = AnthropicProvider::new(Some(""));
        assert!(p.credential.is_none());
    }

    #[test]
    fn creates_with_whitespace_key() {
        let p = AnthropicProvider::new(Some("  anthropic-test-credential  "));
        assert!(p.credential.is_some());
        assert_eq!(p.credential.as_deref(), Some("anthropic-test-credential"));
    }

    #[test]
    fn creates_with_custom_base_url() {
        let p = AnthropicProvider::with_base_url(
            Some("anthropic-credential"),
            Some("https://api.example.com"),
        );
        assert_eq!(p.base_url, "https://api.example.com");
        assert_eq!(p.credential.as_deref(), Some("anthropic-credential"));
    }

    #[test]
    fn custom_base_url_trims_trailing_slash() {
        let p = AnthropicProvider::with_base_url(None, Some("https://api.example.com/"));
        assert_eq!(p.base_url, "https://api.example.com");
    }

    #[test]
    fn default_base_url_when_none_provided() {
        let p = AnthropicProvider::with_base_url(None, None);
        assert_eq!(p.base_url, "https://api.anthropic.com");
    }

    #[tokio::test]
    async fn chat_fails_without_key() {
        let p = AnthropicProvider::new(None);
        let result = p
            .chat_with_system(None, "hello", "claude-3-opus", 0.7)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("credentials not set"),
            "Expected key error, got: {err}"
        );
    }

    #[test]
    fn setup_token_detection_works() {
        assert!(AnthropicProvider::is_setup_token("sk-ant-oat01-abcdef"));
        assert!(!AnthropicProvider::is_setup_token("sk-ant-api-key"));
    }

    #[test]
    fn apply_auth_uses_bearer_and_beta_for_setup_tokens() {
        let provider = AnthropicProvider::new(None);
        let request = provider
            .apply_auth(
                provider
                    .http_client()
                    .get("https://api.anthropic.com/v1/models"),
                "sk-ant-oat01-test-token",
            )
            .build()
            .expect("request should build");

        assert_eq!(
            request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-ant-oat01-test-token")
        );
        assert_eq!(
            request
                .headers()
                .get("anthropic-beta")
                .and_then(|v| v.to_str().ok()),
            Some("oauth-2025-04-20")
        );
        assert!(request.headers().get("x-api-key").is_none());
    }

    #[test]
    fn apply_auth_uses_x_api_key_for_regular_tokens() {
        let provider = AnthropicProvider::new(None);
        let request = provider
            .apply_auth(
                provider
                    .http_client()
                    .get("https://api.anthropic.com/v1/models"),
                "sk-ant-api-key",
            )
            .build()
            .expect("request should build");

        assert_eq!(
            request
                .headers()
                .get("x-api-key")
                .and_then(|v| v.to_str().ok()),
            Some("sk-ant-api-key")
        );
        assert!(request.headers().get("authorization").is_none());
        assert!(request.headers().get("anthropic-beta").is_none());
    }

    #[tokio::test]
    async fn chat_with_system_fails_without_key() {
        let p = AnthropicProvider::new(None);
        let result = p
            .chat_with_system(Some("You are Plaw"), "hello", "claude-3-opus", 0.7)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn chat_request_serializes_without_system() {
        let req = ChatRequest {
            model: "claude-3-opus".to_string(),
            max_tokens: 4096,
            system: None,
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            temperature: 0.7,
            stream: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            !json.contains("system"),
            "system field should be skipped when None"
        );
        assert!(json.contains("claude-3-opus"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn chat_request_serializes_with_system() {
        let req = ChatRequest {
            model: "claude-3-opus".to_string(),
            max_tokens: 4096,
            system: Some("You are Plaw".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            temperature: 0.7,
            stream: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"system\":\"You are Plaw\""));
    }

    #[test]
    fn temperature_range_serializes() {
        for temp in [0.0, 0.5, 1.0, 2.0] {
            let req = ChatRequest {
                model: "claude-3-opus".to_string(),
                max_tokens: 4096,
                system: None,
                messages: vec![],
                temperature: temp,
                stream: None,
            };
            let json = serde_json::to_string(&req).unwrap();
            assert!(json.contains(&format!("{temp}")));
        }
    }

    #[test]
    fn detects_auth_from_jwt_shape() {
        let kind = detect_auth_kind("a.b.c", None);
        assert_eq!(kind, AnthropicAuthKind::Authorization);
    }

    #[test]
    fn cache_control_serializes_correctly() {
        let cache = CacheControl::ephemeral();
        let json = serde_json::to_string(&cache).unwrap();
        assert_eq!(json, r#"{"type":"ephemeral"}"#);
    }

    #[test]
    fn system_prompt_string_variant_serializes() {
        let prompt = SystemPrompt::String("You are a helpful assistant".to_string());
        let json = serde_json::to_string(&prompt).unwrap();
        assert_eq!(json, r#""You are a helpful assistant""#);
    }

    #[test]
    fn system_prompt_blocks_variant_serializes() {
        let prompt = SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "You are a helpful assistant".to_string(),
            cache_control: Some(CacheControl::ephemeral()),
        }]);
        let json = serde_json::to_string(&prompt).unwrap();
        assert!(json.contains(r#""type":"text""#));
        assert!(json.contains("You are a helpful assistant"));
        assert!(json.contains(r#""type":"ephemeral""#));
    }

    #[test]
    fn system_prompt_blocks_without_cache_control() {
        let prompt = SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "Short prompt".to_string(),
            cache_control: None,
        }]);
        let json = serde_json::to_string(&prompt).unwrap();
        assert!(json.contains("Short prompt"));
        assert!(!json.contains("cache_control"));
    }

    #[test]
    fn native_content_text_without_cache_control() {
        let content = NativeContentOut::Text {
            text: "Hello".to_string(),
            cache_control: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains(r#""type":"text""#));
        assert!(json.contains("Hello"));
        assert!(!json.contains("cache_control"));
    }

    #[test]
    fn native_content_text_with_cache_control() {
        let content = NativeContentOut::Text {
            text: "Hello".to_string(),
            cache_control: Some(CacheControl::ephemeral()),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains(r#""type":"text""#));
        assert!(json.contains("Hello"));
        assert!(json.contains(r#""cache_control":{"type":"ephemeral"}"#));
    }

    #[test]
    fn native_content_tool_use_without_cache_control() {
        let content = NativeContentOut::ToolUse {
            id: "tool_123".to_string(),
            name: "get_weather".to_string(),
            input: serde_json::json!({"location": "San Francisco"}),
            cache_control: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains(r#""type":"tool_use""#));
        assert!(json.contains("tool_123"));
        assert!(json.contains("get_weather"));
        assert!(!json.contains("cache_control"));
    }

    #[test]
    fn native_content_tool_result_with_cache_control() {
        let content = NativeContentOut::ToolResult {
            tool_use_id: "tool_123".to_string(),
            content: "Result data".to_string(),
            cache_control: Some(CacheControl::ephemeral()),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains(r#""type":"tool_result""#));
        assert!(json.contains("tool_123"));
        assert!(json.contains("Result data"));
        assert!(json.contains(r#""cache_control":{"type":"ephemeral"}"#));
    }

    #[test]
    fn native_tool_spec_without_cache_control() {
        let schema = serde_json::json!({"type": "object"});
        let tool = NativeToolSpec {
            name: "get_weather",
            description: "Get weather info",
            input_schema: &schema,
            cache_control: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("get_weather"));
        assert!(!json.contains("cache_control"));
    }

    #[test]
    fn native_tool_spec_with_cache_control() {
        let schema = serde_json::json!({"type": "object"});
        let tool = NativeToolSpec {
            name: "get_weather",
            description: "Get weather info",
            input_schema: &schema,
            cache_control: Some(CacheControl::ephemeral()),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("get_weather"));
        assert!(json.contains(r#""cache_control":{"type":"ephemeral"}"#));
    }

    #[test]
    fn should_cache_system_small_prompt() {
        let small_prompt = "You are a helpful assistant.";
        assert!(!AnthropicProvider::should_cache_system(small_prompt));
    }

    #[test]
    fn should_cache_system_large_prompt() {
        let large_prompt = "a".repeat(3073); // Just over 3072 bytes
        assert!(AnthropicProvider::should_cache_system(&large_prompt));
    }

    #[test]
    fn should_cache_system_boundary() {
        let boundary_prompt = "a".repeat(3072); // Exactly 3072 bytes
        assert!(!AnthropicProvider::should_cache_system(&boundary_prompt));

        let over_boundary = "a".repeat(3073);
        assert!(AnthropicProvider::should_cache_system(&over_boundary));
    }

    #[test]
    fn should_cache_conversation_short() {
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "System prompt".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "Hi".to_string(),
            },
        ];
        // Only 2 non-system messages
        assert!(!AnthropicProvider::should_cache_conversation(&messages));
    }

    #[test]
    fn should_cache_conversation_long() {
        let mut messages = vec![ChatMessage {
            role: "system".to_string(),
            content: "System prompt".to_string(),
        }];
        // Add 5 non-system messages
        for i in 0..5 {
            messages.push(ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("Message {i}"),
            });
        }
        assert!(AnthropicProvider::should_cache_conversation(&messages));
    }

    #[test]
    fn should_cache_conversation_boundary() {
        let mut messages = vec![];
        // Add exactly 4 non-system messages
        for i in 0..4 {
            messages.push(ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("Message {i}"),
            });
        }
        assert!(!AnthropicProvider::should_cache_conversation(&messages));

        // Add one more to cross boundary
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: "One more".to_string(),
        });
        assert!(AnthropicProvider::should_cache_conversation(&messages));
    }

    #[test]
    fn apply_cache_to_last_message_text() {
        let mut messages = vec![NativeMessage {
            role: "user".to_string(),
            content: vec![NativeContentOut::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
        }];

        AnthropicProvider::apply_cache_to_last_message(&mut messages);

        match &messages[0].content[0] {
            NativeContentOut::Text { cache_control, .. } => {
                assert!(cache_control.is_some());
            }
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn apply_cache_to_last_message_tool_result() {
        let mut messages = vec![NativeMessage {
            role: "user".to_string(),
            content: vec![NativeContentOut::ToolResult {
                tool_use_id: "tool_123".to_string(),
                content: "Result".to_string(),
                cache_control: None,
            }],
        }];

        AnthropicProvider::apply_cache_to_last_message(&mut messages);

        match &messages[0].content[0] {
            NativeContentOut::ToolResult { cache_control, .. } => {
                assert!(cache_control.is_some());
            }
            _ => panic!("Expected ToolResult variant"),
        }
    }

    #[test]
    fn apply_cache_to_last_message_does_not_affect_tool_use() {
        let mut messages = vec![NativeMessage {
            role: "assistant".to_string(),
            content: vec![NativeContentOut::ToolUse {
                id: "tool_123".to_string(),
                name: "get_weather".to_string(),
                input: serde_json::json!({}),
                cache_control: None,
            }],
        }];

        AnthropicProvider::apply_cache_to_last_message(&mut messages);

        // ToolUse should not be affected
        match &messages[0].content[0] {
            NativeContentOut::ToolUse { cache_control, .. } => {
                assert!(cache_control.is_none());
            }
            _ => panic!("Expected ToolUse variant"),
        }
    }

    #[test]
    fn apply_cache_empty_messages() {
        let mut messages = vec![];
        AnthropicProvider::apply_cache_to_last_message(&mut messages);
        // Should not panic
        assert!(messages.is_empty());
    }

    #[test]
    fn convert_tools_adds_cache_to_last_tool() {
        let tools = vec![
            ToolSpec {
                name: "tool1".to_string(),
                description: "First tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            ToolSpec {
                name: "tool2".to_string(),
                description: "Second tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        ];

        let native_tools = AnthropicProvider::convert_tools(Some(&tools), true).unwrap();

        assert_eq!(native_tools.len(), 2);
        assert!(native_tools[0].cache_control.is_none());
        assert!(native_tools[1].cache_control.is_some());
    }

    #[test]
    fn convert_tools_single_tool_gets_cache() {
        let tools = vec![ToolSpec {
            name: "tool1".to_string(),
            description: "Only tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];

        let native_tools = AnthropicProvider::convert_tools(Some(&tools), true).unwrap();

        assert_eq!(native_tools.len(), 1);
        assert!(native_tools[0].cache_control.is_some());
    }

    #[test]
    fn convert_messages_small_system_prompt() {
        let messages = vec![ChatMessage {
            role: "system".to_string(),
            content: "Short system prompt".to_string(),
        }];

        let (system_prompt, _) = AnthropicProvider::convert_messages(&messages, true, true);

        match system_prompt.unwrap() {
            SystemPrompt::String(s) => {
                assert_eq!(s, "Short system prompt");
            }
            SystemPrompt::Blocks(_) => panic!("Expected String variant for small prompt"),
        }
    }

    #[test]
    fn convert_messages_large_system_prompt() {
        let large_content = "a".repeat(3073);
        let messages = vec![ChatMessage {
            role: "system".to_string(),
            content: large_content.clone(),
        }];

        let (system_prompt, _) = AnthropicProvider::convert_messages(&messages, true, true);

        match system_prompt.unwrap() {
            SystemPrompt::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert_eq!(blocks[0].text, large_content);
                assert!(blocks[0].cache_control.is_some());
            }
            SystemPrompt::String(_) => panic!("Expected Blocks variant for large prompt"),
        }
    }

    #[test]
    fn backward_compatibility_native_chat_request() {
        // Test that requests without cache_control serialize identically to old format
        let req = NativeChatRequest {
            model: "claude-3-opus".to_string(),
            max_tokens: 4096,
            system: Some(SystemPrompt::String("System".to_string())),
            messages: vec![NativeMessage {
                role: "user".to_string(),
                content: vec![NativeContentOut::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            }],
            temperature: 0.7,
            tools: None,
            stream: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("cache_control"));
        assert!(json.contains(r#""system":"System""#));
    }

    #[tokio::test]
    async fn warmup_without_key_is_noop() {
        let provider = AnthropicProvider::new(None);
        let result = provider.warmup().await;
        assert!(result.is_ok());
    }

    #[test]
    fn convert_messages_preserves_multi_turn_history() {
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are helpful.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "gen a 2 sum in golang".to_string(),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "```go\nfunc twoSum(nums []int) {}\n```".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "what's meaning of make here?".to_string(),
            },
        ];

        let (system, native_msgs) = AnthropicProvider::convert_messages(&messages, true, true);

        // System prompt extracted
        assert!(system.is_some());
        // All 3 non-system messages preserved in order
        assert_eq!(native_msgs.len(), 3);
        assert_eq!(native_msgs[0].role, "user");
        assert_eq!(native_msgs[1].role, "assistant");
        assert_eq!(native_msgs[2].role, "user");
    }

    /// Integration test: spin up a mock Anthropic API server, call chat_with_tools
    /// with a multi-turn conversation + tools, and verify the request body contains
    /// ALL conversation turns and native tool definitions.
    #[tokio::test]
    async fn chat_with_tools_sends_full_history_and_native_tools() {
        use axum::{routing::post, Json, Router};
        use std::sync::{Arc, Mutex};
        use tokio::net::TcpListener;

        // Captured request body for assertion
        let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let app = Router::new().route(
            "/v1/messages",
            post(move |Json(body): Json<serde_json::Value>| {
                let cap = captured_clone.clone();
                async move {
                    *cap.lock().unwrap() = Some(body);
                    // Return SSE stream matching Anthropic streaming format
                    let sse_body = [
                        "event: message_start",
                        r#"data: {"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":100}}}"#,
                        "",
                        "event: content_block_start",
                        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                        "",
                        "event: content_block_delta",
                        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"The make function creates a map."}}"#,
                        "",
                        "event: content_block_stop",
                        r#"data: {"type":"content_block_stop","index":0}"#,
                        "",
                        "event: message_delta",
                        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}}"#,
                        "",
                        "event: message_stop",
                        r#"data: {"type":"message_stop"}"#,
                        "",
                    ].join("\n");
                    (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                        sse_body,
                    )
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Create provider pointing at mock server
        let provider = AnthropicProvider {
            credential: Some("test-key".to_string()),
            base_url: format!("http://{addr}"),
            max_tokens: DEFAULT_MAX_TOKENS,
        };

        // Multi-turn conversation: system → user (Go code) → assistant (code response) → user (follow-up)
        let messages = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("gen a 2 sum in golang"),
            ChatMessage::assistant("```go\nfunc twoSum(nums []int, target int) []int {\n    m := make(map[int]int)\n    for i, n := range nums {\n        if j, ok := m[target-n]; ok {\n            return []int{j, i}\n        }\n        m[n] = i\n    }\n    return nil\n}\n```"),
            ChatMessage::user("what's meaning of make here?"),
        ];

        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "shell",
                "description": "Run a shell command",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"}
                    },
                    "required": ["command"]
                }
            }
        })];

        let result = provider
            .chat_with_tools(&messages, &tools, "claude-opus-4-6", 0.7)
            .await;
        assert!(result.is_ok(), "chat_with_tools failed: {:?}", result.err());

        let body = captured
            .lock()
            .unwrap()
            .take()
            .expect("No request captured");

        // Verify system prompt extracted to top-level field
        let system = &body["system"];
        assert!(
            system.to_string().contains("helpful assistant"),
            "System prompt missing: {system}"
        );

        // Verify ALL conversation turns present in messages array
        let msgs = body["messages"].as_array().expect("messages not an array");
        assert_eq!(
            msgs.len(),
            3,
            "Expected 3 messages (2 user + 1 assistant), got {}",
            msgs.len()
        );

        // Turn 1: user with Go request
        assert_eq!(msgs[0]["role"], "user");
        let turn1_text = msgs[0]["content"].to_string();
        assert!(
            turn1_text.contains("2 sum"),
            "Turn 1 missing Go request: {turn1_text}"
        );

        // Turn 2: assistant with Go code
        assert_eq!(msgs[1]["role"], "assistant");
        let turn2_text = msgs[1]["content"].to_string();
        assert!(
            turn2_text.contains("make(map[int]int)"),
            "Turn 2 missing Go code: {turn2_text}"
        );

        // Turn 3: user follow-up
        assert_eq!(msgs[2]["role"], "user");
        let turn3_text = msgs[2]["content"].to_string();
        assert!(
            turn3_text.contains("meaning of make"),
            "Turn 3 missing follow-up: {turn3_text}"
        );

        // Verify native tools are present
        let api_tools = body["tools"].as_array().expect("tools not an array");
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0]["name"], "shell");
        assert!(
            api_tools[0]["input_schema"].is_object(),
            "Missing input_schema"
        );

        server_handle.abort();
    }

    #[test]
    fn native_response_parses_usage() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hello"}],
            "usage": {"input_tokens": 300, "output_tokens": 75}
        }"#;
        let resp: NativeChatResponse = serde_json::from_str(json).unwrap();
        let result = AnthropicProvider::parse_native_response(resp);
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(300));
        assert_eq!(usage.output_tokens, Some(75));
        // Cache fields absent on this fixture → both stay None.
        assert!(usage.cache_creation_input_tokens.is_none());
        assert!(usage.cache_read_input_tokens.is_none());
    }

    /// PR #78 — Anthropic surfaces the prefix-cache breakdown on the
    /// `usage` object when the request crossed a `cache_control`
    /// breakpoint. Buffered (non-streaming) JSON path.
    #[test]
    fn native_response_parses_cache_breakdown() {
        let json = r#"{
            "content": [{"type": "text", "text": "ok"}],
            "usage": {
                "input_tokens": 10000,
                "output_tokens": 50,
                "cache_creation_input_tokens": 8000,
                "cache_read_input_tokens": 1500
            }
        }"#;
        let resp: NativeChatResponse = serde_json::from_str(json).unwrap();
        let result = AnthropicProvider::parse_native_response(resp);
        let usage = result.usage.expect("usage must parse");
        assert_eq!(usage.input_tokens, Some(10_000));
        assert_eq!(usage.output_tokens, Some(50));
        assert_eq!(usage.cache_creation_input_tokens, Some(8_000));
        assert_eq!(usage.cache_read_input_tokens, Some(1_500));
    }

    #[test]
    fn native_response_parses_without_usage() {
        let json = r#"{"content": [{"type": "text", "text": "Hello"}]}"#;
        let resp: NativeChatResponse = serde_json::from_str(json).unwrap();
        let result = AnthropicProvider::parse_native_response(resp);
        assert!(result.usage.is_none());
    }

    #[test]
    fn capabilities_reports_vision_and_native_tool_calling() {
        let provider = AnthropicProvider::new(Some("test-key"));
        let caps = provider.capabilities();
        assert!(caps.vision);
        assert!(caps.native_tool_calling);
    }

    /// True-streaming proof: mock SSE server sends `text_delta` events
    /// across multiple HTTP chunks. The streaming impl must deliver each
    /// text_delta to `on_token` BEFORE the response stream completes,
    /// AND still return the fully-assembled ChatResponse at the end.
    #[tokio::test]
    async fn chat_streaming_forwards_text_deltas_to_on_token() {
        use axum::{
            body::Body,
            response::{IntoResponse, Response},
            routing::post,
            Router,
        };
        use tokio::net::TcpListener;
        use tokio::sync::mpsc;

        // SSE events split across several body chunks so we exercise the
        // boundary-spanning logic in consume_sse_stream_with_tokens.
        let app = Router::new().route(
            "/v1/messages",
            post(|| async {
                let chunks: Vec<Result<&'static str, std::io::Error>> = vec![
                    Ok("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-7\",\"usage\":{\"input_tokens\":12}}}\n\n"),
                    Ok("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"),
                    Ok("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n"),
                    Ok("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo, \"}}\n\n"),
                    Ok("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"world!\"}}\n\n"),
                    Ok("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n"),
                    Ok("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n"),
                    Ok("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
                ];
                let stream = futures_util::stream::iter(chunks);
                let body = Body::from_stream(stream);
                Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
                    .body(body)
                    .unwrap()
                    .into_response()
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let provider = AnthropicProvider {
            credential: Some("test-key".to_string()),
            base_url: format!("http://{addr}"),
            max_tokens: DEFAULT_MAX_TOKENS,
        };

        let (tx, mut rx) = mpsc::channel::<String>(16);
        let messages = vec![ChatMessage::user("Greet me")];
        let req = ProviderChatRequest {
            messages: &messages,
            tools: None,
        };

        let response = provider
            .chat_streaming(req, "claude-opus-4-7", 0.0, Some(&tx))
            .await
            .expect("chat_streaming succeeds");

        // All 3 text_delta fragments arrived through on_token.
        let mut tokens = Vec::new();
        while let Ok(t) = rx.try_recv() {
            tokens.push(t);
        }
        assert_eq!(tokens, vec!["Hel", "lo, ", "world!"]);

        // Final assembled response still carries the full text.
        assert_eq!(response.text.as_deref(), Some("Hello, world!"));
        assert_eq!(response.usage.as_ref().unwrap().input_tokens, Some(12));
        assert_eq!(response.usage.as_ref().unwrap().output_tokens, Some(3));

        server.abort();
    }

    /// Default `chat_streaming` (no override) must not forward to
    /// `on_token` — only providers that explicitly opt in do. This
    /// guards against accidentally regressing the default-impl contract
    /// when the trait method's default body is edited.
    #[tokio::test]
    async fn chat_streaming_default_impl_does_not_invoke_on_token() {
        use crate::providers::traits::ToolsPayload;

        // Minimal Provider impl whose chat() returns a fixed string and
        // whose chat_streaming() inherits the default (no override).
        struct StubProvider;
        #[async_trait]
        impl Provider for StubProvider {
            async fn chat_with_system(
                &self,
                _system: Option<&str>,
                _message: &str,
                _model: &str,
                _temperature: f64,
            ) -> anyhow::Result<String> {
                Ok("stub".to_string())
            }
            fn convert_tools(&self, _tools: &[ToolSpec]) -> ToolsPayload {
                ToolsPayload::PromptGuided {
                    instructions: String::new(),
                }
            }
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(4);
        let messages = vec![ChatMessage::user("hi")];
        let req = ProviderChatRequest {
            messages: &messages,
            tools: None,
        };

        let response = StubProvider
            .chat_streaming(req, "stub-model", 0.0, Some(&tx))
            .await
            .unwrap();

        assert!(rx.try_recv().is_err(), "default impl must not push tokens");
        assert_eq!(response.text.as_deref(), Some("stub"));
    }
}
