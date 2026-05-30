//! Stdio MCP client.
//!
//! Spawns an MCP server subprocess, performs the `initialize` →
//! `notifications/initialized` handshake, and exposes
//! [`McpClient::list_tools`] and [`McpClient::call_tool`] for the
//! agent loop. Request/response correlation uses a monotonic `id`
//! counter; the read task fans inbound responses out to per-request
//! oneshot channels.
//!
//! **Framing**: one JSON-RPC object per line, UTF-8, no embedded
//! newlines (per spec). The reader uses `tokio::io::BufReader::lines`
//! which handles `\n` and `\r\n` transparently.
//!
//! **stderr**: captured and forwarded to `tracing::warn!` with the
//! server name prefix. Spec says servers MAY log to stderr.
//!
//! **Shutdown**: drop the [`McpClient`] → stdin closes → server's
//! read loop sees EOF → server exits gracefully. The background tasks
//! exit when their channels close.

use super::protocol::{
    CallToolResult, ClientCapabilities, ClientInfo, InitializeParams, InitializeResult,
    JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, ListToolsResult,
    ToolDescriptor, PROTOCOL_VERSION,
};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;

/// One MCP server connection.
///
/// Lifetime: created via [`McpClient::connect`] which spawns the
/// subprocess and runs the handshake; dropped to shut down.
/// Cloning is intentionally not implemented — the underlying stdin
/// handle is single-writer; share an `Arc<McpClient>` if multiple
/// callers need to issue requests concurrently.
pub struct McpClient {
    /// Server name (matches [`crate::config::McpServerConfig::name`]).
    server_name: String,
    /// Writer half of the subprocess stdin. Mutex serializes writes
    /// across concurrent `tools/call` invocations.
    stdin: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    /// Monotonic request id counter.
    next_id: AtomicU64,
    /// In-flight requests awaiting responses, keyed by id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    /// Cached `initialize` response (server capabilities + info).
    initialize_result: InitializeResult,
    /// Default per-request timeout.
    request_timeout: Duration,
    /// Process handle. Dropped → SIGKILL on Windows / kill() on Unix.
    /// Wrapped in Mutex<Option> so [`Self::shutdown`] can take ownership.
    child: Mutex<Option<Child>>,
}

impl McpClient {
    /// Spawn the configured subprocess, complete the MCP handshake,
    /// and return a ready-to-use client. Errors if the subprocess
    /// fails to spawn, exits early, or the handshake times out /
    /// returns a protocol error.
    pub async fn connect(
        server_name: impl Into<String>,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        startup_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self> {
        let server_name = server_name.into();
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn MCP server '{server_name}'"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("MCP server '{server_name}': no stdin pipe"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("MCP server '{server_name}': no stdout pipe"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("MCP server '{server_name}': no stderr pipe"))?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        spawn_stdout_reader(server_name.clone(), stdout, pending.clone());
        spawn_stderr_logger(server_name.clone(), stderr);

        let stdin_writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>> =
            Mutex::new(Box::new(stdin));

        let mut client = Self {
            server_name: server_name.clone(),
            stdin: stdin_writer,
            next_id: AtomicU64::new(1),
            pending: pending.clone(),
            // Filled in after handshake; placeholder used only between
            // construction and the handshake call below — never observed
            // by external code because `connect` either returns Ok with
            // a populated value or returns Err and the client is dropped.
            initialize_result: InitializeResult {
                protocol_version: String::new(),
                capabilities: Default::default(),
                server_info: Default::default(),
                instructions: None,
            },
            request_timeout,
            child: Mutex::new(Some(child)),
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
            self.request_internal("initialize", Some(params)),
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
        self.notify("notifications/initialized", None).await?;

        Ok(init)
    }

    /// List the server's advertised tools.
    pub async fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        let result_value = timeout(
            self.request_timeout,
            self.request_internal("tools/list", None),
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
            self.request_internal("tools/call", Some(params)),
        )
        .await
        .map_err(|_| anyhow!("tools/call '{tool_name}' timed out"))??;
        let r: CallToolResult = serde_json::from_value(result_value)
            .context("tools/call response was not a CallToolResult")?;
        Ok(r)
    }

    /// Send a request and wait for the matching response. Returns the
    /// raw `result` field on success or a protocol error on failure.
    async fn request_internal(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let req = JsonRpcRequest::new(id, method, params);
        let line = serde_json::to_string(&req)?;
        self.write_line(&line).await.with_context(|| {
            format!(
                "failed to write {} request to MCP server '{}'",
                method, self.server_name
            )
        })?;

        let response = match rx.await {
            Ok(m) => m,
            Err(_) => {
                // oneshot sender dropped → reader task ended without
                // receiving a response (typically: server crashed or
                // closed stdout). Surface a clear error.
                self.pending.lock().await.remove(&id);
                bail!(
                    "MCP server '{}' closed stdout before responding to {method}",
                    self.server_name
                );
            }
        };

        if let Some(err) = response.error {
            return Err(McpProtocolError::from(err).into());
        }
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// Send a notification (no response expected).
    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let n = JsonRpcNotification::new(method, params);
        let line = serde_json::to_string(&n)?;
        self.write_line(&line).await
    }

    async fn write_line(&self, json: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Cleanly close stdin, giving the server a chance to exit.
    /// Called from `Drop` indirectly via `kill_on_drop`; exposed for
    /// tests that want deterministic shutdown.
    pub async fn shutdown(&self) {
        // Drop stdin first so server sees EOF.
        let mut stdin = self.stdin.lock().await;
        let _ = stdin.shutdown().await;
        drop(stdin);

        let mut child_slot = self.child.lock().await;
        if let Some(mut child) = child_slot.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // `kill_on_drop(true)` on the Command builder handles the
        // hard cleanup. No async work is possible here.
    }
}

/// Wraps a server-returned [`JsonRpcError`] for ergonomic surfacing
/// via `anyhow::Error::downcast_ref` in higher layers.
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

fn spawn_stdout_reader<R: tokio::io::AsyncRead + Send + Unpin + 'static>(
    server_name: String,
    stdout: R,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                        Ok(msg) => {
                            if msg.is_response() {
                                if let Some(id) = msg.id {
                                    if let Some(tx) =
                                        pending.lock().await.remove(&id)
                                    {
                                        let _ = tx.send(msg);
                                    } else {
                                        tracing::warn!(
                                            server = %server_name,
                                            id,
                                            "MCP response for unknown request id (already timed out or never sent)"
                                        );
                                    }
                                }
                            } else if let Some(method) = msg.method.as_deref() {
                                tracing::debug!(
                                    server = %server_name,
                                    %method,
                                    "MCP notification received (ignored in Phase 0)"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                server = %server_name,
                                error = %e,
                                line = %truncate(trimmed, 200),
                                "MCP server emitted non-JSON-RPC line on stdout"
                            );
                        }
                    }
                }
                Ok(None) => {
                    // stdout closed — server exited or closed its
                    // stdout. Drop any pending senders so their
                    // recv() calls bail out.
                    tracing::info!(server = %server_name, "MCP server stdout closed");
                    pending.lock().await.clear();
                    return;
                }
                Err(e) => {
                    tracing::warn!(server = %server_name, error = %e, "MCP stdout read error");
                    pending.lock().await.clear();
                    return;
                }
            }
        }
    });
}

