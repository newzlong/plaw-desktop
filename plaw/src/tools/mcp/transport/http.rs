//! Streamable HTTP transport (MCP spec 2025-06-18).
//!
//! Capability matrix:
//!
//! - ✅ Sync request/response via JSON POST.
//! - ✅ Static bearer token via `Authorization: Bearer ...` header.
//! - ✅ Custom headers (verbatim).
//! - ✅ Mandatory `MCP-Protocol-Version` echoed on every POST after the
//!   initial handshake.
//! - ✅ Server-issued `Mcp-Session-Id` captured on initialize, echoed
//!   on subsequent calls.
//! - ✅ OAuth 2.1 (PR #79–#81): PRM + AS metadata + DCR + PKCE +
//!   401-retry via `AuthService::get_valid_mcp_access_token`.
//! - ✅ `text/event-stream` response bodies (PR #83): when the server
//!   replies with SSE we stream `reqwest::Response::bytes_stream()`
//!   into [`super::sse::SseParser`], collect intermediate
//!   notifications/requests (logged at debug, not yet routed), and
//!   return on the first JSON-RPC response whose `id` matches.
//!   Multi-event streams (server progress notifications before the
//!   final response) are supported. `Last-Event-ID` is CAPTURED into
//!   `last_event_id` for future Phase 3 reconnect-resend, but Phase
//!   2a does NOT retransmit it.
//! - ❌ Standalone `GET` notification stream — `subscribe_notifications`
//!   is intentionally absent from the trait. Deferred to its own PR.
//! - ❌ `Last-Event-ID` resend on reconnect — field captured, no
//!   reconnect logic yet.
//! - ❌ `retry:` field honoring (reconnect timing). Deferred with
//!   the GET listener.
//! - ❌ Server-to-client *request* handling (sampling/createMessage,
//!   roots/list, elicitation/createMessage). Plaw cannot fulfill
//!   any of these — needs agent-loop integration design first.
//! - ❌ DELETE on shutdown (no persistent server-side state to clean).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;

use super::sse::SseParser;
use super::McpTransport;
use crate::security::{Secret, SecretStore};
use crate::tools::mcp::client::McpProtocolError;
use crate::tools::mcp::protocol::{
    JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, PROTOCOL_VERSION,
};

/// Hard cap on bytes we will buffer for one SSE response stream. 4 MiB
/// is large enough for any reasonable JSON-RPC response (the largest
/// MCP responses we've seen are tools/list at ~80 KiB) and small
/// enough that a hung/malicious server cannot exhaust memory.
const SSE_BUFFER_BYTE_CAP: usize = 4 * 1024 * 1024;

/// Wall-clock deadline for completing an SSE response. `reqwest`'s
/// per-request `.timeout()` only covers headers + buffered body, NOT
/// live-streamed body — without this, a server that opens an SSE
/// stream then never writes hangs the agent loop. 120 s matches the
/// existing `request_timeout_ms` default in `[mcp.servers].
/// request_timeout_ms`; a future PR could plumb the per-server
/// override.
const SSE_STREAM_DEADLINE: Duration = Duration::from_secs(120);

/// HTTP transport state. `reqwest::Client` is cheap to clone and pools
/// connections internally, so we keep one per server and let `Drop`
/// release pooled sockets without an explicit DELETE.
pub(crate) struct HttpTransport {
    server_name: String,
    url: String,
    http: reqwest::Client,
    /// Static bearer token from `[mcp.servers.X.transport.bearer_token]`
    /// (PR #76). Already revealed at construction time. NEVER swapped —
    /// when OAuth is configured, `oauth_bearer` takes precedence in
    /// `build_post`. Mutually exclusive with `oauth_*` fields per the
    /// config-schema check in PR #79.
    static_bearer: Option<String>,
    headers: HashMap<String, String>,
    /// Monotonic request id counter. HTTP correlates trivially (one POST
    /// = one round-trip) but the JSON-RPC `id` field is still mandatory.
    next_id: std::sync::atomic::AtomicU64,
    /// Captured `Mcp-Session-Id` response header from `initialize`. Echoed
    /// on every subsequent POST so the server can route to its per-session
    /// state. `None` if the server did not issue one (allowed by spec).
    session_id: Mutex<Option<String>>,
    /// Set to true after the `initialize` response was parsed. Subsequent
    /// POSTs include the `MCP-Protocol-Version` header per spec §2.
    handshake_complete: std::sync::atomic::AtomicBool,
    /// PR #81: OAuth-managed access token. Swapped on `attempt_oauth_recovery`
    /// after a 401. `tokio::sync::Mutex` (NOT `std::sync::Mutex`) so the
    /// hold can span the AuthService refresh + IdP roundtrip without
    /// blocking the runtime. `None` when OAuth is not configured for
    /// this server.
    oauth_bearer: tokio::sync::Mutex<Option<String>>,
    /// PR #81: handle to the AuthService used to refresh the OAuth token
    /// when a 401 is observed. `None` = Phase 0 behavior preserved
    /// byte-identically (the existing static-bearer-only flow).
    auth_service: Option<Arc<crate::auth::AuthService>>,
    /// PR #81: the MCP server name under which OAuth tokens are
    /// persisted in `auth-profiles.json` (as `provider = "mcp:<name>"`).
    /// `Some` iff `auth_service` is also `Some`. Set at construction time
    /// from `[[mcp.servers]] name`.
    oauth_server_name: Option<String>,
    /// PR #83: most recent `id:` field observed on a dispatched SSE
    /// event. Captured for future Phase 3 reconnect-resend (the spec
    /// expects the client to send `Last-Event-ID: <id>` on
    /// reconnect); Phase 2a does NOT retransmit. Hot-path-free:
    /// only written by the SSE response reader at most once per
    /// stream; never on the JSON path.
    last_event_id: tokio::sync::Mutex<Option<String>>,
}

