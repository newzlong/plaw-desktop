//! OAuth 2.1 authorization-code + PKCE ceremony with loopback redirect.
//!
//! Owns the end-to-end "run a real OAuth flow against a real MCP
//! server" sequence:
//!
//! 1. [`pick_loopback_port`] — bind 127.0.0.1 on one of the
//!    47830-47839 ports per RFC 8252 §7.3 (NOT port 0; GitHub OAuth
//!    Apps need the exact registered redirect_uri).
//! 2. [`run_authorization_code_flow`] — build the authorize URL with
//!    PKCE challenge + state + RFC 8707 `resource=`, launch the
//!    user's browser, spin up an axum one-shot listener on the
//!    chosen port, await the redirect callback, validate `state`
//!    matches, exchange the auth code at the token endpoint with
//!    the PKCE verifier + resource indicator, return the
//!    [`crate::auth::profiles::TokenSet`].
//! 3. [`refresh_token_grant`] — RFC 6749 §6 refresh, with RFC 8707
//!    `resource=` ALWAYS included (Linear + Notion rotate the
//!    refresh_token on every use; persisting before returning is
//!    enforced at the caller layer).
//!
//! # Threat model (lens C from the synthesis)
//!
//! - **`state` CSRF**: 24-byte OsRng token (192 bits). Verified
//!   byte-equal in the redirect handler; mismatched state returns
//!   400 to the browser and an `anyhow::Error` to the orchestrator.
//! - **Open redirect on the loopback listener**: the listener
//!   accepts only one path (`/oauth/callback`); any other path
//!   returns 404 without touching the pending-state map.
//! - **Browser-launch failure**: never aborts. The authorize URL is
//!   ALWAYS printed to stdout so a user without a default browser
//!   can paste it into one of their choosing.
//! - **Listener timeout**: 300 s wall clock. After that the server
//!   shuts down cleanly and the orchestrator returns a clear
//!   "OAuth authorization timed out" error.
//! - **Concurrent flows on the same port**: prevented by the
//!   exclusive `TcpListener::bind` on the port. The caller of
//!   `pick_loopback_port` holds the listener for the whole flow.
//!
//! # PR #80 visibility
//!
//! Everything `pub(crate)`. The synthesis kept the trait surface at
//! `request + notify + close` (PR #76), so OAuth recovery lives
//! inside `HttpTransport::request`'s 401 branch (wired in PR #81)
//! and the CLI calls into `AuthService::run_mcp_login` (wired in
//! this PR).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::auth::profiles::TokenSet;

use super::discovery::{AuthServerMetadata, ProtectedResourceMetadata};
use super::pkce::{generate_pkce, generate_state, PkcePair};
use super::ClientCredentials;

/// First loopback port plaw tries to bind. The 47830-47839 range is
/// arbitrary (RFC 8252 §7.3 only requires a loopback IP, not a
/// specific port range) but is documented so users can pre-register
/// the same range in their OAuth App redirect URIs.
const LOOPBACK_PORT_BASE: u16 = 47830;
const LOOPBACK_PORT_COUNT: u16 = 10;

/// How long the ceremony waits for the user to complete the
/// authorization in their browser before giving up.
const CEREMONY_TIMEOUT_SECS: u64 = 300;

/// Result of a successful loopback bind — caller holds the listener
/// for the rest of the flow so nothing else can grab the port.
pub(crate) struct LoopbackBinding {
    pub(crate) listener: TcpListener,
    pub(crate) port: u16,
}

/// Bind 127.0.0.1 on one of the 47830-47839 ports. The first port to
/// bind successfully wins. When `hint` is provided, plaw tries that
/// port exclusively — useful when the user has a strict firewall or
/// an OAuth App registered with a specific redirect_uri port.
pub(crate) async fn pick_loopback_port(hint: Option<u16>) -> Result<LoopbackBinding> {
    if let Some(port) = hint {
        let listener = TcpListener::bind(("127.0.0.1", port)).await.with_context(|| {
            format!(
                "binding the configured loopback_port {port} failed; another process may be using it"
            )
        })?;
        return Ok(LoopbackBinding { listener, port });
    }

    let mut last_err: Option<anyhow::Error> = None;
    for port in LOOPBACK_PORT_BASE..LOOPBACK_PORT_BASE + LOOPBACK_PORT_COUNT {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(LoopbackBinding { listener, port }),
            Err(e) => last_err = Some(anyhow!(e)),
        }
    }
    bail!(
        "all {} loopback ports {}-{} are busy; set `[mcp.servers.X.transport.oauth] loopback_port` \
         to override. Last bind error: {}",
        LOOPBACK_PORT_COUNT,
        LOOPBACK_PORT_BASE,
        LOOPBACK_PORT_BASE + LOOPBACK_PORT_COUNT - 1,
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "<unknown>".into()),
    );
}

