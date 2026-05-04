//! Generate unique memory keys for per-turn conversation autosave.
//!
//! When the agent loop autosaves user messages and (historically)
//! assistant responses to the long-term Memory backend, it needs a
//! durable, collision-free key per saved entry. The naive approach of
//! using the message timestamp / index breaks down across:
//!
//!   - **concurrent turns from different channels** (Telegram + Discord
//!     can land in the same wall-clock millisecond);
//!   - **process restarts** (a new process picking up the same backend
//!     would otherwise re-key over a previous run's last entry);
//!   - **rapid back-to-back user input** (slash commands, tool-cancel
//!     racing the next user line).
//!
//! Using a UUID v4 suffix sidesteps all three: the 122 random bits put
//! the practical collision probability below any meaningful threshold,
//! and the prefix lets us still scan / filter entries by category
//! (`user_msg_*`, `assistant_resp_*`, etc.) without an extra index.
//!
//! See `build_context_ignores_legacy_assistant_autosave_entries` in
//! `loop_.rs` for the legacy-prefix migration story — older runs wrote
//! `assistant_resp_*` autosaves which are now ignored when rebuilding
//! conversational context, but the prefix itself is still meaningful
//! for debug-time scans.

use uuid::Uuid;

/// Build a memory key of the form `{prefix}_{uuid_v4}` for a single
/// autosave entry. The UUID is freshly generated per call, so two
/// invocations with the same prefix are guaranteed (modulo the UUID v4
/// collision space) to produce distinct keys.
///
/// The empty / underscore-bearing prefixes are passed through verbatim;
/// no normalisation happens here. Callers that care about a specific
/// shape (e.g. lowercase, no whitespace) must validate at their own
/// call site.
pub(super) fn autosave_memory_key(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard UUID v4 hyphenated form: 8-4-4-4-12 hex digits + 4
    // hyphens = 36 chars. Pinning this here means a future swap of the
    // UUID library / format would have to consciously update the test.
    const UUID_V4_LEN: usize = 36;

    #[test]
    fn key_starts_with_given_prefix_and_separator() {
        let key = autosave_memory_key("user_msg");
        assert!(
            key.starts_with("user_msg_"),
            "expected `user_msg_` prefix, got {key:?}"
        );
    }

    #[test]
    fn two_calls_with_same_prefix_produce_distinct_keys() {
        // The whole point of this helper — concurrent turns must not
        // collide on the memory backend, even when they share a prefix.
        let a = autosave_memory_key("user_msg");
        let b = autosave_memory_key("user_msg");
        assert_ne!(a, b);
    }

    #[test]
    fn many_calls_remain_distinct() {
        // Stronger version of the pairwise test: 1000 keys with the
        // same prefix must all be distinct. UUID v4 makes this a
        // formality, but pinning it here catches a degenerate change
        // (e.g. accidentally reusing a single Uuid::new_v4() handle).
        let n = 1000;
        let keys: std::collections::HashSet<String> =
            (0..n).map(|_| autosave_memory_key("k")).collect();
        assert_eq!(keys.len(), n, "all {n} keys must be distinct");
    }

    #[test]
    fn key_format_is_prefix_underscore_uuidv4() {
        // Shape contract: `<prefix>_<uuid-v4-hyphenated>`. Anything
        // else (e.g. a UUID without hyphens, an extra suffix) would
        // break legacy-key scanners that look for hyphenated forms.
        let key = autosave_memory_key("user_msg");
        let suffix = key.strip_prefix("user_msg_").expect("prefix must match");
        assert_eq!(suffix.len(), UUID_V4_LEN, "uuid suffix must be {UUID_V4_LEN} chars, got {suffix:?}");
        // 5 segments separated by 4 hyphens.
        assert_eq!(suffix.matches('-').count(), 4);
        // Parse-roundtrip catches any non-canonical UUID encoding.
        assert!(
            uuid::Uuid::parse_str(suffix).is_ok(),
            "suffix must parse as UUID, got {suffix:?}"
        );
    }

    #[test]
    fn empty_prefix_passes_through_verbatim() {
        // Callers that opt into an empty prefix get `_<uuid>` rather
        // than a normalised form. Documenting via test, not policy.
        let key = autosave_memory_key("");
        assert!(key.starts_with('_'));
        assert_eq!(key.len(), 1 + UUID_V4_LEN);
    }

    #[test]
    fn prefix_with_internal_underscore_is_preserved() {
        // Production prefixes like `user_msg` and `assistant_resp`
        // already contain underscores; the helper must not collapse
        // or escape them, otherwise legacy key scans break.
        let key = autosave_memory_key("user_msg");
        // Exactly one trailing `_<uuid>`, but the prefix contributes
        // its own `_` — so total `_` count = prefix-internal + 1
        // separator + 4 inside the UUID.
        let underscore_count = key.matches('_').count();
        assert_eq!(underscore_count, 1 /*prefix*/ + 1 /*separator*/);
    }
}
