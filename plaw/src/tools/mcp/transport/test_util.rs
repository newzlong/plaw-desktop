//! Test-only transport helpers — used by `client.rs` integration tests
//! to drive [`super::stdio::StdioTransport`] over an in-memory
//! `tokio::io::duplex` pair without spawning a real subprocess.

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
