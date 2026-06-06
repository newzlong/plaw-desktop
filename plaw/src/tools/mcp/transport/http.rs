//! Streamable HTTP transport (MCP spec 2025-06-18, Phase 0 subset).
//!
//! Phase 0 capability matrix:
//!
//! - ✅ Sync request/response via JSON POST.
//! - ✅ Static bearer token via `Authorization: Bearer ...` header.
//! - ✅ Custom headers (verbatim).
//! - ✅ Mandatory `MCP-Protocol-Version` echoed on every POST after the
//!   initial handshake.
//! - ✅ Server-issued `Mcp-Session-Id` captured on initialize, echoed
//!   on subsequent calls.
//! - ❌ `text/event-stream` response bodies — REJECTED with a clear
//!   error pointing to PR #77. Silent hangs would be worse.
//! - ❌ Standalone `GET` notification stream — `subscribe_notifications`
//!   is intentionally absent from the trait in Phase 0.
//! - ❌ OAuth 2.1 / PKCE / `WWW-Authenticate` / RFC 8414 / RFC 9728 /
//!   RFC 7591 / RFC 8707. 401 / 403 surface as synthetic
//!   `JsonRpcError -32001` with a clear `Phase 0 plaw does not implement
//!   OAuth` message so users see actionable feedback rather than a hang.
//! - ❌ `Last-Event-ID` resumability (not needed without SSE).
//! - ❌ DELETE on shutdown (no persistent server-side state to clean).
//!
//! All of the ❌ items have explicit landing spots in Phase 1 (PR #77).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::McpTransport;
use crate::security::{Secret, SecretStore};
use crate::tools::mcp::client::McpProtocolError;
use crate::tools::mcp::protocol::{
    JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, PROTOCOL_VERSION,
};

/// HTTP transport state. `reqwest::Client` is cheap to clone and pools
/// connections internally, so we keep one per server and let `Drop`
/// release pooled sockets without an explicit DELETE.
pub(crate) struct HttpTransport {
    server_name: String,
    url: String,
    http: reqwest::Client,
    /// Static bearer token (already revealed at construction time). The
    /// `Option<String>` here lives only inside this struct so the
    /// plaintext never lands in `Debug` output of the surrounding
    /// `McpClient` (which derives no transport-leaking impls).
    bearer: Option<String>,
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
}

impl HttpTransport {
    /// Build an `HttpTransport` without performing the MCP handshake.
    /// The handshake itself is driven by the surrounding `McpClient` via
    /// [`McpTransport::request`]; this constructor only validates inputs
    /// and pre-builds the `reqwest::Client` with the configured timeout.
    pub(crate) fn connect(
        server_name: String,
        url: &str,
        bearer_token: Option<&Secret>,
        headers: &HashMap<String, String>,
        request_timeout: Duration,
        secret_store: &SecretStore,
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

        let bearer = bearer_token
            .map(|s| s.reveal(secret_store))
            .transpose()
            .with_context(|| {
                format!("MCP HTTP server '{server_name}': bearer_token decrypt failed")
            })?;

        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .context("building reqwest client for MCP HTTP transport")?;

        Ok(Self {
            server_name,
            url: parsed.into(),
            http,
            bearer,
            headers: headers.clone(),
            next_id: std::sync::atomic::AtomicU64::new(1),
            session_id: Mutex::new(None),
            handshake_complete: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Build a `reqwest::RequestBuilder` with the per-call headers
    /// (Accept, optional MCP-Protocol-Version, optional Mcp-Session-Id,
    /// optional Authorization, plus any user-configured static headers).
    fn build_post(&self) -> reqwest::RequestBuilder {
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
        if let Some(ref token) = self.bearer {
            rb = rb.header("Authorization", format!("Bearer {token}"));
        }
        for (k, v) in &self.headers {
            rb = rb.header(k, v);
        }
        rb
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
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let req = JsonRpcRequest::new(id, method, params);
        let body = serde_json::to_vec(&req)?;

        let response = self.build_post().body(body).send().await.with_context(|| {
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
        let body_bytes = response
            .bytes()
            .await
            .with_context(|| format!("MCP HTTP server '{}': read body failed", self.server_name))?;
        let body_str = String::from_utf8_lossy(&body_bytes);

        if !status.is_success() {
            return Err(McpProtocolError::from(Self::http_status_to_error(
                status.as_u16(),
                body_str.as_ref(),
            ))
            .into());
        }

        if content_type.starts_with("text/event-stream") {
            bail!(
                "MCP HTTP server '{}' returned text/event-stream; PR #76 Phase 0 only supports application/json responses. SSE response bodies land in PR #77.",
                self.server_name
            );
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
        Ok(msg.result.unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let n = JsonRpcNotification::new(method, params);
        let body = serde_json::to_vec(&n)?;

        let response = self.build_post().body(body).send().await.with_context(|| {
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

    #[tokio::test]
    async fn http_text_event_stream_response_is_rejected_with_phase1_hint() {
        let state = MockServerState::default();
        state.push(ScriptedResponse {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: "event: message\ndata: {}\n\n".into(),
            extra_headers: Vec::new(),
        });
        let (url, _) = spawn_mock(state).await;

        let t = make_transport(&url);
        let err = t.request("tools/list", None).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("text/event-stream") && msg.contains("PR #77"),
            "Phase 0 SSE rejection must reference PR #77; got: {msg}"
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
}
