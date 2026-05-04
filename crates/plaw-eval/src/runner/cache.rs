//! Judge response cache — wraps the storage `EvalRepo` with a stable key
//! function. Keys are SHA256 over `(prompt, input, model_version)` so that
//! prompt-template changes naturally invalidate.

use std::sync::Arc;

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::storage::EvalRepo;

/// Stable cache key. Inputs are joined with NUL separators so that `("a", "b")`
/// and `("ab", "")` never collide.
pub fn cache_key(prompt: &str, input: &str, model_version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.as_bytes());
    hasher.update(b"\0");
    hasher.update(model_version.as_bytes());
    let digest = hasher.finalize();
    base16(&digest)
}

fn base16(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Thin wrapper that turns the SQLite cache into a `(key) -> response` API
/// while collecting hit/miss stats for telemetry.
pub struct JudgeCache {
    repo: Arc<EvalRepo>,
    hits: std::sync::atomic::AtomicUsize,
    misses: std::sync::atomic::AtomicUsize,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }
}

impl JudgeCache {
    pub fn new(repo: Arc<EvalRepo>) -> Self {
        Self {
            repo,
            hits: 0.into(),
            misses: 0.into(),
        }
    }

    /// Look up a cached judge response. On miss, returns Ok(None) and bumps
    /// the miss counter.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let entry = self.repo.get_cached(key)?;
        if entry.is_some() {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(entry.map(|e| e.judge_response))
    }

    pub fn set(&self, key: &str, response: &str) -> Result<()> {
        self.repo.set_cached(key, response)
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.misses.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_deterministic_and_collision_resistant() {
        let k1 = cache_key("prompt", "input", "model");
        let k2 = cache_key("prompt", "input", "model");
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64); // 32-byte sha256 in hex

        // Collision resistance: shifting the boundary should produce a
        // different key, because we separate fields with NUL.
        let k_a = cache_key("a", "b", "c");
        let k_b = cache_key("ab", "", "c");
        let k_c = cache_key("a", "bc", "");
        assert_ne!(k_a, k_b);
        assert_ne!(k_a, k_c);
        assert_ne!(k_b, k_c);
    }

    #[test]
    fn cache_get_set_with_stats() {
        let repo = Arc::new(EvalRepo::open_in_memory().unwrap());
        let cache = JudgeCache::new(repo);

        // Miss
        assert!(cache.get("k1").unwrap().is_none());
        // Hit after set
        cache.set("k1", "{\"ok\":true}").unwrap();
        let v = cache.get("k1").unwrap().unwrap();
        assert!(v.contains("ok"));

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate() - 0.5).abs() < 1e-12);
    }

    // ─── Property-based tests (proptest) ──────────────────────────────────
    //
    // The hand-test above checks one specific collision case (boundary
    // shift between the prompt and input fields). proptest scales that
    // up to thousands of arbitrary triples to make sure no other input
    // shape collides — the cache_key is load-bearing for the whole
    // re-run-skipping eval pipeline, so a silent collision would cause
    // the wrong score to be served.

    use proptest::prelude::*;

    proptest! {
        /// Determinism: same inputs → same key, every time. Sanity check
        /// that the SHA256 finalisation is order-stable across calls.
        #[test]
        fn cache_key_is_deterministic(
            prompt in ".*",
            input in ".*",
            model in ".*",
        ) {
            let k1 = cache_key(&prompt, &input, &model);
            let k2 = cache_key(&prompt, &input, &model);
            prop_assert_eq!(k1, k2);
        }

        /// Output is a 64-char lowercase hex digest regardless of input
        /// shape (empty / massive / unicode / control chars).
        #[test]
        fn cache_key_shape_is_64_hex(
            prompt in ".*",
            input in ".*",
            model in ".*",
        ) {
            let k = cache_key(&prompt, &input, &model);
            prop_assert_eq!(k.len(), 64);
            prop_assert!(k.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        }

        /// Field-shift collision resistance: moving one byte from the
        /// `prompt` field into `input` MUST produce a different key
        /// (otherwise the NUL separator failed to do its job and the
        /// cache could serve the wrong response when an unrelated
        /// (prompt, input) pair happened to concatenate to the same
        /// total bytes). Skip the trivial degenerate case where the
        /// shift happens to land on a NUL byte that's already there.
        #[test]
        fn cache_key_resists_field_boundary_shift(
            prefix in "[a-zA-Z0-9]{1,20}",
            suffix in "[a-zA-Z0-9]{0,20}",
            model in "[a-zA-Z0-9]{1,12}",
        ) {
            let prompt_a = format!("{prefix}");
            let input_a = format!("{suffix}");
            // Shift one char from prompt to input.
            let prefix_chars: Vec<char> = prefix.chars().collect();
            let split = prefix_chars.len() - 1;
            let prompt_b: String = prefix_chars[..split].iter().collect();
            let mut input_b = String::new();
            input_b.push(prefix_chars[split]);
            input_b.push_str(&suffix);

            let k_a = cache_key(&prompt_a, &input_a, &model);
            let k_b = cache_key(&prompt_b, &input_b, &model);
            prop_assert_ne!(k_a, k_b);
        }

        /// Different models must produce different keys for the same
        /// (prompt, input). Two judge versions must NEVER share a cache
        /// slot — otherwise a model upgrade silently keeps serving the
        /// old model's responses.
        #[test]
        fn cache_key_different_models_diverge(
            prompt in ".*",
            input in ".*",
            model_a in "[a-zA-Z0-9-]{1,30}",
            model_b in "[a-zA-Z0-9-]{1,30}",
        ) {
            prop_assume!(model_a != model_b);
            let k_a = cache_key(&prompt, &input, &model_a);
            let k_b = cache_key(&prompt, &input, &model_b);
            prop_assert_ne!(k_a, k_b);
        }
    }
}
