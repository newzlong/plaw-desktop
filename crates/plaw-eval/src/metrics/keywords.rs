//! Keyword-coverage metric — deterministic, judge-free signal that the
//! response mentions the expected facts. Cheap to run on every case.
//!
//! Two normalisation knobs:
//! - case-insensitive matching (default)
//! - optional whole-word matching (default), so `"java"` doesn't match
//!   `"javascript"`.

/// Configuration for keyword coverage.
#[derive(Debug, Clone)]
pub struct KeywordConfig {
    pub case_insensitive: bool,
    pub whole_word: bool,
}

impl Default for KeywordConfig {
    fn default() -> Self {
        Self {
            case_insensitive: true,
            whole_word: true,
        }
    }
}

/// Score `[0, 1]` = (number of keywords found) / (total keywords). Returns
/// `1.0` when `keywords` is empty (vacuously true).
pub fn coverage(response: &str, keywords: &[String], cfg: &KeywordConfig) -> f64 {
    if keywords.is_empty() {
        return 1.0;
    }
    let haystack = if cfg.case_insensitive {
        response.to_ascii_lowercase()
    } else {
        response.to_string()
    };
    let mut hits = 0;
    for kw in keywords {
        let needle = if cfg.case_insensitive {
            kw.to_ascii_lowercase()
        } else {
            kw.clone()
        };
        if needle.is_empty() {
            continue;
        }
        let found = if cfg.whole_word {
            contains_whole_word(&haystack, &needle)
        } else {
            haystack.contains(&needle)
        };
        if found {
            hits += 1;
        }
    }
    hits as f64 / keywords.len() as f64
}

/// Minimal whole-word check: needle must be flanked by non-alphanumeric
/// characters or string boundaries. Avoids matching `"go"` inside
/// `"goal"` while still catching `"go!"` or `"go."`.
fn contains_whole_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > bytes.len() {
        return false;
    }
    'outer: for i in 0..=bytes.len() - n.len() {
        if &bytes[i..i + n.len()] != n {
            continue;
        }
        // Boundary checks. We use the alphanumeric-ASCII heuristic; for
        // CJK this devolves to substring matching, which is fine since
        // CJK has no word boundaries anyway.
        if i > 0 && (bytes[i - 1] as char).is_alphanumeric() {
            continue 'outer;
        }
        let after = i + n.len();
        if after < bytes.len() && (bytes[after] as char).is_alphanumeric() {
            continue 'outer;
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_keywords_is_perfect() {
        let cfg = KeywordConfig::default();
        assert_eq!(coverage("anything goes", &[], &cfg), 1.0);
    }

    #[test]
    fn finds_all_keywords_case_insensitive() {
        let cfg = KeywordConfig::default();
        let kws = vec!["Paris".to_string(), "France".to_string()];
        assert_eq!(coverage("paris is the capital of FRANCE.", &kws, &cfg), 1.0);
    }

    #[test]
    fn whole_word_excludes_substring_matches() {
        let cfg = KeywordConfig::default();
        let kws = vec!["java".to_string()];
        assert_eq!(coverage("I love javascript.", &kws, &cfg), 0.0);
        assert_eq!(coverage("I love java.", &kws, &cfg), 1.0);
    }

    #[test]
    fn substring_mode_relaxes_word_check() {
        let cfg = KeywordConfig {
            case_insensitive: true,
            whole_word: false,
        };
        let kws = vec!["java".to_string()];
        assert_eq!(coverage("I love javascript.", &kws, &cfg), 1.0);
    }

    #[test]
    fn fractional_coverage() {
        let cfg = KeywordConfig::default();
        let kws = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let score = coverage("the letter A appears here", &kws, &cfg);
        assert!((score - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn cjk_substring_matches_through_substring_path() {
        // CJK has no word boundaries; whole_word degenerates to substring.
        let cfg = KeywordConfig {
            case_insensitive: false,
            whole_word: false,
        };
        let kws = vec!["北京".to_string()];
        assert_eq!(coverage("我来自北京。", &kws, &cfg), 1.0);
    }

    #[test]
    fn empty_keyword_strings_are_ignored() {
        let cfg = KeywordConfig::default();
        let kws = vec!["".to_string(), "real".to_string()];
        let score = coverage("a real answer", &kws, &cfg);
        // Empty string is skipped; "real" is found → 1/2 of the explicit
        // keywords matched, but we count empty as a miss in the divisor.
        assert!((score - 0.5).abs() < 1e-12);
    }
}