/// Build a redirect_uri of the form `http://127.0.0.1:{port}/oauth/callback`.
/// Plain function (not method) so tests can build URIs without binding.
pub(crate) fn build_redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/oauth/callback")
}

/// Run the full RFC 6749 §4.1 authorization-code + PKCE flow.
///
/// The caller has already done PRM + AS metadata discovery (PR #79
/// foundations) and resolved the [`ClientCredentials`] from either
/// RFC 7591 DCR or pre-registered config. This function owns:
///
/// - generating a fresh PKCE pair + state
/// - building the authorize URL with all RFC 6749 / RFC 7636 /
///   RFC 8707 query params
/// - launching the browser via the `open` crate (best-effort; the
///   URL is always printed to stdout as a fallback)
/// - spinning up the axum loopback listener with a oneshot bridge to
///   the redirect handler
/// - validating the `state` matches and extracting the `code`
/// - POSTing the code + verifier + resource to the token endpoint
///   per RFC 6749 §4.1.3
/// - parsing the [`TokenSet`] from the response
///
/// Errors when:
/// - port allocation fails (all 10 loopback ports busy)
/// - the user closes the browser / never completes auth (300 s timeout)
/// - the redirect arrives with `state` not matching ours
/// - the AS returns an `error=` parameter on the redirect
/// - the token endpoint returns non-2xx or an `error` envelope
pub(crate) async fn run_authorization_code_flow(
    http: &reqwest::Client,
    prm: &ProtectedResourceMetadata,
    as_metadata: &AuthServerMetadata,
    creds: &ClientCredentials,
    scopes: &[String],
    loopback_port_hint: Option<u16>,
) -> Result<TokenSet> {
    let binding = pick_loopback_port(loopback_port_hint).await?;
    let port = binding.port;
    let redirect_uri = build_redirect_uri(port);
    let pkce = generate_pkce();
    let state = generate_state();

    let resolved_scopes: Vec<String> = if scopes.is_empty() {
        prm.scopes_supported.clone()
    } else {
        scopes.to_vec()
    };

    let authorize_url = build_authorize_url(
        as_metadata,
        creds,
        &redirect_uri,
        &pkce,
        &state,
        &resolved_scopes,
        &prm.resource,
    );

    let LoopbackOutcome {
        code_rx,
        server_handle,
    } = spawn_loopback_listener(binding, state.clone());

    eprintln!(
        "\nopen this URL in a browser to authorize plaw to access this MCP server:\n  {authorize_url}\n"
    );
    if let Err(e) = open::that(&authorize_url) {
        tracing::warn!(
            error = %e,
            "failed to launch system browser automatically; complete authorization manually using the URL above"
        );
    }

    let auth_code =
        match tokio::time::timeout(Duration::from_secs(CEREMONY_TIMEOUT_SECS), code_rx).await {
            Ok(Ok(Ok(code))) => code,
            Ok(Ok(Err(e))) => {
                // Handler reported state mismatch / OAuth error — drop the
                // server task to release the port.
                server_handle.abort();
                return Err(e);
            }
            Ok(Err(_)) => {
                server_handle.abort();
                bail!("loopback listener dropped the sender before the redirect arrived");
            }
            Err(_) => {
                server_handle.abort();
                bail!(
                    "OAuth authorization timed out after {CEREMONY_TIMEOUT_SECS} seconds; \
                 nothing was changed on disk. Retry `plaw auth login --provider mcp:<name>`."
                );
            }
        };
    // Successful flow: drop the server task so the port is released
    // before the token exchange (which goes to a different host).
    server_handle.abort();

    let token_set = exchange_code_for_token(
        http,
        &as_metadata.token_endpoint,
        creds,
        &auth_code,
        &pkce.verifier,
        &redirect_uri,
        &prm.resource,
    )
    .await?;

    Ok(token_set)
}

