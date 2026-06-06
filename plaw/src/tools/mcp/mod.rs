//! Model Context Protocol (MCP) client integration.
//!
//! Spawns one MCP server subprocess per `[[mcp.servers]]` entry at
//! agent startup, performs the JSON-RPC `initialize` →
//! `notifications/initialized` handshake, and exposes a single
//! `mcp_call(server, tool, arguments)` proxy tool to the agent loop.
//!
//! Phase 0 scope per the 2026-05-30 `mcp-client-discovery` workflow,
//! extended by PR #76 (2026-06-03):
//!
//! - **stdio + Streamable HTTP transports** — selected via
//!   `[mcp.servers.X.transport]` config (default stdio for back-compat).
//!   HTTP is sync request/response only; `text/event-stream` responses
//!   are rejected with a clear error pointing to PR #77.
//! - **Spec version `2025-06-18`** — advertised in `initialize`;
//!   plaw is lenient about server-returned versions.
//! - **`tools/list` + `tools/call`** — `resources`, `prompts`,
//!   `sampling`, `elicitation`, `roots` all skipped.
//! - **No OAuth** — stdio uses env vars; HTTP supports a static
//!   `bearer_token` only. Full OAuth 2.1 + PKCE deferred to PR #77.
//! - **Eager spawn**, **lazy reconnect** — failed servers don't block
//!   startup; the proxy tool's description surfaces their status.
//!
//! Layered architecture (one file per concern):
//!
//! ```text
//!     [protocol]                JSON-RPC envelopes, MCP-specific types
//!         ↑
//!     [transport/{stdio,http}]  wire framing + per-call correlation
//!         ↑
//!     [client]                  per-server orchestration (handshake,
//!                               list_tools, call_tool, timeouts)
//!         ↑
//!     [registry]                multi-server lifecycle, allow-list, status
//!         ↑
//!     [tool]                    McpTool — implements `Tool` trait
//! ```

pub mod client;
/// MCP OAuth 2.1 + PKCE foundations (PR #79). Dormant until PR #80
/// wires the ceremony into a CLI command and PR #81 plumbs the
/// 401-retry path through [`transport::http::HttpTransport`]. See
/// `oauth/mod.rs` for the layered shipping rationale.
pub(crate) mod oauth;
pub mod protocol;
pub mod registry;
pub mod tool;
pub(crate) mod transport;

pub use registry::{McpRegistry, ServerStatus};
pub use tool::McpTool;
