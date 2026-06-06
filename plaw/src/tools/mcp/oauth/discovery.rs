//! OAuth 2.1 discovery: WWW-Authenticate parser + RFC 9728 PRM +
//! RFC 8414 AS metadata + GitHub fallback.
//!
//! Three concerns in one module because they are tightly coupled by
//! the discovery sequence:
//!
//! 1. The MCP server's 401 carries a WWW-Authenticate header
//!    (RFC 6750 Bearer challenge). [`parse_www_authenticate`] tokenizes
//!    it and extracts `resource_metadata=<url>`.
//! 2. [`fetch_prm`] fetches that URL, parses the JSON per RFC 9728,
//!    and **rejects any URL whose origin differs from the MCP
//!    server's origin**. This blocks a malicious server from
//!    redirecting discovery to attacker.com.
//! 3. [`fetch_as_metadata`] follows the `authorization_servers[0]`
//!    field — fetches `<issuer>/.well-known/oauth-authorization-server`
//!    per RFC 8414. When the fetch returns 404 and the issuer matches
//!    the GitHub OAuth root (`https://github.com/login/oauth`),
//!    [`github_fallback_as_metadata`] returns hardcoded endpoints —
//!    GitHub does not publish RFC 8414 metadata at all.
//!
//! All items `pub(crate)` — only PR #80's ceremony.rs and unit tests
//! consume them.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

/// Parsed RFC 6750 Bearer challenge from a WWW-Authenticate header.
///
/// Only the four fields the OAuth ceremony actually consumes are
/// captured; other auth-params (e.g. `scope=`) are silently dropped.
/// `realm` is captured for human-readable error messages on auth
/// failure; `error` + `error_description` per RFC 6750 §3 surface the
/// AS's reason for the 401.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WwwAuthChallenge {
    pub(crate) realm: Option<String>,
    /// RFC 9728 §5.1: the `resource_metadata` auth-param pointing at
    /// the PRM document for THIS resource. Required by the MCP spec.
    pub(crate) resource_metadata_url: Option<String>,
    /// RFC 6750 §3 — token problem class (`invalid_token`,
    /// `insufficient_scope`, etc.).
    pub(crate) error: Option<String>,
    /// RFC 6750 §3 — human-readable description.
    pub(crate) error_description: Option<String>,
}

/// Parse a `WWW-Authenticate` header value into a [`WwwAuthChallenge`].
///
/// Returns `None` when the scheme is not `Bearer` (we only support
/// Bearer for MCP OAuth Phase 1). Robust against:
///
/// - **Quoted-string values with embedded commas**: `realm="foo,bar"`
///   does NOT split mid-quote (the GitHub MCP error description
///   sometimes contains commas).
/// - **Unquoted values**: RFC 6750 §3 also allows token68 / unquoted
///   values for some params.
/// - **Param order independence**: spec does not mandate order.
/// - **Case-insensitive scheme + param names**: `bearer`, `Bearer`,
///   `BEARER` all accepted; `realm` vs `Realm` accepted.
///
/// Implemented as a hand-rolled tokenizer instead of pulling the
/// `http-auth` crate per CLAUDE.md §10 (no deps for minor convenience)
/// and per [[oss-agent-framework-audit]] no-deps-for-parsing principle.
/// ~80 LOC including doc comments — manageable audit surface.
pub(crate) fn parse_www_authenticate(header: &str) -> Option<WwwAuthChallenge> {
    let trimmed = header.trim_start();
    // The scheme is the first whitespace-delimited token.
    let (scheme, rest) = trimmed.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }

    let mut realm = None;
    let mut resource_metadata_url = None;
    let mut error = None;
    let mut error_description = None;

    let mut chars = rest.chars().peekable();
    while chars.peek().is_some() {
        // Skip leading whitespace + commas between params.
        while let Some(&c) = chars.peek() {
            if c == ',' || c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        // Read the param name up to '='.
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                chars.next();
                break;
            }
            name.push(c);
            chars.next();
        }
        if name.is_empty() {
            break;
        }
        let name = name.trim().to_lowercase();

        // Value is either a quoted-string or a bare token until the
        // next ',' / whitespace boundary.
        let value = if chars.peek() == Some(&'"') {
            chars.next(); // consume opening quote
            let mut buf = String::new();
            let mut escaped = false;
            for c in chars.by_ref() {
                if escaped {
                    buf.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    break;
                } else {
                    buf.push(c);
                }
            }
            buf
        } else {
            let mut buf = String::new();
            while let Some(&c) = chars.peek() {
                if c == ',' || c.is_whitespace() {
                    break;
                }
                buf.push(c);
                chars.next();
            }
            buf
        };

        match name.as_str() {
            "realm" => realm = Some(value),
            "resource_metadata" => resource_metadata_url = Some(value),
            "error" => error = Some(value),
            "error_description" => error_description = Some(value),
            _ => {}
        }
    }

    Some(WwwAuthChallenge {
        realm,
        resource_metadata_url,
        error,
        error_description,
    })
}

