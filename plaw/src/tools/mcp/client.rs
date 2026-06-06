//! Per-server MCP client orchestrator.
//!
//! Transport-agnostic — the byte-level wire (stdio subprocess or Streamable
//! HTTP) lives in [`super::transport`] behind the
//! [`super::transport::McpTransport`] trait. This module owns:
//!
//! - the `initialize` → `notifications/initialized` handshake
//! - per-call timeout enforcement (delegated to the transport for
//!   correlation but wrapped here for spec compliance)
//! - the `list_tools` / `call_tool` public API consumed by [`super::tool`]
//!
//! Per CLAUDE.md §3.3 Rule of Three the [`super::transport::McpTransport`]
//! trait stays `pub(crate)` until a third transport (WebSocket / Unix
//! domain socket / etc.) materialises and survives one release.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

use super::protocol::{
    CallToolResult, ClientCapabilities, ClientInfo, InitializeParams, InitializeResult,
    JsonRpcError, ListToolsResult, ToolDescriptor, PROTOCOL_VERSION,
};
use super::transport::{http::HttpTransport, stdio::StdioTransport, McpTransport};
use crate::security::{Secret, SecretStore};

/// One MCP server connection.
///
/// Lifetime: created via [`McpClient::connect`] (stdio) or
/// [`McpClient::connect_http`] (HTTP), both of which run the handshake
/// before returning. Dropped to shut down; transports handle their own
/// cleanup in [`McpTransport::close`].
///
/// Cloning is intentionally not implemented — concurrent callers should
/// share an `Arc<McpClient>`.
pub struct McpClient {
    server_name: String,
    transport: Box<dyn McpTransport>,
    initialize_result: InitializeResult,
    request_timeout: Duration,
}

impl McpClient {
    /// Spawn a stdio MCP subprocess, complete the handshake, return a
    /// ready-to-use client.
    pub async fn connect(
        server_name: impl Into<String>,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        startup_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self> {
        let server_name = server_name.into();
        let transport = StdioTransport::spawn(server_name.clone(), command, args, env).await?;
        Self::with_transport(
            server_name,
            Box::new(transport),
            startup_timeout,
            request_timeout,
        )
        .await
    }

    /// Build an HTTP MCP client, complete the handshake (POST `initialize`
    /// → POST `notifications/initialized`), return a ready-to-use client.
    pub async fn connect_http(
        server_name: impl Into<String>,
        url: &str,
        bearer_token: Option<&Secret>,
        headers: &HashMap<String, String>,
        startup_timeout: Duration,
        request_timeout: Duration,
        secret_store: &SecretStore,
    ) -> Result<Self> {
        let server_name = server_name.into();
        let transport = HttpTransport::connect(
            server_name.clone(),
            url,
            bearer_token,
            headers,
            request_timeout,
            secret_store,
        )?;
        Self::with_transport(
            server_name,
            Box::new(transport),
            startup_timeout,
            request_timeout,
        )
        .await
    }

    /// Transport-agnostic constructor — runs the handshake. Exposed at
    /// `pub(crate)` so future transports (and the test harness) can
    /// reuse it.
    pub(crate) async fn with_transport(
        server_name: String,
        transport: Box<dyn McpTransport>,
        startup_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self> {
        let mut client = Self {
            server_name: server_name.clone(),
            transport,
            initialize_result: InitializeResult {
                protocol_version: String::new(),
                capabilities: Default::default(),
                server_info: Default::default(),
                instructions: None,
            },
            request_timeout,
        };
        let init = client
            .handshake(startup_timeout)
            .await
            .with_context(|| format!("MCP server '{server_name}' handshake failed"))?;
        client.initialize_result = init;
        Ok(client)
    }

    /// Server name passed at construction time.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Result of the `initialize` handshake. Useful for surfacing
    /// `server_info` + `instructions` in the proxy tool's description.
    pub fn initialize_result(&self) -> &InitializeResult {
        &self.initialize_result
    }

    async fn handshake(&self, startup_timeout: Duration) -> Result<InitializeResult> {
        // 1) initialize request → expect InitializeResult.
        let params = serde_json::to_value(InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "plaw",
                version: env!("CARGO_PKG_VERSION"),
            },
        })?;
        let result_value = timeout(
            startup_timeout,
            self.transport.request("initialize", Some(params)),
        )
        .await
        .map_err(|_| anyhow!("handshake timed out after {startup_timeout:?}"))??;

        let init: InitializeResult = serde_json::from_value(result_value)
            .context("server's initialize response was not an InitializeResult")?;

        if init.protocol_version.is_empty() {
            bail!("server's initialize response is missing protocolVersion");
        }
        if init.protocol_version != PROTOCOL_VERSION {
            tracing::warn!(
                server = %self.server_name,
                client_version = PROTOCOL_VERSION,
                server_version = %init.protocol_version,
                "MCP protocol version mismatch (continuing — most variants are wire-compatible)"
            );
        }

        // 2) notifications/initialized — fire and forget.
        self.transport
            .notify("notifications/initialized", None)
            .await?;

        Ok(init)
    }

