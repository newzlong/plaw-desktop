//! Utility functions for `Plaw`.
//!
//! This module contains reusable helper functions used across the codebase.

/// Truncate a string to at most `max_chars` characters, appending "..." if truncated.
///
/// This function safely handles multi-byte UTF-8 characters (emoji, CJK, accented characters)
/// by using character boundaries instead of byte indices.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_chars` - Maximum number of characters to keep (excluding "...")
///
/// # Returns
/// * Original string if length <= `max_chars`
/// * Truncated string with "..." appended if length > `max_chars`
///
/// # Examples
/// ```ignore
/// use plaw::util::truncate_with_ellipsis;
///
/// // ASCII string - no truncation needed
/// assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
///
/// // ASCII string - truncation needed
/// assert_eq!(truncate_with_ellipsis("hello world", 5), "hello...");
///
/// // Multi-byte UTF-8 (emoji) - safe truncation
/// assert_eq!(truncate_with_ellipsis("Hello 🦀 World", 8), "Hello 🦀...");
/// assert_eq!(truncate_with_ellipsis("😀😀😀😀", 2), "😀😀...");
///
/// // Empty string
/// assert_eq!(truncate_with_ellipsis("", 10), "");
/// ```
pub fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => {
            let truncated = &s[..idx];
            // Trim trailing whitespace for cleaner output
            format!("{}...", truncated.trim_end())
        }
        None => s.to_string(),
    }
}

/// Return the greatest valid UTF-8 char boundary at or below `index`.
///
/// This mirrors `str::floor_char_boundary` behavior while remaining compatible
/// with stable toolchains where that API is not available.
pub fn floor_utf8_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }

    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Utility enum for handling optional values.
pub enum MaybeSet<T> {
    Set(T),
    Unset,
    Null,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_ascii_no_truncation() {
        // ASCII string shorter than limit - no change
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
        assert_eq!(truncate_with_ellipsis("hello world", 50), "hello world");
    }

    #[test]
    fn test_truncate_ascii_with_truncation() {
        // ASCII string longer than limit - truncates
        assert_eq!(truncate_with_ellipsis("hello world", 5), "hello...");
        assert_eq!(
            truncate_with_ellipsis("This is a long message", 10),
            "This is a..."
        );
    }

    #[test]
    fn test_truncate_empty_string() {
        assert_eq!(truncate_with_ellipsis("", 10), "");
    }