/// RFC 9728 Protected Resource Metadata document.
///
/// Only the fields plaw's OAuth ceremony actually reads are captured.
/// `authorization_servers` is mandatory per the spec; we pick element
/// `[0]` (single-AS-per-PRM per PR #79 anti-scope, multi-AS deferred).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProtectedResourceMetadata {
    /// The resource identifier — sent verbatim (no canonicalisation)
    /// as the RFC 8707 `resource=` parameter on every authorize and
    /// token request.
    pub(crate) resource: String,
    pub(crate) authorization_servers: Vec<String>,
    #[serde(default)]
    pub(crate) scopes_supported: Vec<String>,
    #[serde(default)]
    pub(crate) bearer_methods_supported: Vec<String>,
}

/// RFC 8414 Authorization Server Metadata document.
///
/// `registration_endpoint` is optional per RFC 8414 §2; when absent,
/// the ceremony falls back to `PreRegistered` client credentials from
/// `[mcp.servers.<name>.transport.oauth]`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AuthServerMetadata {
    pub(crate) issuer: String,
    pub(crate) authorization_endpoint: String,
    pub(crate) token_endpoint: String,
    #[serde(default)]
    pub(crate) registration_endpoint: Option<String>,
    #[serde(default)]
    pub(crate) code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub(crate) grant_types_supported: Vec<String>,
    #[serde(default)]
    pub(crate) token_endpoint_auth_methods_supported: Vec<String>,
}

/// Fetch the PRM document from `prm_url` and validate its origin
/// matches the MCP server's origin.
///
/// # Security
///
/// The origin check is the primary defence against a malicious MCP
/// server returning `WWW-Authenticate: Bearer
/// resource_metadata="https://attacker.com/..."` to drive plaw into
/// running the OAuth ceremony against an attacker-controlled AS.
/// Rejecting any cross-origin PRM URL blocks this trivially — the
/// PRM document MUST be served by the resource server itself per
/// RFC 9728 §3.
pub(crate) async fn fetch_prm(
    http: &reqwest::Client,
    prm_url: &str,
    mcp_origin: &str,
) -> Result<ProtectedResourceMetadata> {
    let prm_parsed = reqwest::Url::parse(prm_url)
        .with_context(|| format!("PRM url '{prm_url}' did not parse"))?;
    let mcp_parsed = reqwest::Url::parse(mcp_origin)
        .with_context(|| format!("MCP origin '{mcp_origin}' did not parse"))?;
    if origin_of(&prm_parsed) != origin_of(&mcp_parsed) {
        bail!(
            "PRM url origin '{}' does not match MCP server origin '{}'; rejecting cross-origin discovery (RFC 9728 security check)",
            origin_of(&prm_parsed),
            origin_of(&mcp_parsed),
        );
    }
    let response = http
        .get(prm_url)
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| format!("fetching PRM at {prm_url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("PRM fetch returned HTTP {status} at {prm_url}");
    }
    let body = response
        .bytes()
        .await
        .with_context(|| format!("reading PRM body from {prm_url}"))?;
    let prm: ProtectedResourceMetadata = serde_json::from_slice(&body)
        .with_context(|| format!("parsing PRM JSON from {prm_url}"))?;
    if prm.authorization_servers.is_empty() {
        bail!("PRM at {prm_url} is missing required `authorization_servers` field");
    }
    Ok(prm)
}