impl HttpTransport {
    /// Build an `HttpTransport` without performing the MCP handshake.
    /// The handshake itself is driven by the surrounding `McpClient` via
    /// [`McpTransport::request`]; this constructor only validates inputs
    /// and pre-builds the `reqwest::Client` with the configured timeout.
    ///
    /// PR #81 adds two optional parameters at the end:
    /// - `auth_service`: when `Some`, OAuth recovery on 401 is enabled.
    ///   When `None`, the transport behaves byte-identically to PR #76
    ///   (Phase 0 fail-fast 401).
    /// - `oauth_server_name`: when `Some` (always together with
    ///   `auth_service`), this is the server identifier
    ///   `AuthService::get_valid_mcp_access_token` will look up.
    pub(crate) fn connect(
        server_name: String,
        url: &str,
        bearer_token: Option<&Secret>,
        headers: &HashMap<String, String>,
        request_timeout: Duration,
        secret_store: &SecretStore,
        auth_service: Option<Arc<crate::auth::AuthService>>,
        oauth_server_name: Option<String>,
    ) -> Result<Self> {
        let parsed: reqwest::Url = url
            .parse()
            .with_context(|| format!("MCP HTTP server '{server_name}': invalid url"))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            bail!(
                "MCP HTTP server '{server_name}': url must use http:// or https:// (got {})",
                parsed.scheme()
            );
        }

        let static_bearer = bearer_token
            .map(|s| s.reveal(secret_store))
            .transpose()
            .with_context(|| {
                format!("MCP HTTP server '{server_name}': bearer_token decrypt failed")
            })?;

        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .context("building reqwest client for MCP HTTP transport")?;

