//! Transport abstraction for MCP clients.
//!
//! Decouples the JSON-RPC envelope semantics (handled by [`super::client::McpClient`])
//! from the byte-level wire (stdio subprocess in PR #63, Streamable HTTP added in
//! PR #76 Phase 0). Each transport handles its own correlation strategy:
//!
//! - [`stdio::StdioTransport`] keeps the per-id `oneshot` pending-map and the
//!   single background reader task that fans inbound JSON-RPC messages out to
//!   per-request channels.
//! - [`http::HttpTransport`] correlates trivially — one HTTP POST is one
//!   round-trip — so there is no pending-map and no reader task.
//!
//! Visibility is intentionally `pub(crate)`. Promote to `pub` only after a real
//! third transport (WebSocket / Unix domain socket / etc.) materialises and
//! survives one release per [CLAUDE.md §3.3 Rule of Three].

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub(crate) mod http;
pub(crate) mod sse;
pub(crate) mod stdio;

#[cfg(test)]
pub(crate) mod test_util;

/// Wire transport for an MCP server connection.
///
/// Implementations OWN their correlation state; callers see only the
/// per-call request/notify/close surface. Three operations cover every
/// caller in [`super::client::McpClient`]:
///
/// 1. [`Self::request`] — send a JSON-RPC request and await the matching
///    response. The response's `error` field (if present) is surfaced as
///    an [`super::client::McpProtocolError`] inside the returned `anyhow::Error`
///    so higher layers can `downcast_ref`. The success path returns the
///    raw `result` value.
/// 2. [`Self::notify`] — send a fire-and-forget JSON-RPC notification.
///    No response expected.
/// 3. [`Self::close`] — best-effort cleanup. Stdio closes its stdin and
///    kills the subprocess; HTTP is a no-op (reqwest pools connections
///    internally, dropping the [`http::HttpTransport`] releases them).
#[async_trait]
pub(crate) trait McpTransport: Send + Sync {
    /// Send a request and wait for the matching response. Returns the raw
    /// `result` field on success, or an error containing an
    /// [`super::client::McpProtocolError`] when the server returned a JSON-RPC
    /// error envelope, or a transport-level `anyhow::Error` for IO /
    /// timeout / parse failures.
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value>;

    /// Send a fire-and-forget notification. No correlation; no response.
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()>;

    /// Best-effort shutdown. Idempotent — safe to call from `Drop`-adjacent
    /// async cleanup paths. Stdio: closes stdin so the server sees EOF then
    /// waits for the child to exit. HTTP: cancels the per-server
    /// notification listener task (PR #85b) and awaits it with a 2 s
    /// timeout. The reqwest connection pool releases on Drop.
    async fn close(&self);

    /// PR #85b: opt-in MCP standalone GET notification stream.
    ///
    /// Stdio's default no-op is correct — stdio MCP servers already
    /// emit notifications inline on stdout via the existing reader
    /// task, so there is no separate "GET stream" concept. HTTP
    /// overrides this to spawn a per-server background task that
    /// issues `GET <url>` with `Accept: text/event-stream`, feeds the
    /// response through [`super::sse::SseParser`], and logs each
    /// notification at `debug` (or `info` for `notifications/message`).
    ///
    /// Server-initiated REQUESTS (`sampling/createMessage`,
    /// `elicitation/create`, `roots/list`) arriving on this stream get
    /// a JSON-RPC `-32601 Method not found` reply so the server does
    /// not deadlock; functional routing is a Phase 3b/3c concern.
    ///
    /// The listener is gated on BOTH (a) the caller passing a
    /// `capabilities` hint with at least one `*_list_changed` flag set
    /// — built from `initialize_result.capabilities` — AND (b) the
    /// per-server `enable_notifications` config flag (default OFF).
    /// Without either, this call is a silent no-op.
    async fn start_notification_listener(&self, _capabilities: NotificationCapability) {}
}

/// PR #85b: which list-changed signals the server advertised in its
/// `initialize` capabilities object. Built once by `McpClient` after
/// handshake and handed to the transport so the transport does not
/// have to know the `InitializeResult` shape.
///
/// `is_none()` returns `true` when the server advertised NO listChanged
/// flags — in that case the GET stream is pointless (the server has
/// nothing to push) and the listener spawn is skipped.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NotificationCapability {
    pub tools_list_changed: bool,
    pub prompts_list_changed: bool,
    pub resources_list_changed: bool,
}

impl NotificationCapability {
    pub(crate) fn is_none(&self) -> bool {
        !self.tools_list_changed && !self.prompts_list_changed && !self.resources_list_changed
    }
}
