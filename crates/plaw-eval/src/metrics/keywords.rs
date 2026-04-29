//! Keyword-coverage metric — deterministic, judge-free signal that the
//! response mentions the expected facts. Cheap to run on every case.
//!
//! Three normalisation knobs:
//! - case-insensitive matching (default)
//! - optional whole-word matching (default), so `"java"` doesn't match
//!   `"javascript"`.
//! - **synonym groups** — a keyword can be a `|`-separated list of
//!   alternatives, e.g. `"不知道|无法|没有信息"`. The keyword counts as
//!   hit iff *any* alternative is found. Lets cases tolerate wording
//!   variation without listing every paraphrase as a separate slot.

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
        // A keyword may be a `|`-separated list of synonyms. Hit if any matches.
        let alternatives: Vec<String> = kw
            .split('|')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                if cfg.case_insensitive {
                    s.to_ascii_lowercase()
                } else {
                    s.to_string()
                }
            })
            .collect();
        if alternatives.is_empty() {
            continue;
        }
        let any_found = alternatives.iter().any(|needle| {
            if cfg.whole_word {
                contains_whole_word(&haystack, needle)
            } else {
                haystack.contains(needle)
            }
        });
        if any_found {
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
    if needle.len() > haystack.len() {
        return false;
    }
    // Walk match positions on str, then check the *char* before and after
    // (not the raw byte) so multi-byte UTF-8 doesn't get mistaken for a
    // letter. For CJK the chars are non-alphanumeric so whole_word
    // degenerates to substring matching as advertised.
    for (i, _) in haystack.match_indices(needle) {
        // Boundary check uses *ASCII* alphanumeric only. CJK / accented
        // letters / etc. are alphabetic in Unicode but for our purposes
        // they're word boundaries (CJK has none, accented runs of Latin
        // are rare in keyword cases). Using is_ascii_alphanumeric keeps
        // English whole-word semantics while letting CJK pass through.
        let before_ok = if i == 0 {
            true
        } else {
            !haystack[..i]
                .chars()
                .next_back()
                .map(|c| c.is_ascii_alphanumeric())
                .unwrap_or(false)
        };
        if !before_ok {
            continue;
        }
        let after_idx = i + needle.len();
        let after_ok = if after_idx >= haystack.len() {
            true
        } else {
            !haystack[after_idx..]
                .chars()
                .next()
                .map(|c| c.is_ascii_alphanumeric())
                .unwrap_or(false)
        };
        if after_ok {
            return true;
        }
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
    fn cjk_works_with_whole_word_too() {
        // Regression: byte-level boundary check used to mistake CJK
        // continuation bytes for Latin letters and reject every CJK
        // match. Boundary check is now char-aware.
        let cfg = KeywordConfig::default(); // whole_word = true
        let kws = vec!["不能".to_string()];
        assert_eq!(coverage("不能。这是一个经典问题。", &kws, &cfg), 1.0);
        let kws2 = vec!["量子".to_string()];
        assert_eq!(coverage("量子力学是描述微观粒子的理论。", &kws2, &cfg), 1.0);
    }

    #[test]
    fn synonym_groups_match_any_alternative() {
        let cfg = KeywordConfig::default();
        // Single slot with three synonyms — any one hits.
        let kws = vec!["不知道|无法|没有信息".to_string()];
        assert_eq!(coverage("我无法知道你昨天做了什么。", &kws, &cfg), 1.0);
        assert_eq!(coverage("没有信息可供参考。", &kws, &cfg), 1.0);
        assert_eq!(coverage("我能回答这个问题。", &kws, &cfg), 0.0);
    }

    #[test]
    fn synonym_groups_count_as_one_slot() {
        let cfg = KeywordConfig::default();
        // Two slots: one with synonyms, one literal. Both must hit for 1.0.
        let kws = vec!["不知道|无法".to_string(), "抱歉".to_string()];
        assert_eq!(coverage("抱歉，我无法回答。", &kws, &cfg), 1.0);
        assert!((coverage("我无法回答。", &kws, &cfg) - 0.5).abs() < 1e-12);
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
