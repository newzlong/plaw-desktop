//! MCP OAuth 2.1 + PKCE — Phase 1 foundations (PR #79).
//!
//! Replaces the Phase 0 fail-fast 401 path from PR #76. Splits across
//! three PRs to keep each layer independently reviewable and revertable
//! per CLAUDE.md §3.8 (security-boundary code amplifies the value of
//! small reviewable units):
//!
//! - **PR #79 (this PR)** — DORMANT foundations: WWW-Authenticate parser,
//!   RFC 9728 PRM discovery, RFC 8414 AS metadata discovery (with GitHub
//!   fallback to hardcoded endpoints), RFC 7591 Dynamic Client
//!   Registration, RFC 7636 PKCE generation, config schema, schema-v2
//!   migration on `auth-profiles.json`. NOT wired into HttpTransport —
//!   the Phase 0 `JsonRpcError -32001` path still surfaces on 401.
//!   Foundation modules are pub(crate) and reachable only from
//!   unit tests until PR #80.
//! - **PR #80 (next)** — OAuth ceremony: loopback listener on
//!   127.0.0.1:47830-47839, authorization-code flow with state/PKCE
//!   bookkeeping, token exchange, `AuthService::run_mcp_login` driven by
//!   `plaw auth login --provider mcp:<name>` CLI.
//! - **PR #81 (next)** — Transport integration: `HttpTransport::request`
//!   detects 401, calls `AuthService::get_valid_mcp_access_token`,
//!   single-flight refresh mutex, swap bearer, retry-once. Registry
//!   surfaces `NeedsAuth` status. End-to-end on real GitHub MCP /
//!   Linear MCP / Notion MCP.
//!
//! # Why this split (not one mega-PR)
//!
//! The synthesis self-rated as HIGH risk + ~1150 LOC. OAuth flows are
//! CVE magnets — PKCE state mismatches, refresh-token rotation, origin
//! checks are all areas where a small bug = full account takeover for
//! every MCP server the user has connected. Layering reduces blast
//! radius:
//!
//! 1. PR #79 is observably DORMANT — no runtime behavior change. The
//!    schema-v2 bump on the shared `auth-profiles.json` is the one
//!    real risk; covered by `schema_v1_file_loads_into_v2_with_oauth_fields_defaulted`
//!    + `future_schema_version_rejected_with_clear_error` tests.
//! 2. PR #80 adds the ceremony as an opt-in CLI command. Users not
//!    running `plaw auth login --provider mcp:*` see zero change.
//! 3. PR #81 wires the recovery path. The narrowest, riskiest commit;
//!    benefits from two PRs of working+tested foundation underneath.
//!
//! All items pub(crate) — no public API surface escapes the crate.
//! Matches `tools/mcp/transport/mod.rs:13-15` Rule-of-Three guidance.

pub(crate) mod ceremony;
pub(crate) mod dcr;
pub(crate) mod discovery;
pub(crate) mod pkce;

// Re-export the small handful of types each submodule needs to expose
// to PR #80's ceremony.rs (or to tests). Keep this list minimal so the
// public-shape audit is one screen.
pub(crate) use dcr::{dynamic_register, DcrResult};
pub(crate) use discovery::{
    fetch_as_metadata, fetch_prm, github_fallback_as_metadata, parse_www_authenticate,
    AuthServerMetadata, ProtectedResourceMetadata, WwwAuthChallenge,
};
pub(crate) use pkce::{generate_pkce, generate_state, PkcePair};

/// OAuth client credentials for an MCP server connection.
///
/// `Public` is the RFC 7591 DCR result for servers that support dynamic
/// client registration (Linear, Notion). The auth server returned a
/// `client_id` but no `client_secret` — the PKCE verifier is the proof
/// of possession.
///
/// `PreRegistered` is the GitHub-style escape hatch: the user pasted
/// `client_id` + `client_secret` into config from
/// `github.com/settings/applications/new`. GitHub's MCP server's OAuth
/// flow requires both; DCR is not supported.
///
/// Either variant works with PKCE; the variant only determines whether
/// the token endpoint POST carries `client_secret=` body parameter.
#[derive(Debug, Clone)]
pub(crate) enum ClientCredentials {
    /// RFC 7591 / DCR result. PKCE verifier proves possession; no
    /// client_secret needed.
    Public { client_id: String },
    /// User-pasted OAuth App credentials. Required by GitHub's MCP
    /// server which does not advertise a `registration_endpoint`.
    PreRegistered {
        client_id: String,
        client_secret: String,
    },
}

impl ClientCredentials {
    /// Owned `client_id` regardless of variant. Used to build the
    /// authorize URL `client_id` query parameter.
    pub(crate) fn client_id(&self) -> &str {
        match self {
            Self::Public { client_id } | Self::PreRegistered { client_id, .. } => client_id,
        }
    }

    /// `Some(secret)` iff the variant carries one. Used to decide
    /// whether the token endpoint POST body includes
    /// `client_secret=<value>`.
    pub(crate) fn client_secret(&self) -> Option<&str> {
        match self {
            Self::Public { .. } => None,
            Self::PreRegistered { client_secret, .. } => Some(client_secret),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_credentials_public_returns_client_id_only() {
        let creds = ClientCredentials::Public {
            client_id: "plaw_dcr_42".into(),
        };
        assert_eq!(creds.client_id(), "plaw_dcr_42");
        assert!(creds.client_secret().is_none());
    }

    #[test]
    fn client_credentials_pre_registered_returns_both() {
        let creds = ClientCredentials::PreRegistered {
            client_id: "Iv1.github_app".into(),
            client_secret: "ghs_must_not_log".into(),
        };
        assert_eq!(creds.client_id(), "Iv1.github_app");
        assert_eq!(creds.client_secret(), Some("ghs_must_not_log"));
    }
}
