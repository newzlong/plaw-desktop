//! RFC 7636 PKCE S256 generation for MCP OAuth Phase 1.
//!
//! Thin wrapper over the existing `auth::oauth_common::generate_pkce_state`
//! helper — same OsRng, same SHA-256, same `URL_SAFE_NO_PAD` base64
//! encoding (Notion's strict-matcher rejects trailing `=`). Per
//! CLAUDE.md §3.3 (DRY + Rule of Three), there is exactly ONE PKCE
//! generator in the crate; this module exposes the MCP-flavoured types
//! (`PkcePair`) over it without re-implementing the crypto.
//!
//! # Reuse vs duplicate
//!
//! `auth::oauth_common` lives at `pub(crate)`, is already exercised by
//! the OpenAI Codex + Gemini OAuth flows, and ships under load. The
//! synthesis's "Phase 1 should use OsRng explicitly" concern is
//! already satisfied by the upstream helper. Duplicating the
//! `random_base64url(64) + SHA256 + base64url-no-pad` lines into this
//! module would only add a second audit surface and a second place a
//! future RNG-typo could break PKCE.
//!
//! # What this module owns
//!
//! The MCP-flavoured `PkcePair` struct (named per synthesis) plus the
//! `generate_state` helper returning a 24-byte base64url-no-pad CSRF
//! token. State and verifier are both OsRng-derived; both must be
//! held in process memory only and never persisted.

use crate::auth::oauth_common::{generate_pkce_state, random_base64url};

/// PKCE verifier + challenge pair per RFC 7636 §4.
///
/// `verifier`: 86 ASCII chars (64 random bytes, base64url-no-pad — well
/// within the 43-128 range required by §4.1). Lives in process memory
/// only; never serialized, never logged, dropped on completion of the
/// authorization-code exchange.
///
/// `challenge`: SHA-256(verifier) → base64url-no-pad. Always 43 chars.
/// Sent on the authorize request as `code_challenge=<challenge>` with
/// `code_challenge_method=S256`.
#[derive(Debug, Clone)]
pub(crate) struct PkcePair {
    pub(crate) verifier: String,
    pub(crate) challenge: String,
}

/// Generate a fresh PKCE pair for one authorization. S256 only — plain
/// is intentionally rejected even when the AS metadata advertises it
/// (PR #79 spec lock: PKCE S256 mandatory per MCP 2025-06-18 even
/// against AS that allow plain).
pub(crate) fn generate_pkce() -> PkcePair {
    let inner = generate_pkce_state();
    PkcePair {
        verifier: inner.code_verifier,
        challenge: inner.code_challenge,
    }
}

/// Generate a fresh 24-byte (32-char) CSRF state token. Sent on the
/// authorize URL as `state=<token>` and asserted byte-equal in the
/// loopback redirect handler.
///
/// 24 bytes of OsRng entropy = 192 bits, well above the RFC 6749 §10.12
/// "sufficient entropy" bar.
pub(crate) fn generate_state() -> String {
    random_base64url(24)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    /// Notion (and other strict matchers) reject `=` padding in the
    /// PKCE fields. The base64 engine MUST be `URL_SAFE_NO_PAD`.
    /// Regression test for any future swap to `URL_SAFE` (with pad).
    #[test]
    fn pkce_pair_has_no_padding() {
        for _ in 0..16 {
            let pair = generate_pkce();
            assert!(
                !pair.verifier.contains('='),
                "verifier MUST be base64url-no-pad, got: {}",
                pair.verifier
            );
            assert!(
                !pair.challenge.contains('='),
                "challenge MUST be base64url-no-pad, got: {}",
                pair.challenge
            );
        }
    }

    /// Independent recomputation of challenge from verifier proves the
    /// challenge is genuinely `base64url-no-pad(SHA256(verifier))`.
    #[test]
    fn pkce_challenge_matches_sha256_of_verifier() {
        let pair = generate_pkce();
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(pair.verifier.as_bytes()));
        assert_eq!(pair.challenge, expected);
    }

    /// RFC 7636 §4.1: verifier length MUST be 43-128 chars. 64 random
    /// bytes base64url-no-pad = 86 chars — sweet spot.
    #[test]
    fn pkce_verifier_length_within_rfc7636_bounds() {
        let pair = generate_pkce();
        let len = pair.verifier.len();
        assert!(
            (43..=128).contains(&len),
            "verifier len {len} out of RFC 7636 §4.1 range 43-128"
        );
    }

    /// Two consecutive calls MUST produce different verifiers — sanity
    /// check that OsRng is actually consulted per call (regression
    /// against a future "static seed" bug). Not a cryptographic proof
    /// of OsRng-ness, just a smoke test.
    #[test]
    fn pkce_pair_is_distinct_per_call() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
    }

    /// CSRF state token is non-empty, no padding, distinct per call.
    #[test]
    fn state_token_distinct_and_no_padding() {
        let a = generate_state();
        let b = generate_state();
        assert!(!a.is_empty());
        assert!(!a.contains('='));
        assert_ne!(a, b);
    }
}
