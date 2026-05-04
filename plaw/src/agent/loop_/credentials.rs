//! Credential / secret scrubbing for tool output and traces.
//!
//! When plaw logs raw provider responses or tool output (for telemetry,
//! observability traces, or scrub-then-history paths), the payload may
//! contain accidentally-leaked tokens / API keys / passwords. The
//! sanitizer here pattern-matches `key: "value"` style assignments
//! and rewrites them to a redacted placeholder while keeping the
//! first 4 chars of the value as a debugging breadcrumb.
//!
//! Why this lives in its own module: the regex is the *security
//! contract* — every change here directly affects what plaw is willing
//! to write into traces. Pinning the regex + behaviour in a
//! dedicated, well-tested file makes drift easy to audit.

use std::sync::LazyLock;

use regex::Regex;

/// Match `(key)["']?\s*[:=]\s*(value)` for known sensitive keys, where
/// `value` is one of:
///   - a double-quoted string of ≥8 chars
///   - a single-quoted string of ≥8 chars
///   - an unquoted token of ≥8 chars from `[A-Za-z0-9_\-\.]`
///
/// The 8-char minimum avoids false positives on "api_key=foo" placeholder
/// examples while still catching real API keys (which are universally
/// longer than that).
static SENSITIVE_KV_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(token|api[_-]?key|password|secret|user[_-]?key|bearer|credential)["']?\s*[:=]\s*(?:"([^"]{8,})"|'([^']{8,})'|([a-zA-Z0-9_\-\.]{8,}))"#).unwrap()
});

/// Scrub credentials from `input` to prevent accidental exfiltration
/// into traces or persisted history. Replaces matched `key: value`
/// pairs with `key: prefix*[REDACTED]`, where `prefix` is the first 4
/// characters of the value (or empty when the value is ≤4 chars). The
/// surrounding quote/separator style (`:` vs `=`, quoted vs not) is
/// preserved so logs stay structurally intact.
///
/// Multibyte safety: prefix collection uses `chars().take(4)` rather
/// than byte slicing, so a value beginning with CJK or emoji won't
/// produce a partial-codepoint string that downstream consumers
/// would have to dance around.
pub(crate) fn scrub_credentials(input: &str) -> String {
    SENSITIVE_KV_REGEX
        .replace_all(input, |caps: &regex::Captures| {
            let full_match = &caps[0];
            let key = &caps[1];
            let val = caps
                .get(2)
                .or(caps.get(3))
                .or(caps.get(4))
                .map(|m| m.as_str())
                .unwrap_or("");

            // Preserve first 4 chars for context, then redact (use char
            // boundaries for multibyte safety).
            let prefix: String = val.chars().take(4).collect();
            let prefix = if val.chars().count() > 4 {
                prefix.as_str()
            } else {
                ""
            };

            if full_match.contains(':') {
                if full_match.contains('"') {
                    format!("\"{key}\": \"{prefix}*[REDACTED]\"")
                } else {
                    format!("{key}: {prefix}*[REDACTED]")
                }
            } else if full_match.contains('=') {
                if full_match.contains('"') {
                    format!("{key}=\"{prefix}*[REDACTED]\"")
                } else {
                    format!("{key}={prefix}*[REDACTED]")
                }
            } else {
                format!("{key}: {prefix}*[REDACTED]")
            }
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Identity / no-op cases ────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(scrub_credentials(""), "");
    }

    #[test]
    fn non_sensitive_text_passes_through_unchanged() {
        let input = "normal text without any secrets";
        assert_eq!(scrub_credentials(input), input);
    }

    #[test]
    fn short_values_under_8_chars_not_redacted() {
        // The regex's value alternation requires ≥8 chars; below that
        // it doesn't match at all (placeholder examples like
        // `api_key="foo"` shouldn't be flagged as real leaks).
        let input = r#"api_key="short""#;
        assert_eq!(scrub_credentials(input), input);
    }

    // ── Redaction shape: prefix preservation + separator/quote style ──

    #[test]
    fn redacts_double_quoted_colon_form_with_prefix() {
        let input = r#"api_key: "abcdefghij1234567890""#;
        let out = scrub_credentials(input);
        assert!(out.contains("api_key"));
        assert!(out.contains("abcd"), "prefix should be preserved: {out}");
        assert!(out.contains("[REDACTED]"));
        // Full secret must NOT appear.
        assert!(!out.contains("abcdefghij1234567890"));
    }

    #[test]
    fn redacts_unquoted_equals_form() {
        // env-var style `KEY=value` (no quotes, no spaces).
        let input = "secret=AKIAIOSFODNN7EXAMPLE";
        let out = scrub_credentials(input);
        assert!(out.contains("secret="));
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn redacts_single_quoted_colon_form() {
        let input = "password: 'mysupersecretpw'";
        let out = scrub_credentials(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("mysupersecretpw"));
    }

    // ── Multiple matches in one input ─────────────────────────────────

    #[test]
    fn redacts_every_match_in_multi_credential_blob() {
        let input = r#"
            api_key: "key12345678"
            password: "passsupersecret"
            non_sensitive: "shown123"
        "#;
        let out = scrub_credentials(input);
        assert_eq!(out.matches("[REDACTED]").count(), 2);
        // Non-sensitive key passes through verbatim.
        assert!(out.contains("non_sensitive: \"shown123\""));
    }

    // ── Key-name variations ───────────────────────────────────────────

    #[test]
    fn matches_dashed_and_underscored_key_variants() {
        // `api_key`, `api-key`, `userkey`, `user-key`, `user_key` are
        // all known leak surfaces — the dash/underscore variants are
        // the ones operator configs typically use.
        for key in ["api_key", "api-key", "user_key", "user-key", "bearer", "token"] {
            let input = format!(r#"{key}: "thissecretvaluehere1234""#);
            let out = scrub_credentials(&input);
            assert!(
                out.contains("[REDACTED]"),
                "{key} must be redacted, got: {out}"
            );
        }
    }

    #[test]
    fn matches_are_case_insensitive() {
        let input = r#"API_KEY: "thissecretvaluehere""#;
        let out = scrub_credentials(input);
        assert!(out.contains("[REDACTED]"));
    }

    // ── Multibyte safety ──────────────────────────────────────────────

    #[test]
    fn multibyte_prefix_does_not_panic() {
        // Hypothetical leak where the value starts with CJK chars.
        // chars().take(4) must operate on char boundaries — slicing
        // bytes here would panic.
        // The regex's value alternation actually only matches
        // [a-zA-Z0-9_\-\.], so CJK won't trip the regex; this test
        // exists to pin the multibyte-safe collection path so a future
        // regex that accepts wider chars stays sound.
        let input = "token: 'aaaa日本語1234'";
        // Expect either redaction or pass-through — both are fine; the
        // critical property is "no panic".
        let _ = scrub_credentials(input);
    }
}
