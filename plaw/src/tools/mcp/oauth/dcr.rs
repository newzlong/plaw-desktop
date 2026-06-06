//! RFC 7591 Dynamic Client Registration for MCP OAuth.
//!
//! POSTs a registration request to the AS's `registration_endpoint`
//! (when present in AS metadata) and parses the resulting `client_id`.
//! Most modern MCP servers (Linear, Notion) support DCR; GitHub does
//! not — when AS metadata returns `registration_endpoint: None`, the
//! caller falls back to `ClientCredentials::PreRegistered` from
//! config.
//!
//! # Public client + native application + loopback redirect
//!
//! The registration request advertises `token_endpoint_auth_method:
//! "none"` (public client per RFC 7591 §2 — PKCE proves possession,
//! no client_secret needed) and `application_type: "native"` (matches
//! the loopback redirect URI per RFC 8252 §7.3). `grant_types`
//! enumerates exactly the two grants plaw uses:
//! `authorization_code` for the initial flow and `refresh_token` for
//! rotation.
//!
//! AS implementations that disagree with `application_type` (some
//! reject anything but "web") return a 400; the caller surfaces a
//! clear error pointing the user at the manual pre-registered path.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Result of [`dynamic_register`] — the subset of the registration
/// response plaw actually uses.
///
/// Most public-client DCR flows return `client_id` only, omitting
/// `client_secret` because PKCE handles proof-of-possession. Some
/// AS implementations return a `client_secret` even for public
/// clients; in that case we still treat it as a public client (PKCE
/// stays mandatory) but the secret is persisted via
/// `AuthProfile::oauth_client_secret`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DcrResult {
    pub(crate) client_id: String,
    pub(crate) client_secret: Option<String>,
}

#[derive(Debug, Serialize)]
struct RegistrationRequest<'a> {
    client_name: &'a str,
    redirect_uris: Vec<String>,
    grant_types: Vec<&'static str>,
    token_endpoint_auth_method: &'static str,
    application_type: &'static str,
}

#[derive(Debug, Deserialize)]
struct RegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

/// POST a public-client registration to `registration_endpoint`.
///
/// Errors when:
/// - the AS returns non-2xx (typically 400 if our request shape is
///   rejected — see module docs)
/// - the response body is missing the required `client_id` field
pub(crate) async fn dynamic_register(
    http: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
    client_name: &str,
) -> Result<DcrResult> {
    let body = RegistrationRequest {
        client_name,
        redirect_uris: vec![redirect_uri.to_string()],
        grant_types: vec!["authorization_code", "refresh_token"],
        token_endpoint_auth_method: "none",
        application_type: "native",
    };

    let response = http
        .post(registration_endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("DCR POST to {registration_endpoint}"))?;

    let status = response.status();
    if !status.is_success() {
        let body_excerpt = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable>".into());
        let excerpt: String = body_excerpt.chars().take(200).collect();
        bail!("DCR returned HTTP {status} at {registration_endpoint}: {excerpt}");
    }

    let parsed: RegistrationResponse = response
        .json()
        .await
        .with_context(|| format!("parsing DCR response from {registration_endpoint}"))?;

    Ok(DcrResult {
        client_id: parsed.client_id,
        client_secret: parsed.client_secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use serde_json::json;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    /// Mock AS that records the POST body and returns a canned
    /// registration response.
    async fn spawn_dcr_mock(
        response: serde_json::Value,
        status: axum::http::StatusCode,
    ) -> (
        String,
        std::sync::Arc<tokio::sync::Mutex<Option<serde_json::Value>>>,
    ) {
        let recorder = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let recorder_clone = recorder.clone();

        let app = Router::new().route(
            "/register",
            post(move |Json(body): Json<serde_json::Value>| {
                let recorder = recorder_clone.clone();
                let response = response.clone();
                async move {
                    *recorder.lock().await = Some(body);
                    (status, Json(response))
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/register"), recorder)
    }

    /// Happy path — the canonical Linear / Notion DCR response shape.
    #[tokio::test]
    async fn dynamic_register_happy_path_parses_client_id() {
        let (url, _recorder) = spawn_dcr_mock(
            json!({"client_id": "dcr_abc123"}),
            axum::http::StatusCode::CREATED,
        )
        .await;
        let http = reqwest::Client::new();
        let result = dynamic_register(&http, &url, "http://127.0.0.1:47830/oauth/callback", "plaw")
            .await
            .unwrap();
        assert_eq!(result.client_id, "dcr_abc123");
        assert!(result.client_secret.is_none());
    }

    /// Some AS implementations return a client_secret even for public
    /// clients. We capture it for persistence but PKCE remains
    /// mandatory regardless.
    #[tokio::test]
    async fn dynamic_register_captures_optional_client_secret() {
        let (url, _) = spawn_dcr_mock(
            json!({"client_id": "dcr_with_secret", "client_secret": "shh"}),
            axum::http::StatusCode::CREATED,
        )
        .await;
        let http = reqwest::Client::new();
        let result = dynamic_register(&http, &url, "http://127.0.0.1:47830/oauth/callback", "plaw")
            .await
            .unwrap();
        assert_eq!(result.client_id, "dcr_with_secret");
        assert_eq!(result.client_secret.as_deref(), Some("shh"));
    }

    /// Request body shape — recorded by the mock — MUST match the
    /// RFC 7591 public-client + native + loopback shape.
    #[tokio::test]
    async fn dynamic_register_posts_canonical_body() {
        let (url, recorder) =
            spawn_dcr_mock(json!({"client_id": "x"}), axum::http::StatusCode::CREATED).await;
        let http = reqwest::Client::new();
        let _ = dynamic_register(&http, &url, "http://127.0.0.1:47830/oauth/callback", "plaw")
            .await
            .unwrap();
        let body = recorder.lock().await.clone().expect("body recorded");
        assert_eq!(body["client_name"], "plaw");
        assert_eq!(body["token_endpoint_auth_method"], "none");
        assert_eq!(body["application_type"], "native");
        let redirects = body["redirect_uris"].as_array().expect("redirect_uris");
        assert_eq!(redirects.len(), 1);
        assert_eq!(
            redirects[0].as_str(),
            Some("http://127.0.0.1:47830/oauth/callback")
        );
        let grants = body["grant_types"].as_array().expect("grant_types");
        let grant_strs: Vec<&str> = grants.iter().filter_map(|v| v.as_str()).collect();
        assert!(grant_strs.contains(&"authorization_code"));
        assert!(grant_strs.contains(&"refresh_token"));
    }

    /// AS rejection surfaces a clear error mentioning the status.
    #[tokio::test]
    async fn dynamic_register_4xx_surfaces_clear_error() {
        let (url, _) = spawn_dcr_mock(
            json!({"error": "invalid_request"}),
            axum::http::StatusCode::BAD_REQUEST,
        )
        .await;
        let http = reqwest::Client::new();
        let err = dynamic_register(&http, &url, "http://127.0.0.1:47830/oauth/callback", "plaw")
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("400") || msg.contains("Bad Request"));
    }
}
