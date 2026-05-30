//! JSON-RPC 2.0 envelope types for the Model Context Protocol (MCP)
//! `2025-06-18` spec.
//!
//! Three message shapes:
//! - **Request** — `{jsonrpc: "2.0", id, method, params?}`. The `id` is
//!   echoed back in the matching response.
//! - **Response** — `{jsonrpc: "2.0", id, result | error}`. Mutually
//!   exclusive `result` / `error`; the absent one is `serde_skip`'d.
//! - **Notification** — `{jsonrpc: "2.0", method, params?}`. No id, no
//!   response.
//!
//! This module intentionally defines its own minimal types instead of
//! depending on a third-party JSON-RPC crate — the spec surface plaw
//! actually needs (tools/list + tools/call + initialize +
//! notifications/initialized + notifications/cancelled) is small,
//! and a hand-rolled envelope keeps the deserialization story explicit
//! for adversarial-server defense.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version plaw advertises in the `initialize` request.
///
/// Servers MAY respond with a different version they support; clients
/// should disconnect on a wholly unsupported response. Plaw is lenient
/// — it accepts any non-empty version string the server returns and
/// logs a warning if it differs.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Outbound JSON-RPC request envelope.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> JsonRpcRequest<'a> {
    pub fn new(id: u64, method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

/// Outbound JSON-RPC notification (no id, no response expected).
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcNotification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> JsonRpcNotification<'a> {
    pub fn new(method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}

/// Inbound message — either a response to one of plaw's outbound
/// requests, or a notification from the server (e.g.
/// `notifications/tools/list_changed`). Plaw routes by presence of
/// the `id` field.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcMessage {
    #[allow(dead_code)]
    #[serde(default)]
    pub jsonrpc: Option<String>,
    /// Present on responses, absent on notifications.
    #[serde(default)]
    pub id: Option<u64>,
    /// Present on notifications and server-initiated requests.
    #[serde(default)]
    pub method: Option<String>,
    /// Present on success responses and on notifications/requests.
    #[serde(default)]
    pub params: Option<Value>,
    /// Present on success responses.
    #[serde(default)]
    pub result: Option<Value>,
    /// Present on error responses.
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcMessage {
    /// Classify the message as a response (has `id`) or a server-side
    /// notification (has `method` but no `id`).
    pub fn is_response(&self) -> bool {
        self.id.is_some() && (self.result.is_some() || self.error.is_some())
    }
}

/// JSON-RPC error object embedded in a failed response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

// ── MCP-specific param/result shapes ─────────────────────────────

/// Params for the `initialize` request that opens every MCP session.
#[derive(Debug, Clone, Serialize)]
pub struct InitializeParams<'a> {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'a str,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo<'a>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ClientCapabilities {
    // Plaw Phase 0 declares no roots/sampling/elicitation since it
    // doesn't implement those server-initiated callbacks. Empty object
    // is valid per spec.
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo<'a> {
    pub name: &'a str,
    pub version: &'a str,
}

/// Result returned from the server's `initialize` response.
#[derive(Debug, Clone, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo", default)]
    pub server_info: ServerInfo,
    /// Optional free-text server instructions; surfaced in the proxy
    /// tool description so the LLM has context.
    #[serde(default)]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerCapabilities {
    /// Present when the server exposes tools. Plaw treats absence
    /// as "no tools" rather than an error — some MCP servers only
    /// expose resources, which plaw skips in Phase 0.
    #[serde(default)]
    pub tools: Option<ToolsCapability>,
    // resources / prompts / logging / completions / experimental
    // are intentionally not deserialized in Phase 0 — they're
    // ignored even when present.
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolsCapability {
    /// Whether the server emits `notifications/tools/list_changed`.
    /// Phase 0 ignores those notifications (tools are listed once at
    /// startup) — captured here for forward compatibility.
    #[serde(rename = "listChanged", default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

/// One entry from `tools/list` response.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for `tools/call` `arguments` of this tool.
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<Value>,
}

/// Result of `tools/list`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListToolsResult {
    #[serde(default)]
    pub tools: Vec<ToolDescriptor>,
    #[serde(rename = "nextCursor", default)]
    pub next_cursor: Option<String>,
}

/// Result of `tools/call`. The MCP spec uses `isError` (not a
/// JSON-RPC `error`) to signal *business* failures — the protocol
/// call succeeded but the tool returned a failure. Plaw must check
/// both layers to surface accurate errors to the LLM.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

/// One content block in a `tools/call` response. Plaw Phase 0 extracts
/// text-typed blocks and concatenates them; other types are
/// stringified to a `[non-text content type=X mime=Y bytes=N]`
/// placeholder so the LLM at least knows something arrived.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        #[serde(default)]
        data: String,
        #[serde(rename = "mimeType", default)]
        mime_type: String,
    },
    Audio {
        #[serde(default)]
        data: String,
        #[serde(rename = "mimeType", default)]
        mime_type: String,
    },
    /// Catch-all for `resource_link` / `resource` / future variants —
    /// rendered as a placeholder for Phase 0.
    #[serde(other)]
    Other,
}

