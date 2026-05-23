//! Security subsystem for policy enforcement, sandboxing, and secret management.
//!
//! This module provides the security infrastructure for Plaw. The core type
//! [`SecurityPolicy`] defines autonomy levels, workspace boundaries, and
//! access-control rules that are enforced across the tool and runtime subsystems.
//! [`PairingGuard`] implements device pairing for channel authentication, and
//! [`SecretStore`] handles encrypted credential storage.
//!
//! OS-level isolation is provided through the [`Sandbox`] trait defined in
//! [`traits`], with pluggable backends including Docker, Firejail, Bubblewrap,
//! and Landlock. The [`create_sandbox`] function selects the best available
//! backend at runtime. An [`AuditLogger`] records security-relevant events for
//! forensic review.
//!
//! # Extension
//!
//! To add a new sandbox backend, implement [`Sandbox`] in a new submodule and
//! register it in [`detect::create_sandbox`]. See `AGENTS.md` §7.5 for security
//! change guidelines.

pub mod audit;
#[cfg(feature = "sandbox-bubblewrap")]
pub mod bubblewrap;
pub mod detect;
pub mod docker;

// Prompt injection defense (contributed from RustyClaw, MIT licensed)
pub mod domain_matcher;
pub mod estop;
#[cfg(target_os = "linux")]
pub mod firejail;
#[cfg(feature = "sandbox-landlock")]
pub mod landlock;
pub mod leak_detector;
pub mod otp;
pub mod pairing;
pub mod policy;
pub mod prompt_guard;
pub mod secrets;
pub mod syscall_anomaly;
pub mod traits;

#[allow(unused_imports)]
pub use audit::{AuditEvent, AuditEventType, AuditLogger};
#[allow(unused_imports)]
pub use detect::create_sandbox;
pub use domain_matcher::DomainMatcher;
#[allow(unused_imports)]
pub use estop::{EstopLevel, EstopManager, EstopState, ResumeSelector};
#[allow(unused_imports)]
pub use otp::OtpValidator;
#[allow(unused_imports)]
pub use pairing::PairingGuard;
pub use policy::{AutonomyLevel, SecurityPolicy};
#[allow(unused_imports)]
pub use secrets::SecretStore;
#[allow(unused_imports)]
pub use syscall_anomaly::{SyscallAnomalyAlert, SyscallAnomalyDetector, SyscallAnomalyKind};
#[allow(unused_imports)]
pub use traits::{NoopSandbox, Sandbox};
// Prompt injection defense exports
pub use leak_detector::{LeakDetector, LeakResult};
#[allow(unused_imports)]
pub use prompt_guard::{GuardAction, GuardResult, PromptGuard};

/// Redact sensitive values for safe logging. Shows first 4 chars + "***" suffix.
/// This function intentionally breaks the data-flow taint chain for static analysis.
/// Currently no in-tree caller uses this helper directly; outbound user-visible
/// content goes through [`scrub_outbound`] (regex-based) and inbound tool results
/// go through `crate::agent::loop_::credentials::scrub_credentials`. Kept as the
/// canonical crate-level redact primitive for future call sites that need
/// last-4-only style masking (e.g., displaying which key was rotated).
#[allow(dead_code)]
pub fn redact(value: &str) -> String {
    if value.len() <= 4 {
        "***".to_string()
    } else {
        format!("{}***", &value[..4])
    }
}