    /// List the server's advertised tools.
    pub async fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        let result_value = timeout(
            self.request_timeout,
            self.transport.request("tools/list", None),
        )
        .await
        .map_err(|_| anyhow!("tools/list timed out after {:?}", self.request_timeout))??;
        let r: ListToolsResult = serde_json::from_value(result_value)
            .context("tools/list response was not a ListToolsResult")?;
        Ok(r.tools)
    }

    /// Call a tool by name with arbitrary JSON arguments. Returns the
    /// parsed [`CallToolResult`] — caller is responsible for checking
    /// `is_error` and rendering content blocks.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallToolResult> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });
        let result_value = timeout(
            self.request_timeout,
            self.transport.request("tools/call", Some(params)),
        )
        .await
        .map_err(|_| anyhow!("tools/call '{tool_name}' timed out"))??;
        let r: CallToolResult = serde_json::from_value(result_value)
            .context("tools/call response was not a CallToolResult")?;
        Ok(r)
    }

    /// Cleanly close the transport.
    pub async fn shutdown(&self) {
        self.transport.close().await;
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Stdio: `kill_on_drop(true)` on the Command builder handles hard
        // cleanup inside StdioTransport. HTTP: no persistent state to
        // clean — dropping the reqwest::Client releases pooled sockets.
    }
}

/// Wraps a server-returned [`JsonRpcError`] for ergonomic surfacing via
/// `anyhow::Error::downcast_ref` in higher layers. Both stdio and HTTP
/// transports route server-level errors through this type.
#[derive(Debug)]
pub struct McpProtocolError(pub JsonRpcError);

impl From<JsonRpcError> for McpProtocolError {
    fn from(e: JsonRpcError) -> Self {
        Self(e)
    }
}