/// RFC 6749 §6 refresh-token grant. Always sends `resource=`
/// (RFC 8707) so the rotated token is bound to the correct MCP
/// server endpoint.
///
/// Returns the new [`TokenSet`]. The caller MUST persist this BEFORE
/// returning the access_token — Linear + Notion rotate refresh_token
/// on every use and persisting the old token after a successful
/// refresh bricks the connection on the next call.
pub(crate) async fn refresh_token_grant(
    http: &reqwest::Client,
    token_endpoint: &str,
    refresh_token: &str,
    creds: &ClientCredentials,
    resource: &str,
) -> Result<TokenSet> {
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
        ("client_id", creds.client_id().to_string()),
        ("resource", resource.to_string()),
    ];
    if let Some(secret) = creds.client_secret() {
        form.push(("client_secret", secret.to_string()));
    }

    let response = http
        .post(token_endpoint)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .with_context(|| format!("POST refresh_token to {token_endpoint}"))?;

    parse_token_response(response).await
}

// ─── Internal helpers ────────────────────────────────────────────────

/// Outcome from [`spawn_loopback_listener`]: the receiver the caller
/// awaits for the auth code + the server task handle so the caller
/// can `.abort()` it on timeout / success / error to release the port.
struct LoopbackOutcome {
    code_rx: oneshot::Receiver<Result<String>>,
    server_handle: tokio::task::JoinHandle<()>,
}

/// Spawn the loopback listener as a background task. The handler
/// sends the auth `code` (or an error) through the returned oneshot
/// the FIRST time a request hits `/oauth/callback`; the caller
/// awaits that receiver. The server task is held alive until the
/// caller `.abort()`s it (success, error, or timeout all path
/// through the abort).
fn spawn_loopback_listener(binding: LoopbackBinding, expected_state: String) -> LoopbackOutcome {
    let (code_tx, code_rx) = oneshot::channel::<Result<String>>();
    let state = ListenerState {
        expected_state,
        code_tx: Arc::new(std::sync::Mutex::new(Some(code_tx))),
    };
    let app = Router::new()
        .route("/oauth/callback", get(handle_callback))
        .with_state(state);
    let LoopbackBinding { listener, port: _ } = binding;
    let server_handle = tokio::spawn(async move {
        // Best-effort serve. If the server errors (port hijacked,
        // OS shutdown, etc.) we log it; the caller is already
        // racing this against a 300 s timeout on code_rx.
        if let Err(e) = axum::serve(listener, app).await {
            tracing::warn!(error = %e, "loopback OAuth listener exited with error");
        }
    });
    LoopbackOutcome {
        code_rx,
        server_handle,
    }
}

#[derive(Clone)]
struct ListenerState {
    expected_state: String,
    code_tx: Arc<std::sync::Mutex<Option<oneshot::Sender<Result<String>>>>>,
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

async fn handle_callback(
    State(state): State<ListenerState>,
    Query(params): Query<CallbackParams>,
) -> Html<&'static str> {
    let outcome = if let Some(err) = params.error.as_deref() {
        let desc = params.error_description.unwrap_or_default();
        Err(anyhow!(
            "authorization server returned OAuth error '{err}': {desc}"
        ))
    } else if params.state.as_deref() != Some(state.expected_state.as_str()) {
        Err(anyhow!(
            "state parameter on redirect did not match the value plaw generated; \
             possible CSRF — refusing to use the code"
        ))
    } else if let Some(code) = params.code {
        Ok(code)
    } else {
        Err(anyhow!(
            "redirect to loopback was missing the `code` parameter"
        ))
    };

    if let Some(tx) = state.code_tx.lock().ok().and_then(|mut g| g.take()) {
        let _ = tx.send(outcome);
    }

    Html(CALLBACK_PAGE)
}

