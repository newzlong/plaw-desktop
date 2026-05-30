//! Model Context Protocol (MCP) client integration.
//!
//! Spawns one MCP server subprocess per `[[mcp.servers]]` entry at
//! agent startup, performs the JSON-RPC `initialize` →
//! `notifications/initialized` handshake, and exposes a single
//! `mcp_call(server, tool, arguments)` proxy tool to the agent loop.
//!
//! Phase 0 scope per the 2026-05-30 `mcp-client-discovery` workflow:
//!
//! - **stdio transport only** — covers ~90% of public MCP servers
//!   (npx / uvx -shipped); HTTP / SSE deferred to Phase 1.
//! - **Spec version `2025-06-18`** — advertised in `initialize`;
//!   plaw is lenient about server-returned versions.
//! - **`tools/list` + `tools/call`** — `resources`, `prompts`,
//!   `sampling`, `elicitation`, `roots` all skipped.
//! - **No OAuth** — credentials reach the subprocess via env vars
//!   (spec explicitly says stdio servers SHOULD NOT do OAuth).
//! - **Eager spawn**, **lazy reconnect** — failed servers don't block
//!   startup; the proxy tool's description surfaces their status.
//!
//! Layered architecture (one file per concern, ~150-300 LOC each):
//!
//! ```text
//!     [protocol]      JSON-RPC envelopes, MCP-specific types
//!         ↑
//!     [client]        stdio framing, request/response correlation
//!         ↑
//!     [registry]      multi-server lifecycle, allow-list, status
//!         ↑
//!     [tool]          McpTool — implements `Tool` trait for the agent loop
//! ```

pub mod client;
pub mod protocol;
pub mod registry;
pub mod tool;

pub use registry::{McpRegistry, ServerStatus};
pub use tool::McpTool;