impl std::fmt::Display for McpProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for McpProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::transport::test_util::pair_with_duplex;
    use serde_json::json;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // ── Test harness ──────────────────────────────────────────────

    /// Build an `McpClient` wired to an in-memory transport. The
    /// returned `server_read`/`server_write` halves let the test act
    /// as the fake MCP server.
    async fn build_test_client(
        request_timeout: Duration,
    ) -> (
        McpClient,
        tokio::io::ReadHalf<tokio::io::DuplexStream>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        let (transport, mut server_read, mut server_write) =
            pair_with_duplex("test-server", request_timeout, 4096);

        // Spawn the client-side connect concurrently with the test-side
        // handshake responder so the in-memory pipe doesn't deadlock.
        let client_fut = tokio::spawn(async move {
            McpClient::with_transport(
                "test-server".into(),
                Box::new(transport),
                Duration::from_secs(5),
                request_timeout,
            )
            .await
        });

        // Read the initialize request, respond with a canned
        // InitializeResult, then consume the notifications/initialized.
        let _init_line = read_one_line(&mut server_read).await;
        let init_result = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "serverInfo": {"name": "test-server", "version": "0.0.1"}
            }
        });
        server_write
            .write_all(format!("{init_result}\n").as_bytes())
            .await
            .unwrap();
        let _initialized_line = read_one_line(&mut server_read).await;

        let client = client_fut.await.unwrap().unwrap();
        (client, server_read, server_write)
    }

    async fn read_one_line(reader: &mut tokio::io::ReadHalf<tokio::io::DuplexStream>) -> String {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte).await {
                Ok(0) => return String::from_utf8_lossy(&buf).to_string(),
                Ok(_) => {
                    if byte[0] == b'\n' {
                        return String::from_utf8_lossy(&buf).to_string();
                    }
                    buf.push(byte[0]);
                }
                Err(_) => return String::from_utf8_lossy(&buf).to_string(),
            }
        }
    }

    // ── Behavior coverage ────────────────────────────────────────

    #[tokio::test]
    async fn list_tools_round_trip() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(2)).await;

        // Client.list_tools sends `tools/list`; respond with two tools.
        let list_fut = tokio::spawn(async move { client.list_tools().await });

        let req_line = read_one_line(&mut server_read).await;
        assert!(req_line.contains("\"method\":\"tools/list\""));

        let response = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [
                    {"name": "echo", "description": "echo", "inputSchema": {"type": "object"}},
                    {"name": "ping", "description": "ping", "inputSchema": {"type": "object"}}
                ]
            }
        });
        server_write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();

        let tools = list_fut.await.unwrap().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[1].name, "ping");
    }

    #[tokio::test]
    async fn out_of_order_responses_are_correlated_by_id() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(2)).await;
        let client = std::sync::Arc::new(client);

        // Fire two concurrent calls; server responds in REVERSE order.
        let c1 = client.clone();
        let call1 = tokio::spawn(async move { c1.call_tool("echo", json!({"v": 1})).await });
        let c2 = client.clone();
        let call2 = tokio::spawn(async move { c2.call_tool("echo", json!({"v": 2})).await });

        let line1 = read_one_line(&mut server_read).await;
        let line2 = read_one_line(&mut server_read).await;
        // The transport allocates monotonic ids starting at 1 (next_id
        // post-handshake). Initialize used id=1, so these are 2 and 3.
        assert!(line1.contains("\"id\":2"));
        assert!(line2.contains("\"id\":3"));

        // Respond to id=3 FIRST.
        let r3 = json!({"jsonrpc": "2.0", "id": 3, "result": {"content": [{"type": "text", "text": "got 2"}], "isError": false}});
        server_write
            .write_all(format!("{r3}\n").as_bytes())
            .await
            .unwrap();
        let r2 = json!({"jsonrpc": "2.0", "id": 2, "result": {"content": [{"type": "text", "text": "got 1"}], "isError": false}});
        server_write
            .write_all(format!("{r2}\n").as_bytes())
            .await
            .unwrap();

        let result1 = call1.await.unwrap().unwrap();
        let result2 = call2.await.unwrap().unwrap();
        assert_eq!(result1.content.len(), 1);
        assert_eq!(result2.content.len(), 1);
    }

    #[tokio::test]
    async fn call_tool_returns_protocol_error_on_server_error_envelope() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(2)).await;

        let call_fut = tokio::spawn(async move { client.call_tool("bad", json!({})).await });

        let _req_line = read_one_line(&mut server_read).await;
        let err_resp = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "error": {"code": -32601, "message": "method not found"}
        });
        server_write
            .write_all(format!("{err_resp}\n").as_bytes())
            .await
            .unwrap();

        let result = call_fut.await.unwrap();
        let err = result.unwrap_err();
        let proto = err
            .downcast_ref::<McpProtocolError>()
            .expect("error must be McpProtocolError");
        assert_eq!(proto.0.code, -32601);
    }

    #[tokio::test]
    async fn call_tool_times_out_when_server_silent() {
        let (client, mut server_read, _server_write) =
            build_test_client(Duration::from_millis(150)).await;

        let result = client.call_tool("slow", json!({})).await;
        // Drain the request so the test doesn't leak.
        let _ = read_one_line(&mut server_read).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn list_tools_surfaces_closed_stdout_error_on_eof_before_response() {
        let (client, mut server_read, server_write) =
            build_test_client(Duration::from_secs(2)).await;

        let list_fut = tokio::spawn(async move { client.list_tools().await });
        let _req_line = read_one_line(&mut server_read).await;
        // Drop the writer half → reader task sees EOF → pending clears.
        drop(server_write);

        let result = list_fut.await.unwrap();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("closed stdout") || msg.contains("EOF"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn malformed_json_line_is_warned_about_but_does_not_kill_reader() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(2)).await;

        let list_fut = tokio::spawn(async move { client.list_tools().await });
        let _req_line = read_one_line(&mut server_read).await;

        // Send garbage first, then a valid response. The reader must
        // recover and route the valid response.
        server_write
            .write_all(b"this is not json at all\n")
            .await
            .unwrap();
        let good = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {"tools": []}
        });
        server_write
            .write_all(format!("{good}\n").as_bytes())
            .await
            .unwrap();

        let tools = list_fut.await.unwrap().unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn concurrent_calls_do_not_interleave_writes_on_stdin() {
        // Stress the stdin Mutex — fire N concurrent calls, assert each
        // line read by the server is a complete JSON object.
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(2)).await;
        let client = std::sync::Arc::new(client);
        const N: usize = 8;

        let mut handles = Vec::new();
        for i in 0..N {
            let c = client.clone();
            handles.push(tokio::spawn(async move {
                c.call_tool("echo", json!({"i": i})).await
            }));
        }

        // Read N lines and respond to each by id.
        for _ in 0..N {
            let line = read_one_line(&mut server_read).await;
            // Each line must parse cleanly as JSON.
            let v: serde_json::Value =
                serde_json::from_str(&line).expect("interleaved writes — line not valid JSON");
            let id = v["id"].as_u64().unwrap();
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"content": [{"type": "text", "text": "ok"}], "isError": false}
            });
            server_write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        }
        for h in handles {
            let _ = h.await.unwrap().unwrap();
        }
    }
}