/// Fetch RFC 8414 AS metadata from `<issuer>/.well-known/oauth-authorization-server`.
///
/// On HTTP 404 AND the issuer matches the GitHub OAuth root,
/// [`github_fallback_as_metadata`] returns hardcoded endpoints —
/// GitHub does not publish RFC 8414 metadata and the MCP spec
/// implicitly accepts well-known issuers via the fallback. For any
/// other issuer, a 404 surfaces as a clear error.
pub(crate) async fn fetch_as_metadata(
    http: &reqwest::Client,
    issuer: &str,
) -> Result<AuthServerMetadata> {
    let issuer_trimmed = issuer.trim_end_matches('/');
    let url = format!("{issuer_trimmed}/.well-known/oauth-authorization-server");
    let response = http
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| format!("fetching AS metadata at {url}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        if let Some(fallback) = github_fallback_as_metadata(issuer) {
            return Ok(fallback);
        }
        bail!(
            "AS metadata at {url} returned 404 and issuer '{issuer}' is not a recognized fallback (GitHub)"
        );
    }
    if !status.is_success() {
        bail!("AS metadata fetch returned HTTP {status} at {url}");
    }
    let body = response
        .bytes()
        .await
        .with_context(|| format!("reading AS metadata body from {url}"))?;
    let metadata: AuthServerMetadata = serde_json::from_slice(&body)
        .with_context(|| format!("parsing AS metadata JSON from {url}"))?;
    Ok(metadata)
}

/// Hardcoded GitHub OAuth endpoints for issuers that match the GitHub
/// OAuth root (`https://github.com/login/oauth`).
///
/// GitHub does not publish RFC 8414 metadata. Without this fallback,
/// the entire GitHub MCP server is unreachable from plaw. The endpoints
/// are taken from GitHub's published OAuth Apps developer docs and
/// have been stable since 2014 — sufficiently load-bearing infra that
/// a hardcoded URL is the lesser evil compared to a perpetually-broken
/// MCP integration.
///
/// Returns `None` for any other issuer so the caller surfaces a clear
/// error per [`fetch_as_metadata`].
pub(crate) fn github_fallback_as_metadata(issuer: &str) -> Option<AuthServerMetadata> {
    let normalized = issuer.trim_end_matches('/');
    if normalized != "https://github.com/login/oauth" {
        return None;
    }
    Some(AuthServerMetadata {
        issuer: normalized.to_string(),
        authorization_endpoint: "https://github.com/login/oauth/authorize".to_string(),
        token_endpoint: "https://github.com/login/oauth/access_token".to_string(),
        // GitHub does not support DCR — caller falls back to
        // PreRegistered ClientCredentials from config.
        registration_endpoint: None,
        // GitHub supports plain AND S256; we always pick S256 per PR
        // #79 spec lock.
        code_challenge_methods_supported: vec!["plain".to_string(), "S256".to_string()],
        grant_types_supported: vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ],
        token_endpoint_auth_methods_supported: vec!["client_secret_post".to_string()],
    })
}