fn spawn_stderr_logger<R: tokio::io::AsyncRead + Send + Unpin + 'static>(
    server_name: String,
    stderr: R,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            tracing::warn!(server = %server_name, "[stderr] {}", truncate(trimmed, 500));
        }
    });
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// `ChildStdin` requires explicit Unpin impl chain for the boxed
// AsyncWrite alias; assert it here to fail fast at compile time if
// the underlying tokio types ever lose it.
const _: fn() = || {
    fn _check<T: AsyncWrite + Unpin>() {}
    _check::<ChildStdin>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::protocol::ContentBlock;

    // The tests below drive McpClient against a hand-rolled
    // in-process MCP server that runs in a tokio task and exchanges
    // JSON-RPC over `tokio::io::DuplexStream`. This skips the
    // subprocess machinery (Command::spawn etc.) and exercises only
    // the framing + request/response correlation logic — i.e. the
    // parts where bugs would actually live. End-to-end subprocess
    // tests against `npx @modelcontextprotocol/server-everything`
    // are deferred to a follow-up PR with offline-mode toggle.

    use std::collections::HashMap;
    use tokio::io::{duplex, AsyncWriteExt, BufReader};
    use tokio::sync::Mutex;

    /// Construct an McpClient hooked to in-memory duplex streams.
    /// `server_reader` is the half the test harness reads (client's
    /// outbound writes appear here); `server_writer` is the half the
    /// test harness writes to (visible to client as inbound).
    async fn build_test_client(
        request_timeout: Duration,
    ) -> (
        McpClient,
        tokio::io::ReadHalf<tokio::io::DuplexStream>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        let (client_side, server_side) = duplex(8192);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (server_read, server_write) = tokio::io::split(server_side);

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        spawn_stdout_reader("test".to_string(), client_read, pending.clone());

        let stdin_writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>> =
            Mutex::new(Box::new(client_write));
        let client = McpClient {
            server_name: "test".into(),
            stdin: stdin_writer,
            next_id: AtomicU64::new(1),
            pending,
            initialize_result: InitializeResult {
                protocol_version: PROTOCOL_VERSION.into(),
                capabilities: Default::default(),
                server_info: Default::default(),
                instructions: None,
            },
            request_timeout,
            child: Mutex::new(None),
        };
        (client, server_read, server_write)
    }

    async fn read_one_line(
        reader: &mut tokio::io::ReadHalf<tokio::io::DuplexStream>,
    ) -> String {
        let mut buf = BufReader::new(reader);
        let mut line = String::new();
        tokio::io::AsyncBufReadExt::read_line(&mut buf, &mut line)
            .await
            .unwrap();
        line.trim().to_string()
    }

    #[tokio::test]
    async fn list_tools_round_trip() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(5)).await;

        // Server task: wait for tools/list, respond with one tool.
        let server = tokio::spawn(async move {
            let req_line = read_one_line(&mut server_read).await;
            let req: serde_json::Value = serde_json::from_str(&req_line).unwrap();
            assert_eq!(req["method"], "tools/list");
            let id = req["id"].as_u64().unwrap();
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {"name": "echo", "description": "Echo a string",
                         "inputSchema": {"type": "object"}}
                    ]
                }
            });
            server_write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        });

        let tools = client.list_tools().await.unwrap();
        server.await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description.as_deref(), Some("Echo a string"));
    }

    #[tokio::test]
    async fn call_tool_returns_text_content() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(5)).await;

        let server = tokio::spawn(async move {
            let req_line = read_one_line(&mut server_read).await;
            let req: serde_json::Value = serde_json::from_str(&req_line).unwrap();
            assert_eq!(req["method"], "tools/call");
            assert_eq!(req["params"]["name"], "echo");
            let id = req["id"].as_u64().unwrap();
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": "echoed: ping"}]
                }
            });
            server_write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        });

        let result = client
            .call_tool("echo", serde_json::json!({"msg": "ping"}))
            .await
            .unwrap();
        server.await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "echoed: ping"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn call_tool_surfaces_is_error_flag() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(5)).await;
        let server = tokio::spawn(async move {
            let req_line = read_one_line(&mut server_read).await;
            let req: serde_json::Value = serde_json::from_str(&req_line).unwrap();
            let id = req["id"].as_u64().unwrap();
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type":"text","text":"rate limit"}],
                    "isError": true
                }
            });
            server_write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        });

        let result = client
            .call_tool("api_call", serde_json::json!({}))
            .await
            .unwrap();
        server.await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn call_tool_propagates_jsonrpc_error_as_anyhow() {
        let (client, mut server_read, mut server_write) =
            build_test_client(Duration::from_secs(5)).await;
        let server = tokio::spawn(async move {
            let req_line = read_one_line(&mut server_read).await;
            let req: serde_json::Value = serde_json::from_str(&req_line).unwrap();
            let id = req["id"].as_u64().unwrap();
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "Method not found"}
            });
            server_write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        });

        let err = client
            .call_tool("does_not_exist", serde_json::json!({}))
            .await
            .unwrap_err();
        server.await.unwrap();
        let msg = err.to_string();
        assert!(msg.contains("Method not found"));
    }

    #[tokio::test]
    async fn request_times_out_when_server_silent() {
        let (client, _server_read, _server_write) =
            build_test_client(Duration::from_millis(50)).await;
        // Don't have the test harness respond — request should time out.
        let err = client.list_tools().await.unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn server_closing_stdout_surfaces_clean_error() {
        let (client, server_read, server_write) =
            build_test_client(Duration::from_secs(5)).await;
        // Drop the write half — client's reader sees EOF, pending
        // requests should resolve to an error.
        drop(server_write);
        // Drop the read half too so any client write fails quickly.
        drop(server_read);
        // Give the reader task a tick to observe EOF and clear `pending`.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let err = client.list_tools().await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("closed stdout") || msg.contains("MCP server"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn concurrent_requests_correlate_correctly() {
        // Two requests in flight at once. Reply out of order. The
        // client's per-id correlation must route them correctly.
        let (client, server_read, mut server_write) =
            build_test_client(Duration::from_secs(5)).await;
        let server = tokio::spawn(async move {
            // Long-lived BufReader so we don't lose buffered bytes
            // between successive line reads.
            let mut server_buf = BufReader::new(server_read);
            let mut req1 = String::new();
            tokio::io::AsyncBufReadExt::read_line(&mut server_buf, &mut req1)
                .await
                .unwrap();
            let req1 = req1.trim().to_string();
            let mut req2 = String::new();
            tokio::io::AsyncBufReadExt::read_line(&mut server_buf, &mut req2)
                .await
                .unwrap();
            let req2 = req2.trim().to_string();
            let r1: serde_json::Value = serde_json::from_str(&req1).unwrap();
            let r2: serde_json::Value = serde_json::from_str(&req2).unwrap();
            let id1 = r1["id"].as_u64().unwrap();
            let id2 = r2["id"].as_u64().unwrap();
            // Reply to req2 FIRST.
            let resp2 = serde_json::json!({
                "jsonrpc":"2.0","id":id2,
                "result":{"content":[{"type":"text","text":"second"}]}
            });
            server_write
                .write_all(format!("{resp2}\n").as_bytes())
                .await
                .unwrap();
            let resp1 = serde_json::json!({
                "jsonrpc":"2.0","id":id1,
                "result":{"content":[{"type":"text","text":"first"}]}
            });
            server_write
                .write_all(format!("{resp1}\n").as_bytes())
                .await
                .unwrap();
        });

        let client_arc = Arc::new(client);
        let c1 = client_arc.clone();
        let h1 = tokio::spawn(async move {
            c1.call_tool("t1", serde_json::json!({})).await
        });
        let c2 = client_arc.clone();
        let h2 = tokio::spawn(async move {
            c2.call_tool("t2", serde_json::json!({})).await
        });
        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();
        server.await.unwrap();
        assert_eq!(r1.content[0].render(), "first");
        assert_eq!(r2.content[0].render(), "second");
    }
}