        // OAuth bearer starts empty — the first POST will hit the 401
        // path, refresh via `attempt_oauth_recovery`, and cache the
        // token in `oauth_bearer` for all subsequent requests. Eager
        // priming would require making `connect` async, which would
        // ripple through every test fixture; the cold-start
        // round-trip cost is one extra HTTP request per plaw boot
        // (acceptable per CLAUDE.md §3.1 KISS).
        Ok(Self {
            server_name,
            url: parsed.into(),
            http,
            static_bearer,
            headers: headers.clone(),
            next_id: std::sync::atomic::AtomicU64::new(1),
            session_id: Mutex::new(None),
            handshake_complete: std::sync::atomic::AtomicBool::new(false),
            last_event_id: tokio::sync::Mutex::new(None),
            oauth_bearer: tokio::sync::Mutex::new(None),
            auth_service,
            oauth_server_name,
        })
    }

    /// Build a `reqwest::RequestBuilder` with the per-call headers
    /// (Accept, optional MCP-Protocol-Version, optional Mcp-Session-Id,
    /// optional Authorization, plus any user-configured static headers).
    ///
    /// PR #81 OAuth precedence: when `oauth_bearer` holds a token, it
    /// wins over `static_bearer`. The two MUST be mutually exclusive
    /// at the config-schema layer (PR #79's
    /// `validate_transport_mutual_exclusivity`) but the precedence here
    /// is defence-in-depth for the case where a user manually edits
    /// `auth-profiles.json` while a static bearer is also set.
    async fn build_post(&self) -> reqwest::RequestBuilder {
        // Per spec §2 the client MUST advertise both response shapes.
        // We then reject text/event-stream after the fact (Phase 0).
        let mut rb = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json");
        if self
            .handshake_complete
            .load(std::sync::atomic::Ordering::Acquire)
        {
            rb = rb.header("MCP-Protocol-Version", PROTOCOL_VERSION);
        }
        if let Ok(guard) = self.session_id.lock() {
            if let Some(ref sid) = *guard {
                rb = rb.header("Mcp-Session-Id", sid);
            }
        }
        // OAuth takes precedence over static bearer. Both are tried
        // before the static-bearer fallback below.
        let oauth_guard = self.oauth_bearer.lock().await;
        if let Some(ref token) = *oauth_guard {
            rb = rb.header("Authorization", format!("Bearer {token}"));
        } else if let Some(ref token) = self.static_bearer {
            rb = rb.header("Authorization", format!("Bearer {token}"));
        }
        drop(oauth_guard);
        for (k, v) in &self.headers {
            rb = rb.header(k, v);
        }
        rb
    }

    /// PR #81: attempt to recover from a 401 by asking AuthService for
    /// a fresh access token, swapping it into `oauth_bearer`. Returns
    /// `Ok(())` when a token was obtained and stashed; `Err` when
    /// OAuth is not configured for this server (the caller falls
    /// through to the static-bearer error path) OR when the refresh
    /// itself failed (the caller surfaces the OAuth-configured error
    /// message pointing the user at `plaw auth login`).
    async fn attempt_oauth_recovery(&self) -> Result<()> {
        let (Some(svc), Some(server_name)) =
            (self.auth_service.as_ref(), self.oauth_server_name.as_ref())
        else {
            anyhow::bail!("no OAuth configured");
        };
        let token = svc
            .get_valid_mcp_access_token(server_name)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no MCP profile for '{server_name}'; run `plaw auth login --provider mcp:{server_name}`"
                )
            })?;
        let mut guard = self.oauth_bearer.lock().await;
        *guard = Some(token);
        Ok(())
    }

    /// PR #81: synthetic JSON-RPC error for 401/403 when OAuth IS
    /// configured but recovery failed (or the user is logged out).
    /// Replaces the Phase 0 "configure a static bearer" message with
    /// an actionable "run plaw auth login" pointer.
    fn http_status_to_error_oauth_aware(
        &self,
        status: u16,
        body: &str,
        oauth_configured: bool,
        recovery_error: Option<&str>,
    ) -> JsonRpcError {
        if !matches!(status, 401 | 403) || !oauth_configured {
            return Self::http_status_to_error(status, body);
        }
        let reason = recovery_error.unwrap_or("token endpoint refused the refresh request");
        let body_excerpt: String = body.chars().take(200).collect();
        let server_name = self.oauth_server_name.as_deref().unwrap_or("<unknown>");
        JsonRpcError {
            code: -32001,
            message: format!(
                "HTTP {status}: MCP OAuth recovery failed ({reason}). \
                 Run `plaw auth login --provider mcp:{server_name}` to re-authenticate. Body: {body_excerpt}"
            ),
            data: None,
        }
    }

    /// Map a non-2xx HTTP status to a synthetic JSON-RPC error envelope
    /// so the McpClient layer can surface it uniformly via
    /// `McpProtocolError`. 401/403 carry an explicit pointer to the
    /// missing OAuth implementation (PR #77).
    fn http_status_to_error(status: u16, body: &str) -> JsonRpcError {
        let body_excerpt: String = body.chars().take(200).collect();
        match status {
            401 | 403 => JsonRpcError {
                code: -32001,
                message: format!(
                    "HTTP {status}: server requires authorization; Phase 0 plaw does not implement OAuth. Configure a static bearer via [mcp.servers.X.transport.bearer_token] if the server supports it. Body: {body_excerpt}"
                ),
                data: None,
            },
            404 => JsonRpcError {
                code: -32002,
                message: format!("HTTP 404: endpoint not found (or session expired). Body: {body_excerpt}"),
                data: None,
            },
            500..=599 => JsonRpcError {
                code: -32003,
                message: format!("HTTP {status}: server error. Body: {body_excerpt}"),
                data: None,
            },
            _ => JsonRpcError {
                code: -32000,
                message: format!("HTTP {status}: transport error. Body: {body_excerpt}"),
                data: None,
            },
        }
    }

    /// Capture `Mcp-Session-Id` from the response headers if present.
    fn capture_session_id(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(val) = headers.get("mcp-session-id") {
            if let Ok(s) = val.to_str() {
                if let Ok(mut guard) = self.session_id.lock() {
                    *guard = Some(s.to_string());
                }
            }
        }
    }

    /// PR #83: drive an SSE response stream until we either find a
    /// JSON-RPC response matching `request_id` or hit one of the
    /// failure modes (stream EOF without a matching response,
    /// JSON-RPC error envelope, parser error, byte cap, wall-clock
    /// deadline, server-side error inside an event's data field).
    ///
    /// Intermediate events that decode as JSON-RPC notifications
    /// (`progress`, `message`, etc.) or server→client requests
    /// (`sampling/createMessage`, `roots/list`,
    /// `elicitation/createMessage`) are logged at `tracing::debug!`
    /// and discarded — wiring them into the agent loop is a Phase 3
    /// concern. Events whose `event:` field is set to anything other
    /// than `message` (or absent — spec default is `message`) are
    /// ignored entirely (e.g. `event: ping` keepalives).
    ///
    /// Errors NEVER set `handshake_complete`; the caller flips that
    /// flag only after `Ok(value)` returns.
    async fn read_sse_response(
        &self,
        response: reqwest::Response,
        method: &str,
        request_id: u64,
    ) -> Result<Value> {
        tokio::time::timeout(
            SSE_STREAM_DEADLINE,
            self.read_sse_response_inner(response, method, request_id),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "MCP HTTP server '{}': SSE stream for {method} did not produce a matching JSON-RPC response within {}s",
                self.server_name,
                SSE_STREAM_DEADLINE.as_secs()
            )
        })?
    }

    async fn read_sse_response_inner(
        &self,
        response: reqwest::Response,
        method: &str,
        request_id: u64,
    ) -> Result<Value> {
        use serde_json::Value as JsonValue;

        let mut parser = SseParser::new(SSE_BUFFER_BYTE_CAP);
        let mut byte_stream = response.bytes_stream();
        let request_id_value = JsonValue::from(request_id);

        loop {
            // Try to pull the next chunk. If the stream ends, run
            // parser.finish() to flush any bare-CR-pending event,
            // then bail because we never matched the request.
            let chunk = match byte_stream.next().await {
                Some(item) => item.with_context(|| {
                    format!(
                        "MCP HTTP server '{}': SSE stream read error",
                        self.server_name
                    )
                })?,
                None => {
                    let final_events = parser.finish().with_context(|| {
                        format!(
                            "MCP HTTP server '{}': SSE stream ended with malformed final event",
                            self.server_name
                        )
                    })?;
                    if let Some(value) =
                        self.match_sse_events(&final_events, &request_id_value, method)?
                    {
                        return Ok(value);
                    }
                    bail!(
                        "MCP HTTP server '{}': SSE stream ended before a JSON-RPC response with id={request_id} arrived for {method}",
                        self.server_name
                    );
                }
            };

            let events = parser.feed(&chunk).with_context(|| {
                format!("MCP HTTP server '{}': SSE parse error", self.server_name)
            })?;
            if let Some(value) = self.match_sse_events(&events, &request_id_value, method)? {
                // Capture last_event_id for future Phase 3 reconnect
                // resend BEFORE returning. Synchronous tokio::Mutex
                // lock is cheap — single-write-per-stream pattern.
                if let Some(id) = parser.last_event_id() {
                    let mut guard = self.last_event_id.lock().await;
                    *guard = Some(id.to_string());
                }
                return Ok(value);
            }
        }
    }

    /// Scan a batch of dispatched SSE events for a JSON-RPC response
    /// matching `request_id`. Returns:
    /// - `Ok(Some(value))` — matching response found; caller returns.
    /// - `Ok(None)` — no match yet; caller pulls more bytes.
    /// - `Err(_)` — a matching response carried a JSON-RPC error
    ///   envelope (`McpProtocolError`), or an event's `data:` field
    ///   was not valid JSON.
    fn match_sse_events(
        &self,
        events: &[super::sse::SseEvent],
        request_id_value: &Value,
        method: &str,
    ) -> Result<Option<Value>> {
        for event in events {
            // Filter on event name. Spec default is `message` when
            // the `event:` field is absent.
            let kind = event.event.as_deref().unwrap_or("message");
            if kind != "message" {
                // Keepalives, progress hints carried via custom
                // event names, etc. Log and skip.
                tracing::debug!(
                    server = %self.server_name,
                    method,
                    sse_event = kind,
                    "skipping non-message SSE event"
                );
                continue;
            }
            // Parse data as JSON-RPC. Malformed data here is fatal
            // — a spec-compliant server MUST send valid JSON-RPC
            // inside `data:` for `event: message`.
            let msg: JsonRpcMessage = match serde_json::from_str(&event.data) {
                Ok(m) => m,
                Err(e) => bail!(
                    "MCP HTTP server '{}': SSE event data was not a JSON-RPC message: {e}; data: {}",
                    self.server_name,
                    truncate(&event.data, 200)
                ),
            };

            // Determine kind:
            // - id == request_id → matching response
            // - id == null/absent → notification (log, skip)
            // - id == something else → server→client request
            //   (sampling/roots/elicitation — log, skip)
            let id_match = msg
                .id
                .as_ref()
                .map(|got| got == request_id_value)
                .unwrap_or(false);
            if id_match {
                if let Some(err) = msg.error {
                    return Err(McpProtocolError::from(err).into());
                }
                return Ok(Some(msg.result.unwrap_or(Value::Null)));
            }
            if msg.id.is_none() {
                tracing::debug!(
                    server = %self.server_name,
                    method,
                    notification_method = ?msg.method,
                    "SSE intermediate JSON-RPC notification (Phase 2a logs+discards; Phase 3 will route)"
                );
            } else {
                tracing::debug!(
                    server = %self.server_name,
                    method,
                    server_request_method = ?msg.method,
                    "SSE server-to-client request (sampling/roots/elicitation deferred to Phase 3)"
                );
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let req = JsonRpcRequest::new(id, method, params);
        let body = serde_json::to_vec(&req)?;
        // PR #81: at most ONE recovery attempt per request. A second
        // 401 after a successful refresh means something else is
        // wrong (scope mismatch, audience mismatch, server bug); fall
        // through to the error path rather than thrashing the IdP.
        let oauth_configured = self.auth_service.is_some();
        let mut already_retried = false;

        loop {
            let response = self
                .build_post()
                .await
                .body(body.clone())
                .send()
                .await
                .with_context(|| {
                    format!(
                        "MCP HTTP server '{}': POST {} failed",
                        self.server_name, method
                    )
                })?;

            let status = response.status();
            self.capture_session_id(response.headers());
            // Snapshot the content-type before consuming the response body so
            // we can branch on application/json vs text/event-stream cleanly.
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_lowercase();

            // PR #83: SSE response body. Fork BEFORE `.bytes().await`
            // so we stream incrementally. Gated on `status.is_success()`
            // so a 401 with text/event-stream content-type still flows
            // through the OAuth-recovery path below (defensive: spec
            // doesn't say the AS error page is required to be JSON).
            if status.is_success() && content_type.starts_with("text/event-stream") {
                let value = self.read_sse_response(response, method, id).await?;
                // PR #83: handshake_complete is set ONLY after a
                // successful JSON-RPC parse from the stream. A
                // malformed SSE stream returns Err above and leaves
                // handshake_complete=false so subsequent POSTs don't
                // start sending MCP-Protocol-Version against a server
                // that may not be in a valid handshake state.
                self.handshake_complete
                    .store(true, std::sync::atomic::Ordering::Release);
                return Ok(value);
            }

            let body_bytes = response.bytes().await.with_context(|| {
                format!("MCP HTTP server '{}': read body failed", self.server_name)
            })?;
            let body_str = String::from_utf8_lossy(&body_bytes);

            // PR #81: OAuth recovery on the FIRST 401 only.
            if status == reqwest::StatusCode::UNAUTHORIZED && oauth_configured && !already_retried {
                match self.attempt_oauth_recovery().await {
                    Ok(()) => {
                        already_retried = true;
                        tracing::info!(
                            server = %self.server_name,
                            method,
                            "OAuth token refreshed after 401; retrying request once"
                        );
                        continue;
                    }
                    Err(refresh_err) => {
                        let synthetic = self.http_status_to_error_oauth_aware(
                            status.as_u16(),
                            body_str.as_ref(),
                            true,
                            Some(refresh_err.to_string().as_str()),
                        );
                        return Err(McpProtocolError::from(synthetic).into());
                    }
                }
            }

            if !status.is_success() {
                // For 401/403 with OAuth configured: surface the
                // "run plaw auth login" guidance even on second
                // failure (so the user knows what to do).
                let synthetic = if oauth_configured {
                    self.http_status_to_error_oauth_aware(
                        status.as_u16(),
                        body_str.as_ref(),
                        true,
                        None,
                    )
                } else {
                    Self::http_status_to_error(status.as_u16(), body_str.as_ref())
                };
                return Err(McpProtocolError::from(synthetic).into());
            }

            let msg: JsonRpcMessage = serde_json::from_slice(&body_bytes).with_context(|| {
                format!(
                    "MCP HTTP server '{}': response was not a JSON-RPC message (body: {})",
                    self.server_name,
                    truncate(body_str.as_ref(), 200)
                )
            })?;

            if let Some(err) = msg.error {
                return Err(McpProtocolError::from(err).into());
            }
            // After the first successful response we treat the handshake as
            // complete so subsequent POSTs include MCP-Protocol-Version.
            self.handshake_complete
                .store(true, std::sync::atomic::Ordering::Release);
            return Ok(msg.result.unwrap_or(Value::Null));
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let n = JsonRpcNotification::new(method, params);
        let body = serde_json::to_vec(&n)?;

        let response = self
            .build_post()
            .await
            .body(body)
            .send()
            .await
            .with_context(|| {
                format!(
                    "MCP HTTP server '{}': POST notification {} failed",
                    self.server_name, method
                )
            })?;

        let status = response.status();
        self.capture_session_id(response.headers());

        // Notifications: spec §2 says server MAY accept (HTTP 202 / 204)
        // or stream a body (text/event-stream); the latter is Phase 1.
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable>"));
            return Err(anyhow!(
                "MCP HTTP server '{}': notification {} returned HTTP {}: {}",
                self.server_name,
                method,
                status,
                truncate(&body, 200)
            ));
        }
        Ok(())
    }

    async fn close(&self) {
        // No persistent connection state — dropping the reqwest::Client
        // releases pooled sockets. DELETE on Mcp-Session-Id is a Phase 1
        // nicety.
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::client::McpProtocolError;
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
        Router,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::net::TcpListener;

    /// Per-server request inspector. Tests can read what the HTTP
    /// transport ACTUALLY put on the wire — headers + body — to assert
    /// spec compliance and rule out secret leakage.
    #[derive(Default, Clone)]
    struct RequestRecorder(Arc<StdMutex<Vec<RecordedRequest>>>);

    #[derive(Clone, Debug)]
    struct RecordedRequest {
        headers: Vec<(String, String)>,
        body: String,
    }

    impl RequestRecorder {
        fn snapshot(&self) -> Vec<RecordedRequest> {
            self.0.lock().unwrap().clone()
        }
    }

    /// Captures the request, then returns whatever the test queued via
    /// `MockServerState::next_response`. Each request consumes one
    /// scripted response in FIFO order; if the queue runs out the
    /// server returns a 500.
    #[derive(Clone, Default)]
    struct MockServerState {
        recorder: RequestRecorder,
        responses: Arc<StdMutex<Vec<ScriptedResponse>>>,
    }

    #[derive(Clone)]
    struct ScriptedResponse {
        status: StatusCode,
        content_type: &'static str,
        body: String,
        extra_headers: Vec<(&'static str, String)>,
    }

    impl MockServerState {
        fn push(&self, r: ScriptedResponse) {
            self.responses.lock().unwrap().push(r);
        }

        fn json_ok(body: serde_json::Value) -> ScriptedResponse {
            ScriptedResponse {
                status: StatusCode::OK,
                content_type: "application/json",
                body: body.to_string(),
                extra_headers: Vec::new(),
            }
        }
    }

    async fn mock_handler(
        State(state): State<MockServerState>,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> impl IntoResponse {
        let hdrs: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_lowercase(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body_str = String::from_utf8_lossy(&body).to_string();
        state.recorder.0.lock().unwrap().push(RecordedRequest {
            headers: hdrs,
            body: body_str,
        });

        let resp = {
            let mut queue = state.responses.lock().unwrap();
            if queue.is_empty() {
                ScriptedResponse {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    content_type: "text/plain",
                    body: "no scripted response".into(),
                    extra_headers: Vec::new(),
                }
            } else {
                queue.remove(0)
            }
        };

        let mut response = axum::http::Response::builder()
            .status(resp.status)
            .header("Content-Type", resp.content_type);
        for (k, v) in &resp.extra_headers {
            response = response.header(*k, v);
        }
        response.body(axum::body::Body::from(resp.body)).unwrap()
    }

    async fn spawn_mock(state: MockServerState) -> (String, RequestRecorder) {
        let recorder = state.recorder.clone();
        let app = Router::new()
            .route("/", post(mock_handler))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/"), recorder)
    }

    fn empty_secret_store() -> SecretStore {
        SecretStore::new(std::path::Path::new(""), false)
    }

    fn make_transport(url: &str) -> HttpTransport {
        HttpTransport::connect(
            "test-http".into(),
            url,
            None,
            &HashMap::new(),
            Duration::from_secs(2),
            &empty_secret_store(),
            None,
            None,
        )
        .unwrap()
    }

    // ── Behavior coverage ───────────────────────────────────────────

    #[tokio::test]
    async fn http_request_happy_path_returns_result_value() {
        let state = MockServerState::default();
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"tools": [{"name": "ping", "description": "ok", "inputSchema": {"type":"object"}}]}
        })));
        let (url, _recorder) = spawn_mock(state).await;

        let t = make_transport(&url);
        let result = t.request("tools/list", None).await.unwrap();
        assert!(result["tools"][0]["name"] == "ping");
    }

    #[tokio::test]
    async fn http_401_maps_to_oauth_phase_marker_jsonrpc_error() {
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::UNAUTHORIZED,
            content_type: "text/plain",
            body: "auth required".into(),
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let err = t.request("tools/list", None).await.unwrap_err();
        let proto = err
            .downcast_ref::<McpProtocolError>()
            .expect("401 must surface as McpProtocolError");
        assert_eq!(proto.0.code, -32001);
        assert!(
            proto
                .0
                .message
                .contains("Phase 0 plaw does not implement OAuth"),
            "got: {}",
            proto.0.message
        );
    }

    #[tokio::test]
    async fn http_500_maps_to_server_error_code() {
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            content_type: "text/plain",
            body: "boom".into(),
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let err = t.request("tools/list", None).await.unwrap_err();
        let proto = err.downcast_ref::<McpProtocolError>().unwrap();
        assert_eq!(proto.0.code, -32003);
    }

    // ── PR #83: SSE response body parsing ──────────────────────────

    /// Canonical single-event SSE response. Server sends ONE
    /// `event: message` whose `data:` field is the full JSON-RPC
    /// response — exactly the shape produced by the official MCP SDKs
    /// with `enableJsonResponse=false` (the default for Notion,
    /// Linear, GitHub remote MCP). The transport must successfully
    /// decode and return the result value.
    #[tokio::test]
    async fn http_sse_response_single_message_event_returns_result() {
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: format!(
                "event: message\ndata: {}\n\n",
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"tools": [{"name": "shell"}]}
                })
            ),
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let result = t.request("tools/list", None).await.unwrap();
        assert_eq!(
            result,
            serde_json::json!({"tools": [{"name": "shell"}]}),
            "SSE-wrapped result must decode identically to JSON path"
        );
    }

    /// Server may send progress notifications BEFORE the response
    /// per MCP spec 2025-06-18. The transport must skip them and
    /// return only on the matching response.
    #[tokio::test]
    async fn http_sse_response_progress_then_response_extracts_response() {
        let state = MockServerState::default();
        let body = format!(
            "event: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {"progressToken": "tok-1", "progress": 0.5}
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"ok": true}
            }),
        );
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body,
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let result = t.request("tools/call", None).await.unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
    }

    /// `event: ping` keepalives (or any non-`message` event name)
    /// are not JSON-RPC payloads and must be ignored without
    /// affecting parse state.
    #[tokio::test]
    async fn http_sse_response_non_message_event_ignored() {
        let state = MockServerState::default();
        let body = format!(
            "event: ping\ndata: keepalive\n\nevent: message\ndata: {}\n\n",
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": "pong"})
        );
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body,
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let result = t.request("ping", None).await.unwrap();
        assert_eq!(result, serde_json::json!("pong"));
    }

    /// Malformed JSON inside an `event: message` data field surfaces
    /// as a clean error (NOT a silent hang, NOT a panic).
    #[tokio::test]
    async fn http_sse_response_malformed_data_field_returns_error() {
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: "event: message\ndata: {not-valid-json\n\n".into(),
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let err = t.request("anything", None).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not a JSON-RPC message"),
            "malformed SSE data must surface a JSON-RPC parse error; got: {msg}"
        );
    }

    /// Stream ending with only notifications and no response event
    /// must error (NOT hang, NOT return Null). Validates the EOF
    /// branch.
    #[tokio::test]
    async fn http_sse_response_stream_ends_before_matching_response_errors() {
        let state = MockServerState::default();
        let body = format!(
            "event: message\ndata: {}\n\n",
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/log",
                "params": {"level": "info"}
            })
        );
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body,
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let err = t.request("tools/list", None).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("SSE stream ended"),
            "stream-end-without-response must error explicitly; got: {msg}"
        );
    }

    /// `Mcp-Session-Id` header on an SSE response must round-trip
    /// identically to the JSON branch.
    #[tokio::test]
    async fn http_sse_response_session_id_round_trips() {
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: format!(
                "event: message\ndata: {}\n\n",
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": "ok"})
            ),
            extra_headers: vec![("Mcp-Session-Id".into(), "session-from-sse-42".into())],
        });
        // Second request: must echo the captured session id.
        state.push(MockServerState::json_ok(serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "result": "ok"
        })));
        let (url, recorder) = spawn_mock(state).await;

        let t = make_transport(&url);
        t.request("initialize", None).await.unwrap();
        t.request("ping", None).await.unwrap();

        let snap = recorder.snapshot();
        let sid = snap[1]
            .headers
            .iter()
            .find(|(k, _)| k == "mcp-session-id")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert_eq!(
            sid, "session-from-sse-42",
            "session id captured from SSE response must echo on subsequent POST"
        );
    }

    /// `handshake_complete` MUST NOT be set if the SSE response
    /// errors out. Subsequent POSTs would otherwise send
    /// `MCP-Protocol-Version` against a server that may not be in
    /// a valid handshake state.
    #[tokio::test]
    async fn http_sse_response_handshake_complete_set_only_after_success() {
        let state = MockServerState::default();
        // First response: malformed JSON-RPC inside SSE → error.
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: "event: message\ndata: not-valid-json\n\n".into(),
            extra_headers: Vec::new(),
        });
        // Second response: well-formed.
        state.push(MockServerState::json_ok(serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "result": "ok"
        })));
        let (url, recorder) = spawn_mock(state).await;

        let t = make_transport(&url);
        let _ = t.request("initialize", None).await.unwrap_err();
        let _ = t.request("ping", None).await.unwrap();

        let snap = recorder.snapshot();
        // After the FIRST (failed) request, handshake_complete is
        // still false → the SECOND request must NOT carry
        // MCP-Protocol-Version. This validates the lens A risk #4
        // invariant.
        let pv = snap[1]
            .headers
            .iter()
            .find(|(k, _)| k == "mcp-protocol-version")
            .map(|(_, v)| v.as_str());
        assert!(
            pv.is_none(),
            "MCP-Protocol-Version must NOT be sent after a failed first request; got: {pv:?}"
        );
    }

    #[tokio::test]
    async fn mcp_session_id_is_captured_and_echoed_on_subsequent_request() {
        let state = MockServerState::default();
        // First response carries Mcp-Session-Id.
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "application/json",
            body: json!({"jsonrpc": "2.0", "id": 1, "result": {}}).to_string(),
            extra_headers: vec![("Mcp-Session-Id", "abc-session-123".into())],
        });
        // Second response is plain — but we'll inspect what we SENT.
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 2, "result": {}
        })));
        let (url, recorder) = spawn_mock(state).await;

        let t = make_transport(&url);
        t.request("initialize", None).await.unwrap();
        t.request("tools/list", None).await.unwrap();

        let snap = recorder.snapshot();
        assert_eq!(snap.len(), 2);
        // First request: no Mcp-Session-Id yet.
        assert!(!snap[0].headers.iter().any(|(k, _)| k == "mcp-session-id"));
        // Second request: echoed back exactly.
        let echoed = snap[1]
            .headers
            .iter()
            .find(|(k, _)| k == "mcp-session-id")
            .expect("Mcp-Session-Id must be echoed on the 2nd request");
        assert_eq!(echoed.1, "abc-session-123");
    }

    #[tokio::test]
    async fn mcp_protocol_version_header_added_after_first_response() {
        let state = MockServerState::default();
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 1, "result": {}
        })));
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 2, "result": {}
        })));
        let (url, recorder) = spawn_mock(state).await;

        let t = make_transport(&url);
        t.request("initialize", None).await.unwrap();
        t.request("tools/list", None).await.unwrap();

        let snap = recorder.snapshot();
        // First request must NOT include MCP-Protocol-Version (per spec
        // §2 — handshake establishes the version).
        assert!(!snap[0]
            .headers
            .iter()
            .any(|(k, _)| k == "mcp-protocol-version"));
        // Second request MUST include it.
        let v = snap[1]
            .headers
            .iter()
            .find(|(k, _)| k == "mcp-protocol-version")
            .expect("MCP-Protocol-Version must be sent post-handshake");
        assert_eq!(v.1, PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn accept_header_advertises_both_response_shapes() {
        let state = MockServerState::default();
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 1, "result": {}
        })));
        let (url, recorder) = spawn_mock(state).await;

        let t = make_transport(&url);
        t.request("anything", None).await.unwrap();

        let snap = recorder.snapshot();
        let accept = snap[0]
            .headers
            .iter()
            .find(|(k, _)| k == "accept")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert!(
            accept.contains("application/json") && accept.contains("text/event-stream"),
            "Accept must advertise both shapes per spec §2; got: {accept}"
        );
    }

    #[tokio::test]
    async fn bearer_token_is_sent_as_authorization_header() {
        let state = MockServerState::default();
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 1, "result": {}
        })));
        let (url, recorder) = spawn_mock(state).await;

        let secret_store = SecretStore::new(std::path::Path::new(""), false);
        let secret = Secret::new_from_plaintext("test-static-bearer", &secret_store).unwrap();
        let t = HttpTransport::connect(
            "test-http".into(),
            &url,
            Some(&secret),
            &HashMap::new(),
            Duration::from_secs(2),
            &secret_store,
            None,
            None,
        )
        .unwrap();
        t.request("anything", None).await.unwrap();

        let snap = recorder.snapshot();
        let auth = snap[0]
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert_eq!(auth, "Bearer test-static-bearer");
    }

    #[tokio::test]
    async fn custom_headers_pass_through_verbatim() {
        let state = MockServerState::default();
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 1, "result": {}
        })));
        let (url, recorder) = spawn_mock(state).await;

        let mut headers = HashMap::new();
        headers.insert("X-Api-Key".into(), "magic-key-42".into());
        let t = HttpTransport::connect(
            "test-http".into(),
            &url,
            None,
            &headers,
            Duration::from_secs(2),
            &empty_secret_store(),
            None,
            None,
        )
        .unwrap();
        t.request("anything", None).await.unwrap();

        let snap = recorder.snapshot();
        let custom = snap[0]
            .headers
            .iter()
            .find(|(k, _)| k == "x-api-key")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert_eq!(custom, "magic-key-42");
    }

    #[tokio::test]
    async fn server_jsonrpc_error_envelope_surfaces_as_mcp_protocol_error() {
        let state = MockServerState::default();
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 1,
            "error": {"code": -32601, "message": "method not found"}
        })));
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let err = t.request("bad/method", None).await.unwrap_err();
        let proto = err.downcast_ref::<McpProtocolError>().unwrap();
        assert_eq!(proto.0.code, -32601);
    }

    #[tokio::test]
    async fn invalid_url_scheme_is_rejected_at_construct_time() {
        let result = HttpTransport::connect(
            "bad".into(),
            "file:///etc/passwd",
            None,
            &HashMap::new(),
            Duration::from_secs(2),
            &empty_secret_store(),
            None,
            None,
        );
        // HttpTransport contains a `Mutex` so Result::unwrap_err / Debug
        // are unavailable — pattern-match instead.
        let msg = match result {
            Ok(_) => panic!("file:// scheme must be rejected"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            msg.contains("http://") && msg.contains("https://"),
            "scheme rejection must mention allowed schemes; got: {msg}"
        );
    }

    // ── PR #81: 401 OAuth recovery ─────────────────────────────────────

    /// PR #81 regression: without an AuthService, a 401 surfaces the
    /// Phase-0 "configure a static bearer" wording byte-identically to
    /// PR #76. Users who never enabled OAuth see ZERO change.
    #[tokio::test]
    async fn no_oauth_configured_keeps_phase0_message_on_401() {
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::UNAUTHORIZED,
            content_type: "text/plain",
            body: "no bearer".into(),
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;
        // make_transport passes (None, None) for auth_service +
        // oauth_server_name — the canonical no-OAuth setup.
        let t = make_transport(&url);
        let err = t.request("tools/list", None).await.unwrap_err();
        let proto = err.downcast_ref::<McpProtocolError>().unwrap();
        assert_eq!(proto.0.code, -32001);
        assert!(
            proto
                .0
                .message
                .contains("Phase 0 plaw does not implement OAuth"),
            "expected Phase 0 wording; got: {}",
            proto.0.message
        );
        assert!(
            !proto.0.message.contains("plaw auth login"),
            "Phase 0 message must NOT mention the OAuth login command"
        );
    }

    /// PR #81 happy path: OAuth is configured, the FIRST request gets a
    /// 401, the transport calls AuthService::get_valid_mcp_access_token,
    /// swaps the bearer, retries once — and the second request hits
    /// 200. Asserts: exactly 2 server-side requests, the second carries
    /// the new bearer, the user sees Ok(Value).
    #[tokio::test]
    async fn oauth_recovery_on_401_swaps_bearer_and_retries() {
        use crate::auth::profiles::TokenSet;
        let state = MockServerState::default();
        // First request: 401.
        state.push(ScriptedResponse {
            status: StatusCode::UNAUTHORIZED,
            content_type: "text/plain",
            body: "expired token".into(),
            extra_headers: Vec::new(),
        });
        // Second request: 200 with a valid JSON-RPC envelope.
        state.push(MockServerState::json_ok(json!({
            "jsonrpc": "2.0", "id": 1, "result": {"recovered": true}
        })));
        let (url, recorder) = spawn_mock(state).await;

        // Pre-seed an AuthService with a valid MCP profile so the
        // recovery path finds a fresh access token. `auth-profiles.json`
        // lives under a tempdir so the test is hermetic.
        let tmp = tempfile::TempDir::new().unwrap();
        let svc = std::sync::Arc::new(crate::auth::AuthService::new(tmp.path(), false));
        let token_set = TokenSet {
            access_token: "fresh_access_token".into(),
            refresh_token: Some("rrr".into()),
            id_token: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            token_type: Some("Bearer".into()),
            scope: None,
        };
        svc.store_mcp_oauth(
            "test_recovery",
            token_set,
            Some("cid".into()),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let t = HttpTransport::connect(
            "test_recovery".into(),
            &url,
            None,
            &HashMap::new(),
            Duration::from_secs(2),
            &empty_secret_store(),
            Some(svc),
            Some("test_recovery".into()),
        )
        .unwrap();

        let result = t.request("tools/list", None).await.unwrap();
        assert_eq!(result, json!({"recovered": true}));

        let snap = recorder.snapshot();
        assert_eq!(
            snap.len(),
            2,
            "expected exactly 2 server-side requests (1 initial 401 + 1 recovery retry); got {}",
            snap.len()
        );
        // The retry must carry the NEW bearer.
        let auth = snap[1]
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert_eq!(auth, "Bearer fresh_access_token");
    }

    /// PR #81 second-401 path: even after a successful refresh, the
    /// server might 401 again (scope mismatch, audience mismatch,
    /// revoked token). The transport MUST NOT thrash — at most one
    /// retry per request — and the error MUST point the user at
    /// `plaw auth login` (NOT the Phase 0 "static bearer" wording).
    #[tokio::test]
    async fn oauth_recovery_second_401_falls_through_to_oauth_aware_error() {
        use crate::auth::profiles::TokenSet;
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::UNAUTHORIZED,
            content_type: "text/plain",
            body: "first 401".into(),
            extra_headers: Vec::new(),
        });
        state.push(ScriptedResponse {
            status: StatusCode::UNAUTHORIZED,
            content_type: "text/plain",
            body: "still 401".into(),
            extra_headers: Vec::new(),
        });
        let (url, recorder) = spawn_mock(state).await;

        let tmp = tempfile::TempDir::new().unwrap();
        let svc = std::sync::Arc::new(crate::auth::AuthService::new(tmp.path(), false));
        svc.store_mcp_oauth(
            "test_double401",
            TokenSet {
                access_token: "fresh".into(),
                refresh_token: Some("r".into()),
                id_token: None,
                expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                token_type: Some("Bearer".into()),
                scope: None,
            },
            Some("cid".into()),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let t = HttpTransport::connect(
            "test_double401".into(),
            &url,
            None,
            &HashMap::new(),
            Duration::from_secs(2),
            &empty_secret_store(),
            Some(svc),
            Some("test_double401".into()),
        )
        .unwrap();

        let err = t.request("tools/list", None).await.unwrap_err();
        let snap = recorder.snapshot();
        // At most ONE retry per request — exactly 2 server hits, not 3+.
        assert_eq!(snap.len(), 2, "must not thrash the IdP / server");

        let proto = err.downcast_ref::<McpProtocolError>().unwrap();
        assert_eq!(proto.0.code, -32001);
        assert!(
            proto
                .0
                .message
                .contains("plaw auth login --provider mcp:test_double401"),
            "OAuth-configured error must point user at the login command; got: {}",
            proto.0.message
        );
        // And must NOT mention the Phase 0 static-bearer wording.
        assert!(
            !proto
                .0
                .message
                .contains("Phase 0 plaw does not implement OAuth"),
            "OAuth-configured error must NOT use the Phase 0 wording"
        );
    }
}