/// Return `scheme://host:port` of a parsed URL. Port is included only
/// when non-default — matches the WHATWG URL "origin" definition used
/// in browser CORS. Strips path, query, fragment.
fn origin_of(url: &reqwest::Url) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("");
    match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── WWW-Authenticate parser ─────────────────────────────────────

    /// Verbatim challenge observed from GitHub MCP `api.githubcopilot.com/mcp/`.
    /// resource_metadata must extract correctly even though the
    /// error_description contains the spec word "request".
    #[test]
    fn parse_www_authenticate_extracts_github_resource_metadata() {
        let header = r#"Bearer error="invalid_request", error_description="No access token was provided in this request", resource_metadata="https://api.githubcopilot.com/.well-known/oauth-protected-resource/mcp/""#;
        let challenge = parse_www_authenticate(header).expect("Bearer parses");
        assert_eq!(challenge.error.as_deref(), Some("invalid_request"));
        assert_eq!(
            challenge.error_description.as_deref(),
            Some("No access token was provided in this request")
        );
        assert_eq!(
            challenge.resource_metadata_url.as_deref(),
            Some("https://api.githubcopilot.com/.well-known/oauth-protected-resource/mcp/")
        );
    }

    /// Quoted-string values with embedded commas MUST NOT split the
    /// param list. Without quote tracking, `realm="foo,bar"` would
    /// split at the comma and produce `realm="foo` + an orphan
    /// `bar"` param.
    #[test]
    fn parse_www_authenticate_handles_embedded_comma_in_quoted_value() {
        let header =
            r#"Bearer realm="foo,bar", resource_metadata="https://example.com/.well-known/prm""#;
        let challenge = parse_www_authenticate(header).expect("Bearer parses");
        assert_eq!(challenge.realm.as_deref(), Some("foo,bar"));
        assert_eq!(
            challenge.resource_metadata_url.as_deref(),
            Some("https://example.com/.well-known/prm")
        );
    }

    /// Param name is case-insensitive per RFC 7230 §3.2.6. Real-world
    /// AS implementations send `Realm` vs `realm` inconsistently.
    #[test]
    fn parse_www_authenticate_param_name_case_insensitive() {
        let header = r#"Bearer Realm="example", RESOURCE_METADATA="https://a/b""#;
        let challenge = parse_www_authenticate(header).expect("Bearer parses");
        assert_eq!(challenge.realm.as_deref(), Some("example"));
        assert_eq!(
            challenge.resource_metadata_url.as_deref(),
            Some("https://a/b")
        );
    }

    /// Scheme other than Bearer returns None — we explicitly do not
    /// support Digest / Basic / mTLS for MCP OAuth Phase 1.
    #[test]
    fn parse_www_authenticate_non_bearer_returns_none() {
        assert!(parse_www_authenticate(r#"Basic realm="x""#).is_none());
        assert!(parse_www_authenticate(r#"Digest realm="x""#).is_none());
    }

    /// Order-independence: `error` after `resource_metadata` parses
    /// the same as before.
    #[test]
    fn parse_www_authenticate_param_order_independent() {
        let a = parse_www_authenticate(
            r#"Bearer resource_metadata="https://x/y", error="invalid_token""#,
        )
        .unwrap();
        let b = parse_www_authenticate(
            r#"Bearer error="invalid_token", resource_metadata="https://x/y""#,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    /// Unquoted token values — RFC 6750 §3 allows them; some AS send
    /// `error=invalid_token` without quotes.
    #[test]
    fn parse_www_authenticate_unquoted_value() {
        let challenge = parse_www_authenticate(r#"Bearer error=invalid_token"#).unwrap();
        assert_eq!(challenge.error.as_deref(), Some("invalid_token"));
    }

    // ── GitHub fallback ─────────────────────────────────────────────

    /// GitHub issuer returns the hardcoded endpoints — the bedrock of
    /// the GitHub MCP integration. Without this, no GitHub MCP.
    #[test]
    fn github_fallback_returns_hardcoded_endpoints() {
        let metadata = github_fallback_as_metadata("https://github.com/login/oauth")
            .expect("GitHub issuer recognized");
        assert_eq!(metadata.issuer, "https://github.com/login/oauth");
        assert_eq!(
            metadata.authorization_endpoint,
            "https://github.com/login/oauth/authorize"
        );
        assert_eq!(
            metadata.token_endpoint,
            "https://github.com/login/oauth/access_token"
        );
        assert!(
            metadata.registration_endpoint.is_none(),
            "DCR not supported"
        );
        assert!(metadata
            .code_challenge_methods_supported
            .contains(&"S256".to_string()));
    }

    /// Trailing slash in the issuer URL must not break the match.
    #[test]
    fn github_fallback_trims_trailing_slash() {
        assert!(github_fallback_as_metadata("https://github.com/login/oauth/").is_some());
        assert!(github_fallback_as_metadata("https://github.com/login/oauth").is_some());
    }

    /// Any other issuer returns None — fallbacks are explicit, not
    /// implicit. A typo'd issuer URL must surface as an error.
    #[test]
    fn github_fallback_non_github_returns_none() {
        assert!(github_fallback_as_metadata("https://example.com/oauth").is_none());
        assert!(github_fallback_as_metadata("https://gitlab.com/login/oauth").is_none());
        assert!(
            github_fallback_as_metadata("https://github.com/login/oauth/extra").is_none(),
            "extra path must NOT match — origin alone is insufficient"
        );
    }

    // ── PRM origin check ─────────────────────────────────────────────

    /// Origin-of helper: scheme + host, port only when non-default.
    #[test]
    fn origin_of_drops_default_port_and_path() {
        let u = reqwest::Url::parse("https://api.example.com/some/path?q=1").unwrap();
        assert_eq!(origin_of(&u), "https://api.example.com");
    }

    #[test]
    fn origin_of_keeps_non_default_port() {
        let u = reqwest::Url::parse("https://example.com:8443/x").unwrap();
        assert_eq!(origin_of(&u), "https://example.com:8443");
    }
}