/// Scrub outbound user-visible content for credential leaks before it crosses
/// a network boundary (WebSocket `done` event, channel message send, draft
/// finalize). Returns the original content when clean; otherwise the redacted
/// variant produced by [`LeakDetector`].
///
/// This is the outbound counterpart to
/// `crate::agent::loop_::credentials::scrub_credentials` (which protects the
/// inbound tool-result → LLM direction). The detector is a per-process
/// singleton so the regex compilation cost is paid once.
///
/// On detection a `tracing::warn!` records the pattern names (not the
/// content). The function is infallible — a clean string passes through with
/// zero allocation beyond `to_string`.
pub fn scrub_outbound(content: &str) -> String {
    use std::sync::OnceLock;
    static DETECTOR: OnceLock<LeakDetector> = OnceLock::new();
    let detector = DETECTOR.get_or_init(LeakDetector::new);
    match detector.scan(content) {
        LeakResult::Clean => content.to_string(),
        LeakResult::Detected { patterns, redacted } => {
            tracing::warn!(
                patterns = ?patterns,
                bytes_in = content.len(),
                bytes_out = redacted.len(),
                "outbound credential leak detected and redacted"
            );
            redacted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexported_policy_and_pairing_types_are_usable() {
        let policy = SecurityPolicy::default();
        assert_eq!(policy.autonomy, AutonomyLevel::Supervised);

        let guard = PairingGuard::new(false, &[]);
        assert!(!guard.require_pairing());
    }

    #[test]
    fn reexported_secret_store_encrypt_decrypt_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let store = SecretStore::new(temp.path(), false);

        let encrypted = store.encrypt("top-secret").unwrap();
        let decrypted = store.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted, "top-secret");
    }

    #[test]
    fn redact_hides_most_of_value() {
        assert_eq!(redact("abcdefgh"), "abcd***");
        assert_eq!(redact("ab"), "***");
        assert_eq!(redact(""), "***");
        assert_eq!(redact("12345"), "1234***");
    }

    #[test]
    fn scrub_outbound_passes_clean_content_unchanged() {
        let s = "Plaw agent reply: file written to ./output.txt successfully.";
        assert_eq!(scrub_outbound(s), s);
    }

    #[test]
    fn scrub_outbound_redacts_anthropic_key() {
        let leaked = format!(
            "Found in env: sk-ant-{}",
            "A".repeat(40)
        );
        let scrubbed = scrub_outbound(&leaked);
        assert!(scrubbed.contains("[REDACTED_API_KEY]"));
        assert!(!scrubbed.contains("sk-ant-AAAA"));
    }

    #[test]
    fn scrub_outbound_redacts_openai_style_key() {
        let leaked = format!("export OPENAI_KEY=sk-{}", "B".repeat(48));
        let scrubbed = scrub_outbound(&leaked);
        assert!(scrubbed.contains("[REDACTED_API_KEY]"));
    }

    #[test]
    fn scrub_outbound_redacts_github_pat() {
        let leaked = format!("token: github_pat_{}", "C".repeat(30));
        let scrubbed = scrub_outbound(&leaked);
        assert!(scrubbed.contains("[REDACTED_API_KEY]"));
    }

    #[test]
    fn scrub_outbound_redacts_aws_access_key_id() {
        // AWS-documented example KID
        let leaked = "AWS access key: AKIAIOSFODNN7EXAMPLE";
        let scrubbed = scrub_outbound(leaked);
        assert!(scrubbed.contains("[REDACTED_AWS_CREDENTIAL]"));
        assert!(!scrubbed.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn scrub_outbound_redacts_jwt_bearer() {
        // Fake three-segment JWT-shaped token
        let leaked = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJwbGF3X3VzZXIifQ.fake_signature_xxx";
        let scrubbed = scrub_outbound(leaked);
        assert!(scrubbed.contains("[REDACTED_JWT]"));
    }

    #[test]
    fn scrub_outbound_redacts_postgres_url_with_password() {
        let leaked = "DATABASE_URL=postgres://plaw_user:supersecret@db.internal:5432/plaw";
        let scrubbed = scrub_outbound(leaked);
        assert!(scrubbed.contains("[REDACTED_DATABASE_URL]"));
        assert!(!scrubbed.contains("supersecret"));
    }

    #[test]
    fn scrub_outbound_redacts_pem_private_key() {
        let leaked = "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAAS...\n-----END RSA PRIVATE KEY-----\n";
        let scrubbed = scrub_outbound(leaked);
        assert!(scrubbed.contains("[REDACTED_PRIVATE_KEY]"));
        assert!(!scrubbed.contains("MIIEvgIBADAN"));
    }

    #[test]
    fn scrub_outbound_ignores_benign_password_mention() {
        // Plain prose mentioning the word "password" must not trigger —
        // the regex requires `password=…` / `password: …` followed by a
        // value of at least 8 chars.
        let s = "Reminder: your password should be strong and unique.";
        assert_eq!(scrub_outbound(s), s);
    }
}