impl ContentBlock {
    /// Render a content block as a single string for embedding in the
    /// proxy tool's text-shaped `ToolResult`. Non-text blocks degrade
    /// to a structured placeholder so the LLM is aware they exist
    /// without choking on raw base64.
    pub fn render(&self) -> String {
        match self {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::Image { mime_type, data } => format!(
                "[mcp:image mime={mime_type} bytes={}]",
                data.len()
            ),
            ContentBlock::Audio { mime_type, data } => format!(
                "[mcp:audio mime={mime_type} bytes={}]",
                data.len()
            ),
            ContentBlock::Other => "[mcp:non-text content]".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_with_jsonrpc_field() {
        let req = JsonRpcRequest::new(7, "tools/list", None);
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
        assert!(s.contains("\"id\":7"));
        assert!(s.contains("\"method\":\"tools/list\""));
        // params omitted when None
        assert!(!s.contains("\"params\""));
    }

    #[test]
    fn request_serializes_params_when_provided() {
        let req = JsonRpcRequest::new(
            1,
            "tools/call",
            Some(serde_json::json!({"name": "x", "arguments": {}})),
        );
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"params\""));
        assert!(s.contains("\"name\":\"x\""));
    }

    #[test]
    fn notification_serializes_without_id() {
        let n = JsonRpcNotification::new("notifications/initialized", None);
        let s = serde_json::to_string(&n).unwrap();
        assert!(!s.contains("\"id\""));
        assert!(s.contains("\"method\":\"notifications/initialized\""));
    }

    #[test]
    fn parse_success_response() {
        let body = r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#;
        let m: JsonRpcMessage = serde_json::from_str(body).unwrap();
        assert_eq!(m.id, Some(7));
        assert!(m.result.is_some());
        assert!(m.error.is_none());
        assert!(m.is_response());
    }

    #[test]
    fn parse_error_response() {
        let body = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"Method not found"}}"#;
        let m: JsonRpcMessage = serde_json::from_str(body).unwrap();
        assert_eq!(m.id, Some(7));
        assert!(m.error.is_some());
        let err = m.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn parse_server_notification() {
        let body =
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{}}"#;
        let m: JsonRpcMessage = serde_json::from_str(body).unwrap();
        assert!(m.id.is_none());
        assert_eq!(m.method.as_deref(), Some("notifications/tools/list_changed"));
        assert!(!m.is_response());
    }

    #[test]
    fn parse_initialize_result() {
        let body = r#"{
            "protocolVersion":"2025-06-18",
            "capabilities":{"tools":{"listChanged":true}},
            "serverInfo":{"name":"test-server","version":"0.1.0"},
            "instructions":"Use carefully."
        }"#;
        let r: InitializeResult = serde_json::from_str(body).unwrap();
        assert_eq!(r.protocol_version, "2025-06-18");
        assert!(r.capabilities.tools.is_some());
        assert!(r.capabilities.tools.unwrap().list_changed);
        assert_eq!(r.server_info.name, "test-server");
        assert_eq!(r.instructions.as_deref(), Some("Use carefully."));
    }

    #[test]
    fn parse_initialize_result_with_no_tools_capability() {
        // Some servers only expose resources; their capabilities.tools
        // is absent. Plaw must accept this gracefully.
        let body = r#"{
            "protocolVersion":"2024-11-05",
            "capabilities":{},
            "serverInfo":{"name":"x","version":"0.0.1"}
        }"#;
        let r: InitializeResult = serde_json::from_str(body).unwrap();
        assert!(r.capabilities.tools.is_none());
    }

    #[test]
    fn parse_list_tools_result() {
        let body = r#"{
            "tools":[
                {"name":"create_issue","description":"Create a GitHub issue",
                 "inputSchema":{"type":"object","properties":{"title":{"type":"string"}}}},
                {"name":"list_prs","title":"List PRs","description":"List pull requests"}
            ]
        }"#;
        let r: ListToolsResult = serde_json::from_str(body).unwrap();
        assert_eq!(r.tools.len(), 2);
        assert_eq!(r.tools[0].name, "create_issue");
        assert!(r.tools[0].input_schema.is_some());
        assert_eq!(r.tools[1].title.as_deref(), Some("List PRs"));
    }

    #[test]
    fn parse_call_tool_result_with_text_content() {
        let body = r#"{"content":[{"type":"text","text":"hello"}]}"#;
        let r: CallToolResult = serde_json::from_str(body).unwrap();
        assert!(!r.is_error);
        assert_eq!(r.content.len(), 1);
        match &r.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parse_call_tool_result_with_is_error_flag() {
        let body = r#"{"content":[{"type":"text","text":"oops"}],"isError":true}"#;
        let r: CallToolResult = serde_json::from_str(body).unwrap();
        assert!(r.is_error);
    }

    #[test]
    fn parse_call_tool_result_with_mixed_content() {
        let body = r#"{"content":[
            {"type":"text","text":"prelude"},
            {"type":"image","data":"AAAA","mimeType":"image/png"},
            {"type":"resource_link","uri":"file://x"}
        ]}"#;
        let r: CallToolResult = serde_json::from_str(body).unwrap();
        assert_eq!(r.content.len(), 3);
        assert_eq!(r.content[0].render(), "prelude");
        assert!(r.content[1].render().starts_with("[mcp:image"));
        assert_eq!(r.content[2].render(), "[mcp:non-text content]");
    }

    #[test]
    fn content_block_render_text_passthrough() {
        let c = ContentBlock::Text {
            text: "verbatim".into(),
        };
        assert_eq!(c.render(), "verbatim");
    }

    #[test]
    fn protocol_version_constant_matches_spec_string() {
        // Regression guard: bumping PROTOCOL_VERSION should be a
        // deliberate decision visible in PR review.
        assert_eq!(PROTOCOL_VERSION, "2025-06-18");
    }
}
