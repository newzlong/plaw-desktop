//! Test-only transport helpers.
//!
//! Two harnesses live here:
//!
//! - [`pair_with_duplex`] — drives [`super::stdio::StdioTransport`]
//!   over an in-memory `tokio::io::duplex` pair (used by
//!   `client.rs` integration tests; no subprocess spawn).
//! - [`http_mock`] — buffered-body axum mock server for HTTP
//!   transport integration tests. Lifted from `http.rs::tests` in
//!   PR #85a (was inline-duplicated since PR #76). Unblocks
//!   PR #85b's GET-listener tests which need a `Stream`-bodied
//!   response variant (added in PR #85b alongside the listener);
//!   PR #85a is a pure behaviour-preserving move.

use std::time::Duration;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

use super::stdio::StdioTransport;

/// Build a [`StdioTransport`] backed by two halves of an in-memory
/// duplex stream. Returns the transport + the "server side" of the
/// duplex (the half the test acts as a fake server on).
///
/// `client_to_server` is the stream the transport WRITES requests into;
/// the test READs from it via the returned `ReadHalf<DuplexStream>`.
/// `server_to_client` is the stream the test WRITES fake server
/// responses into; the transport READs from it.
///
/// `_request_timeout` is accepted for API symmetry with the production
/// `connect` constructor but is unused (timeouts are enforced by the
/// surrounding `McpClient`, not the transport).
pub(crate) fn pair_with_duplex(
    server_name: &str,
    _request_timeout: Duration,
    buf_size: usize,
) -> (
    StdioTransport,
    ReadHalf<DuplexStream>,
    WriteHalf<DuplexStream>,
) {
    let (transport_stdin_w, server_stdin_r_full) = tokio::io::duplex(buf_size);
    let (server_stdout_w_full, transport_stdout_r) = tokio::io::duplex(buf_size);

    // Split each duplex into the half each side uses.
    let (server_read, _) = tokio::io::split(server_stdin_r_full);
    let (_, server_write) = tokio::io::split(server_stdout_w_full);

    let transport = StdioTransport::from_pipes(
        server_name.to_string(),
        Box::new(transport_stdin_w),
        transport_stdout_r,
    );
    (transport, server_read, server_write)
}

/// Shared buffered-body HTTP mock server harness.
///
/// Lifted into a sibling submodule in PR #85a so PR #85b's
/// GET-listener tests can reuse the same `MockServerState` /
/// `ScriptedResponse` shape without inline duplication. The move is
/// behaviour-preserving — `http.rs::tests` continues to operate on
/// byte-identical mock fixtures, only the import path changes.
///
/// All types and helpers are `pub(crate)` to support cross-module
/// use from sibling `transport/*` tests while still being
/// `#[cfg(test)]`-gated.
pub(crate) mod http_mock {
    use axum::{
        extract::State, http::HeaderMap, http::StatusCode, response::IntoResponse, routing::post,
        Router,
    };
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::net::TcpListener;

    /// Per-server request inspector. Tests can read what the HTTP
    /// transport ACTUALLY put on the wire — headers + body — to assert
    /// spec compliance and rule out secret leakage.
    #[derive(Default, Clone)]
    pub(crate) struct RequestRecorder(pub(crate) Arc<StdMutex<Vec<RecordedRequest>>>);

    #[derive(Clone, Debug)]
    pub(crate) struct RecordedRequest {
        pub headers: Vec<(String, String)>,
        pub body: String,
    }

    impl RequestRecorder {
        pub(crate) fn snapshot(&self) -> Vec<RecordedRequest> {
            self.0.lock().unwrap().clone()
        }
    }

    /// Captures the request, then returns whatever the test queued via
    /// [`MockServerState::push`]. Each request consumes one scripted
    /// response in FIFO order; if the queue runs out the server
    /// returns a 500.
    #[derive(Clone, Default)]
    pub(crate) struct MockServerState {
        pub(crate) recorder: RequestRecorder,
        pub(crate) responses: Arc<StdMutex<Vec<ScriptedResponse>>>,
    }

    #[derive(Clone)]
    pub(crate) struct ScriptedResponse {
        pub status: StatusCode,
        pub content_type: &'static str,
        pub body: String,
        pub extra_headers: Vec<(&'static str, String)>,
    }

    impl MockServerState {
        pub(crate) fn push(&self, r: ScriptedResponse) {
            self.responses.lock().unwrap().push(r);
        }

        pub(crate) fn json_ok(body: serde_json::Value) -> ScriptedResponse {
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

    /// Bind a random loopback port + return `(url, recorder)`. The
    /// axum task runs in the background until the runtime stops.
    pub(crate) async fn spawn_mock(state: MockServerState) -> (String, RequestRecorder) {
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
}
