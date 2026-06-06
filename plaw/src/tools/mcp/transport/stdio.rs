//! Stdio JSON-RPC transport.
//!
//! Spawns an MCP server subprocess and exchanges newline-delimited JSON-RPC
//! envelopes over its stdin/stdout. One background tokio task reads stdout
//! and fans inbound responses out to per-request `oneshot` channels keyed
//! by the JSON-RPC `id`. Stderr is forwarded as `tracing::warn!`.
//!
//! Framing: one JSON-RPC object per line, UTF-8, no embedded newlines (per
//! spec). The reader uses `tokio::io::BufReader::lines` which handles `\n`
//! and `\r\n` transparently.
//!
//! Shutdown: closing stdin lets the server's read loop see EOF and exit
//! gracefully. If the server fails to exit, `kill_on_drop(true)` on the
//! `Command` builder enforces hard cleanup on `Drop`.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

use super::McpTransport;
use crate::tools::mcp::client::McpProtocolError;
use crate::tools::mcp::protocol::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest};

/// Stdio transport — owns the subprocess + its IO + the request
/// correlation table.
pub(crate) struct StdioTransport {
    server_name: String,
    /// Writer half of the subprocess stdin. Mutex serializes concurrent
    /// writers. Boxed `dyn AsyncWrite` so the test helper can plug in a
    /// `DuplexStream` instead of a real `ChildStdin`.
    stdin: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    /// Monotonic request id counter.
    next_id: AtomicU64,
    /// In-flight requests awaiting responses, keyed by id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    /// Process handle. Dropped → `kill_on_drop(true)` semantics. Mutex<Option>
    /// so `close()` can take ownership.
    child: Mutex<Option<Child>>,
}

impl StdioTransport {
    /// Spawn the subprocess + start the background reader/stderr tasks.
    /// Caller is responsible for performing the MCP `initialize` /
    /// `notifications/initialized` handshake via [`McpTransport::request`]
    /// / [`McpTransport::notify`] after construction.
    pub(crate) async fn spawn(
        server_name: String,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
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

        let stdin_writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>> = Mutex::new(Box::new(stdin));

        Ok(Self {
            server_name,
            stdin: stdin_writer,
            next_id: AtomicU64::new(1),
            pending,
            child: Mutex::new(Some(child)),
        })
    }

    /// Construct directly from `dyn AsyncWrite` + `dyn AsyncRead` halves.
    /// Used by the test harness in [`super::test_util`] to drive the
    /// transport over an in-memory `tokio::io::duplex` pair without
    /// spawning a real subprocess.
    #[cfg(test)]
    pub(crate) fn from_pipes<R>(
        server_name: String,
        stdin_write: Box<dyn AsyncWrite + Send + Unpin>,
        stdout_read: R,
    ) -> Self
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
    {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        spawn_stdout_reader(server_name.clone(), stdout_read, pending.clone());
        Self {
            server_name,
            stdin: Mutex::new(stdin_write),
            next_id: AtomicU64::new(1),
            pending,
            child: Mutex::new(None),
        }
    }

    async fn write_line(&self, json: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
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
        Ok(response.result.unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let n = JsonRpcNotification::new(method, params);
        let line = serde_json::to_string(&n)?;
        self.write_line(&line).await
    }

    async fn close(&self) {
        // Drop stdin first so the server sees EOF.
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
                                    if let Some(tx) = pending.lock().await.remove(&id) {
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
                    // stdout closed — server exited or closed its stdout.
                    // Drop any pending senders so their recv() calls bail.
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