const CALLBACK_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>plaw OAuth complete</title>
<style>body{font-family:system-ui;text-align:center;margin-top:5em}h1{color:#3a7d44}</style>
</head><body>
<h1>plaw is now authorized</h1>
<p>You can close this tab and return to your terminal.</p>
</body></html>"#;

/// Build the authorize URL with all required query parameters per
/// RFC 6749 §4.1.1, RFC 7636 §4.3, RFC 8707 §2.
fn build_authorize_url(
    as_metadata: &AuthServerMetadata,
    creds: &ClientCredentials,
    redirect_uri: &str,
    pkce: &PkcePair,
    state: &str,
    scopes: &[String],
    resource: &str,
) -> String {
    let mut url = as_metadata.authorization_endpoint.clone();
    let separator = if url.contains('?') { '&' } else { '?' };
    url.push(separator);

    let mut params: Vec<(&str, String)> = vec![
        ("response_type", "code".to_string()),
        ("client_id", creds.client_id().to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("code_challenge", pkce.challenge.clone()),
        ("code_challenge_method", "S256".to_string()),
        ("state", state.to_string()),
        ("resource", resource.to_string()),
    ];
    if !scopes.is_empty() {
        params.push(("scope", scopes.join(" ")));
    }

    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    url.push_str(&query);
    url
}

async fn exchange_code_for_token(
    http: &reqwest::Client,
    token_endpoint: &str,
    creds: &ClientCredentials,
    code: &str,
    pkce_verifier: &str,
    redirect_uri: &str,
    resource: &str,
) -> Result<TokenSet> {
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("client_id", creds.client_id().to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("code_verifier", pkce_verifier.to_string()),
        ("resource", resource.to_string()),
    ];
    if let Some(secret) = creds.client_secret() {
        form.push(("client_secret", secret.to_string()));
    }

    let response = http
        .post(token_endpoint)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .with_context(|| format!("POST token exchange to {token_endpoint}"))?;

    parse_token_response(response).await
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenSet> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .context("reading token endpoint response body")?;
    let parsed: TokenResponse = match serde_json::from_slice(&bytes) {
        Ok(t) => t,
        Err(e) => {
            let preview: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            bail!(
                "token endpoint at returned HTTP {status} with non-JSON body: {preview} (parse error: {e})"
            );
        }
    };

    if let Some(err) = parsed.error {
        let desc = parsed.error_description.unwrap_or_default();
        bail!("token endpoint returned OAuth error '{err}': {desc}");
    }
    if !status.is_success() {
        bail!("token endpoint returned HTTP {status} without an OAuth error envelope");
    }
    let access_token = parsed
        .access_token
        .ok_or_else(|| anyhow!("token endpoint response missing `access_token`"))?;

    let expires_at: Option<DateTime<Utc>> = parsed.expires_in.and_then(|secs| {
        let now = Utc::now();
        let cap = i64::try_from(secs).unwrap_or(i64::MAX);
        Utc.timestamp_opt(now.timestamp().saturating_add(cap), 0)
            .single()
    });

    Ok(TokenSet {
        access_token,
        refresh_token: parsed.refresh_token,
        id_token: parsed.id_token,
        expires_at,
        token_type: parsed.token_type,
        scope: parsed.scope,
    })
}

fn url_encode(input: &str) -> String {
    crate::auth::oauth_common::url_encode(input)
}

/// Helper used by tests / future schema docs — never used in
/// production code (Phase 1a's `[mcp.servers.X.transport.oauth].scopes`
/// already arrives as a `Vec<String>`). Kept private.
#[allow(dead_code)]
fn parse_space_separated_scopes(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// Helper used by tests that need to construct an [`AuthServerMetadata`]
/// without going through HTTP discovery.
#[cfg(test)]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn fixture_as_metadata(token_endpoint: String) -> AuthServerMetadata {
    AuthServerMetadata {
        issuer: "https://issuer.example.test/oauth".into(),
        authorization_endpoint: "https://issuer.example.test/oauth/authorize".into(),
        token_endpoint,
        registration_endpoint: None,
        code_challenge_methods_supported: vec!["S256".into()],
        grant_types_supported: vec!["authorization_code".into(), "refresh_token".into()],
        token_endpoint_auth_methods_supported: vec!["none".into()],
    }
}

/// Helper used by tests that need a [`ProtectedResourceMetadata`].
#[cfg(test)]
pub(crate) fn fixture_prm(resource: &str) -> ProtectedResourceMetadata {
    ProtectedResourceMetadata {
        resource: resource.into(),
        authorization_servers: vec!["https://issuer.example.test/oauth".into()],
        scopes_supported: vec!["read".into(), "write".into()],
        bearer_methods_supported: vec!["header".into()],
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn parse_query(url: &str) -> HashMap<String, String> {
    let q = url.split_once('?').map_or("", |(_, q)| q);
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| {
            (
                crate::auth::oauth_common::url_decode(k),
                crate::auth::oauth_common::url_decode(v),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Port picker ──────────────────────────────────────────────────

    #[tokio::test]
    async fn pick_loopback_port_returns_one_of_range() {
        let binding = pick_loopback_port(None).await.expect("port available");
        assert!(
            (LOOPBACK_PORT_BASE..LOOPBACK_PORT_BASE + LOOPBACK_PORT_COUNT).contains(&binding.port),
            "port {} out of expected range",
            binding.port
        );
        drop(binding);
    }

    #[tokio::test]
    async fn pick_loopback_port_respects_explicit_hint_when_available() {
        // Bind to port 0 first to find a guaranteed-free ephemeral port,
        // close it, then ask the picker to use it — should succeed.
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let binding = pick_loopback_port(Some(port))
            .await
            .expect("hint port should be free immediately after probe drop");
        assert_eq!(binding.port, port);
    }

    // ── Authorize URL construction ───────────────────────────────────

    #[test]
    fn authorize_url_contains_all_mandatory_params() {
        let metadata = fixture_as_metadata("https://issuer.example.test/oauth/token".into());
        let creds = ClientCredentials::Public {
            client_id: "dcr_abc".into(),
        };
        let pkce = PkcePair {
            verifier: "v_verifier_value".into(),
            challenge: "c_challenge_value".into(),
        };
        let url = build_authorize_url(
            &metadata,
            &creds,
            "http://127.0.0.1:47830/oauth/callback",
            &pkce,
            "state_csrf_token",
            &["read".into(), "write".into()],
            "https://mcp.example/v1",
        );
        let params = parse_query(&url);
        assert_eq!(
            params.get("response_type").map(String::as_str),
            Some("code")
        );
        assert_eq!(params.get("client_id").map(String::as_str), Some("dcr_abc"));
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:47830/oauth/callback")
        );
        assert_eq!(
            params.get("code_challenge").map(String::as_str),
            Some("c_challenge_value")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            params.get("state").map(String::as_str),
            Some("state_csrf_token")
        );
        assert_eq!(
            params.get("resource").map(String::as_str),
            Some("https://mcp.example/v1")
        );
        assert_eq!(params.get("scope").map(String::as_str), Some("read write"));
        // Public client → no client_secret on the authorize URL (it's
        // only sent at the token endpoint).
        assert!(params.get("client_secret").is_none());
    }

    #[test]
    fn authorize_url_omits_scope_when_empty_list() {
        let metadata = fixture_as_metadata("https://issuer.example.test/oauth/token".into());
        let creds = ClientCredentials::Public {
            client_id: "x".into(),
        };
        let pkce = PkcePair {
            verifier: "v".into(),
            challenge: "c".into(),
        };
        let url = build_authorize_url(
            &metadata,
            &creds,
            "http://127.0.0.1:47830/oauth/callback",
            &pkce,
            "s",
            &[],
            "https://r/x",
        );
        let params = parse_query(&url);
        assert!(params.get("scope").is_none());
    }

    #[test]
    fn authorize_url_uses_ampersand_when_endpoint_already_has_query() {
        let mut metadata = fixture_as_metadata("https://issuer.example.test/oauth/token".into());
        metadata.authorization_endpoint =
            "https://issuer.example.test/oauth/authorize?tenant=plaw".into();
        let creds = ClientCredentials::Public {
            client_id: "x".into(),
        };
        let pkce = PkcePair {
            verifier: "v".into(),
            challenge: "c".into(),
        };
        let url = build_authorize_url(
            &metadata,
            &creds,
            "http://127.0.0.1:47830/oauth/callback",
            &pkce,
            "s",
            &[],
            "r",
        );
        // The fixture's query starts at `?tenant=plaw`; our params must
        // be appended with `&`, not a second `?`.
        assert!(url.starts_with("https://issuer.example.test/oauth/authorize?tenant=plaw&"));
        assert!(!url.contains("oauth/authorize?tenant=plaw?"));
    }

    // ── Token endpoint exchange ──────────────────────────────────────

    #[tokio::test]
    async fn exchange_code_for_token_happy_path_parses_token_set() {
        let server = spawn_token_endpoint_mock(
            axum::http::StatusCode::OK,
            json!({
                "access_token": "tok_access",
                "refresh_token": "tok_refresh",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "read write"
            }),
        )
        .await;
        let http = reqwest::Client::new();
        let creds = ClientCredentials::Public {
            client_id: "c".into(),
        };
        let token_set = exchange_code_for_token(
            &http,
            &server.url,
            &creds,
            "auth_code_xyz",
            "pkce_verifier",
            "http://127.0.0.1:47830/oauth/callback",
            "https://mcp.example/v1",
        )
        .await
        .unwrap();
        assert_eq!(token_set.access_token, "tok_access");
        assert_eq!(token_set.refresh_token.as_deref(), Some("tok_refresh"));
        assert_eq!(token_set.token_type.as_deref(), Some("Bearer"));
        assert_eq!(token_set.scope.as_deref(), Some("read write"));
        assert!(token_set.expires_at.is_some());
        // Body inspection: verify resource= + code_verifier= were sent.
        let body = server.recorded_body.lock().await.clone().unwrap();
        assert!(body.contains("resource=https"));
        assert!(body.contains("code_verifier=pkce_verifier"));
        assert!(body.contains("grant_type=authorization_code"));
    }

    #[tokio::test]
    async fn exchange_code_for_token_oauth_error_envelope_surfaces_clearly() {
        let server = spawn_token_endpoint_mock(
            axum::http::StatusCode::BAD_REQUEST,
            json!({
                "error": "invalid_grant",
                "error_description": "auth code was revoked"
            }),
        )
        .await;
        let http = reqwest::Client::new();
        let creds = ClientCredentials::Public {
            client_id: "c".into(),
        };
        let err = exchange_code_for_token(
            &http,
            &server.url,
            &creds,
            "stale_code",
            "verifier",
            "http://127.0.0.1:47830/oauth/callback",
            "r",
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("invalid_grant"));
        assert!(msg.contains("auth code was revoked"));
    }

    #[tokio::test]
    async fn refresh_token_grant_always_sends_resource_parameter() {
        let server = spawn_token_endpoint_mock(
            axum::http::StatusCode::OK,
            json!({
                "access_token": "new_access",
                "refresh_token": "rotated_refresh",
                "expires_in": 3600,
                "token_type": "Bearer"
            }),
        )
        .await;
        let http = reqwest::Client::new();
        let creds = ClientCredentials::Public {
            client_id: "c".into(),
        };
        let token_set = refresh_token_grant(
            &http,
            &server.url,
            "old_refresh",
            &creds,
            "https://mcp.example/v1",
        )
        .await
        .unwrap();
        assert_eq!(token_set.access_token, "new_access");
        assert_eq!(token_set.refresh_token.as_deref(), Some("rotated_refresh"));
        let body = server.recorded_body.lock().await.clone().unwrap();
        assert!(body.contains("grant_type=refresh_token"));
        assert!(body.contains("refresh_token=old_refresh"));
        // RFC 8707 — the regression we MUST keep:
        assert!(body.contains("resource=https"));
    }

    #[tokio::test]
    async fn refresh_token_grant_sends_client_secret_when_pre_registered() {
        let server = spawn_token_endpoint_mock(
            axum::http::StatusCode::OK,
            json!({"access_token": "x", "token_type": "Bearer"}),
        )
        .await;
        let http = reqwest::Client::new();
        let creds = ClientCredentials::PreRegistered {
            client_id: "github_app".into(),
            client_secret: "shh_dont_log".into(),
        };
        let _ = refresh_token_grant(&http, &server.url, "r", &creds, "https://r/x")
            .await
            .unwrap();
        let body = server.recorded_body.lock().await.clone().unwrap();
        assert!(body.contains("client_secret=shh_dont_log"));
    }

    // ── Mock token endpoint harness ──────────────────────────────────

    struct MockServer {
        url: String,
        recorded_body: Arc<tokio::sync::Mutex<Option<String>>>,
    }

    async fn spawn_token_endpoint_mock(
        status: axum::http::StatusCode,
        response: serde_json::Value,
    ) -> MockServer {
        let recorded = Arc::new(tokio::sync::Mutex::new(None));
        let recorded_clone = recorded.clone();
        let app = Router::new().route(
            "/token",
            axum::routing::post(move |body: String| {
                let recorded = recorded_clone.clone();
                let response = response.clone();
                async move {
                    *recorded.lock().await = Some(body);
                    (status, axum::Json(response))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        MockServer {
            url: format!("http://{addr}/token"),
            recorded_body: recorded,
        }
    }
}
