//! `Secret` newtype — type-level enforcement that secrets live encrypted
//! on disk.
//!
//! The four-lens audit's "encrypt `*_token`/`*_secret` newtype" finding
//! noted that even with [`SecretStore`] available, config fields were
//! plain `String` / `Option<String>` — so it was easy (silently) to
//! leave a secret in plaintext on disk. This newtype closes the gap:
//!
//! - `serde::Serialize` writes the inner ciphertext blob verbatim
//!   (callers MUST encrypt before constructing).
//! - `serde::Deserialize` reads the inner blob verbatim — could be
//!   plaintext (legacy config), `enc:` (legacy XOR), or `enc2:`
//!   (ChaCha20-Poly1305). The reveal step disambiguates.
//! - [`Secret::reveal`] is the ONLY way to get the plaintext, and it
//!   requires a [`SecretStore`] reference at the call site — forcing
//!   the reader to think about secret material and creating a single
//!   audit hook for every secret access.
//! - `Debug` redacts. `Display` is intentionally not implemented.
//!
//! # Migration semantics
//!
//! Existing config files have plain-text values. [`Secret::reveal`]
//! delegates to [`SecretStore::decrypt`] which already returns
//! plaintext-as-is when neither `enc:` nor `enc2:` prefix is present
//! (security/secrets.rs:96). So existing configs keep working — first
//! write-back round-trips the value through [`Secret::new_from_plaintext`]
//! and it becomes `enc2:` on disk.
//!
//! # Why lazy reveal
//!
//! Per user design choice 2026-05-23: ciphertext-on-disk is the strong
//! invariant; plaintext only ever lives transiently inside the
//! `reveal(&store) -> Result<String>` return value, owned by the
//! immediate caller. No global mutable state, no plaintext-at-rest in
//! `AppState`. Cost: every reader carries an `&SecretStore` reference
//! (typically through `AppState`).

use crate::security::SecretStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Opaque wrapper around an encrypted-at-rest secret value.
///
/// Internal `String` is the wire/disk form: plaintext for legacy
/// configs, `enc:` for legacy XOR, `enc2:` for ChaCha20-Poly1305.
/// Call [`reveal`](Self::reveal) with a [`SecretStore`] to decrypt.
///
/// JsonSchema treats this as a plain string so config docs / Tauri
/// IPC contracts don't expose the wrapper. Wire form is the user-facing
/// value (encrypted or plaintext, depending on `[secrets] encrypt`).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct Secret(String);

impl Secret {
    /// Encrypt `plaintext` and wrap the resulting ciphertext.
    ///
    /// Use this when constructing a `Secret` from user input (wizard
    /// form, env var, etc.). After this call the value on the wire and
    /// on disk is the ciphertext blob; the plaintext is gone.
    ///
    /// # Errors
    ///
    /// Returns any error from [`SecretStore::encrypt`] (key-file IO,
    /// AEAD failure). Encryption disabled (`store.enabled == false`)
    /// returns the plaintext unchanged — by design, see
    /// `[secrets] encrypt = false` config knob.
    pub fn new_from_plaintext(plaintext: &str, store: &SecretStore) -> anyhow::Result<Self> {
        Ok(Self(store.encrypt(plaintext)?))
    }

    /// Wrap an already-encoded value (plaintext, `enc:`, or `enc2:`)
    /// without going through encryption. Used by deserialization paths
    /// and tests that need to construct a `Secret` from a known wire
    /// representation.
    pub fn from_wire(value: String) -> Self {
        Self(value)
    }

    /// Decrypt and return the plaintext.
    ///
    /// The returned `String` is the only window into the secret value;
    /// it should be dropped as soon as the operation that needs it
    /// completes. Do not store the plaintext in long-lived state.
    ///
    /// # Errors
    ///
    /// Returns any error from [`SecretStore::decrypt`] (key-file IO,
    /// AEAD authentication failure, malformed hex). Plaintext values
    /// (no `enc:` / `enc2:` prefix) are returned as-is.
    pub fn reveal(&self, store: &SecretStore) -> anyhow::Result<String> {
        store.decrypt(&self.0)
    }

    /// Borrow the raw wire form (ciphertext or plaintext). Used by
    /// serialization helpers and the mask-on-output path in
    /// `gateway::api`. Avoid for general reads — `reveal` is the
    /// audited path.
    pub fn as_wire_str(&self) -> &str {
        &self.0
    }

    /// Returns true when the inner value is empty (e.g. an unset
    /// `Option::Some(Secret(""))`). Cheaper than `reveal` for
    /// presence-only checks.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never leak the wire form (could be plaintext for legacy
        // configs). Show length so logs still convey "is something
        // there?" without revealing what.
        write!(f, "Secret([REDACTED, {} bytes])", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_store(enabled: bool) -> (TempDir, SecretStore) {
        let dir = TempDir::new().unwrap();
        let store = SecretStore::new(dir.path(), enabled);
        (dir, store)
    }

    #[test]
    fn new_from_plaintext_round_trips() {
        let (_dir, store) = tmp_store(true);
        let s = Secret::new_from_plaintext("my-api-key-12345", &store).unwrap();
        // Wire form must be ciphertext (encrypted).
        assert!(s.as_wire_str().starts_with("enc2:"), "expected enc2: prefix");
        // Reveal returns plaintext.
        assert_eq!(s.reveal(&store).unwrap(), "my-api-key-12345");
    }

    #[test]
    fn debug_redacts_inner() {
        let (_dir, store) = tmp_store(true);
        let s = Secret::new_from_plaintext("super-secret", &store).unwrap();
        let dbg = format!("{:?}", s);
        assert!(dbg.contains("REDACTED"));
        assert!(!dbg.contains("super-secret"));
    }

    #[test]
    fn from_wire_accepts_legacy_plaintext_and_reveal_passes_through() {
        // Old configs have plain-text secrets — Secret::reveal must
        // tolerate that path via SecretStore's no-prefix passthrough.
        let (_dir, store) = tmp_store(true);
        let s = Secret::from_wire("plain-legacy-token".into());
        assert_eq!(s.reveal(&store).unwrap(), "plain-legacy-token");
    }

    #[test]
    fn serde_roundtrip_preserves_wire_format() {
        let (_dir, store) = tmp_store(true);
        let original = Secret::new_from_plaintext("payload", &store).unwrap();
        let json = serde_json::to_string(&original).unwrap();
        // transparent serde: serializes as bare string, not {"0":"..."}
        assert!(json.starts_with('"') && json.ends_with('"'));
        let restored: Secret = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.as_wire_str(), original.as_wire_str());
        assert_eq!(restored.reveal(&store).unwrap(), "payload");
    }

    #[test]
    fn encryption_disabled_store_yields_passthrough_secret() {
        let (_dir, store) = tmp_store(false);
        let s = Secret::new_from_plaintext("plain-value", &store).unwrap();
        // With encryption disabled, wire form == plaintext.
        assert_eq!(s.as_wire_str(), "plain-value");
        assert_eq!(s.reveal(&store).unwrap(), "plain-value");
    }

    #[test]
    fn is_empty_works_without_revealing() {
        let empty = Secret::from_wire(String::new());
        assert!(empty.is_empty());
        let nonempty = Secret::from_wire("enc2:abc".into());
        assert!(!nonempty.is_empty());
    }

    #[test]
    fn reveal_fails_on_corrupted_enc2_ciphertext() {
        let (_dir, store) = tmp_store(true);
        // Valid prefix but invalid hex / AEAD payload.
        let s = Secret::from_wire("enc2:not-hex-at-all".into());
        assert!(s.reveal(&store).is_err());
    }
}