    #[test]
    fn test_truncate_at_exact_boundary() {
        // String exactly at boundary - no truncation
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_emoji_single() {
        // Single emoji (4 bytes) - should not panic
        let s = "🦀";
        assert_eq!(truncate_with_ellipsis(s, 10), s);
        assert_eq!(truncate_with_ellipsis(s, 1), s);
    }

    #[test]
    fn test_truncate_emoji_multiple() {
        // Multiple emoji - safe truncation at character boundary
        let s = "😀😀😀😀"; // 4 emoji, each 4 bytes = 16 bytes total
        assert_eq!(truncate_with_ellipsis(s, 2), "😀😀...");
        assert_eq!(truncate_with_ellipsis(s, 3), "😀😀😀...");
    }

    #[test]
    fn test_truncate_mixed_ascii_emoji() {
        // Mixed ASCII and emoji
        assert_eq!(truncate_with_ellipsis("Hello 🦀 World", 8), "Hello 🦀...");
        assert_eq!(truncate_with_ellipsis("Hi 😊", 10), "Hi 😊");
    }

    #[test]
    fn test_truncate_cjk_characters() {
        // CJK characters (Chinese - each is 3 bytes)
        let s = "这是一个测试消息用来触发崩溃的中文"; // 21 characters
        let result = truncate_with_ellipsis(s, 16);
        assert!(result.ends_with("..."));
        assert!(result.is_char_boundary(result.len() - 1));
    }

    #[test]
    fn test_truncate_accented_characters() {
        // Accented characters (2 bytes each in UTF-8)
        let s = "café résumé naïve";
        assert_eq!(truncate_with_ellipsis(s, 10), "café résum...");
    }

    #[test]
    fn test_truncate_unicode_edge_case() {
        // Mix of 1-byte, 2-byte, 3-byte, and 4-byte characters
        let s = "aé你好🦀"; // 1 + 1 + 2 + 2 + 4 bytes = 10 bytes, 5 chars
        assert_eq!(truncate_with_ellipsis(s, 3), "aé你...");
    }

    #[test]
    fn test_truncate_long_string() {
        // Long ASCII string
        let s = "a".repeat(200);
        let result = truncate_with_ellipsis(&s, 50);
        assert_eq!(result.len(), 53); // 50 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_zero_max_chars() {
        // Edge case: max_chars = 0
        assert_eq!(truncate_with_ellipsis("hello", 0), "...");
    }

    #[test]
    fn test_floor_utf8_char_boundary_ascii() {
        assert_eq!(floor_utf8_char_boundary("hello", 0), 0);
        assert_eq!(floor_utf8_char_boundary("hello", 3), 3);
        assert_eq!(floor_utf8_char_boundary("hello", 99), 5);
    }

    #[test]
    fn test_floor_utf8_char_boundary_multibyte() {
        let s = "aé你🦀";
        assert_eq!(floor_utf8_char_boundary(s, 1), 1);
        // Index 2 is inside "é" (2-byte char), floor should move back to 1.
        assert_eq!(floor_utf8_char_boundary(s, 2), 1);
        // Index 5 is inside "你" (3-byte char), floor should move back to 3.
        assert_eq!(floor_utf8_char_boundary(s, 5), 3);
    }

    // ─── Property-based tests (proptest) ──────────────────────────────────
    //
    // The hand-written tests above cover specific shapes (ASCII, emoji,
    // CJK, accented). proptest fills in the gaps by generating thousands
    // of arbitrary `(s, max_chars)` pairs — including cases the author
    // didn't think of (interleaved scripts, zero-width joiners, long
    // single-char strings, empty + boundary combinations).

    use proptest::prelude::*;

    proptest! {
        /// truncate_with_ellipsis must never panic on any (s, max_chars) input,
        /// and its output must always be valid UTF-8 (the type system guarantees
        /// the latter — this property documents the invariant explicitly).
        #[test]
        fn truncate_never_panics_and_returns_valid_utf8(
            s in ".*",
            max_chars in 0usize..1024,
        ) {
            let out = truncate_with_ellipsis(&s, max_chars);
            // String is always valid UTF-8 by construction; checking
            // is_char_boundary at len() proves the trailing edge is sane.
            prop_assert!(out.is_char_boundary(out.len()));
        }

        /// When the input fits, the output is the input unchanged. Hand-tests
        /// cover specific lengths; proptest sweeps the full short-input space.
        #[test]
        fn truncate_short_input_returned_verbatim(
            s in ".{0,40}",
            extra in 0usize..32,
        ) {
            let n = s.chars().count();
            let max_chars = n + extra; // input fits with room to spare
            prop_assert_eq!(truncate_with_ellipsis(&s, max_chars), s.clone());
        }

        /// When the input is strictly longer than `max_chars`, the output
        /// must end with the ellipsis suffix. The trailing "..." is the
        /// signal that downstream UI uses to indicate "more content
        /// available" — silently dropping it on a long input would break
        /// log formatting and tracing payload truncation.
        #[test]
        fn truncate_long_input_has_ellipsis_suffix(
            s in ".{1,200}",
            // Bound max_chars below the input length so we always truncate.
            max_chars in 0usize..1,
        ) {
            // Pick max_chars strictly less than the input's char count.
            let n = s.chars().count();
            prop_assume!(n > 0);
            let max_chars = max_chars.min(n.saturating_sub(1));
            prop_assume!(n > max_chars);
            let out = truncate_with_ellipsis(&s, max_chars);
            prop_assert!(out.ends_with("..."), "expected trailing '...' in {out:?}");
        }

        /// floor_utf8_char_boundary: result is always a valid char boundary,
        /// always ≤ requested index, and always ≤ s.len(). These together
        /// guarantee `&s[..result]` never panics.
        #[test]
        fn floor_boundary_is_safe_slice_index(
            s in ".*",
            index in 0usize..256,
        ) {
            let r = floor_utf8_char_boundary(&s, index);
            prop_assert!(r <= s.len());
            prop_assert!(r <= index || r == s.len());
            prop_assert!(s.is_char_boundary(r));
            // Smoke: actually slicing must not panic.
            let _ = &s[..r];
        }

        /// floor_utf8_char_boundary is idempotent: applying it to its own
        /// output yields the same value. Boundary-snapping should be a
        /// fixed-point operation when the input is already a boundary.
        #[test]
        fn floor_boundary_is_idempotent(
            s in ".*",
            index in 0usize..256,
        ) {
            let r1 = floor_utf8_char_boundary(&s, index);
            let r2 = floor_utf8_char_boundary(&s, r1);
            prop_assert_eq!(r1, r2);
        }
    }
}
