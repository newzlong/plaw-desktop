use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A single memory entry.
///
/// The three trailing `Option<String>` fields (`valid_from`, `valid_to`,
/// `supersedes_id`) form the bi-temporal foundation. They are all
/// `#[serde(default)]` so JSON / sqlite / markdown rows written before
/// PR #74 deserialize unchanged with all three as `None`. Today only
/// the type-system surface exists; real bi-temporal semantics (auto-
/// supersede on key collision, `WHERE valid_to IS NULL` read filter,
/// `recall_as_of` time travel) land in PR #75 on `SqliteMemory`.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub score: Option<f64>,
    /// RFC3339 instant the fact became true in the world. `None` for
    /// legacy rows / non-bi-temporal backends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    /// RFC3339 instant the fact stopped being true. `None` means the
    /// fact is currently believed to hold. Backends that participate in
    /// bi-temporal filtering MUST exclude rows where `valid_to <= now`
    /// from default reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    /// Id of the row this entry replaced, when stored via
    /// [`Memory::supersede`] or via key-collision auto-supersession on a
    /// bi-temporal backend. `None` for first-of-key entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<String>,
}

impl std::fmt::Debug for MemoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryEntry")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("content", &self.content)
            .field("category", &self.category)
            .field("timestamp", &self.timestamp)
            .field("score", &self.score)
            .field("valid_from", &self.valid_from)
            .field("valid_to", &self.valid_to)
            .field("supersedes_id", &self.supersedes_id)
            .finish_non_exhaustive()
    }
}

/// Memory categories for organization
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    /// Long-term facts, preferences, decisions
    #[default]
    Core,
    /// Daily session logs
    Daily,
    /// Conversation context
    Conversation,
    /// User-defined custom category
    Custom(String),
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core => write!(f, "core"),
            Self::Daily => write!(f, "daily"),
            Self::Conversation => write!(f, "conversation"),
            Self::Custom(name) => write!(f, "{name}"),
        }
    }
}

/// Core memory trait — implement for any persistence backend.
///
/// Bi-temporal contract (effective PR #74 — implementations land in
/// follow-ups):
///
/// - [`Self::store`] inserts a fresh row. Backends that participate in
///   bi-temporal semantics SHOULD auto-supersede an existing live row
///   that shares the same `key` (stamp its `valid_to`, link the new
///   row's `supersedes_id`). Backends without this capability silently
///   keep the legacy "overwrite on key collision" or "append" behavior.
/// - [`Self::recall`], [`Self::get`], [`Self::list`] MUST return only
///   currently-true rows (those where `valid_to IS NULL`). Bi-temporal
///   backends enforce this in the query; non-bi-temporal backends meet
///   it trivially because they never set `valid_to`.
/// - [`Self::supersede`] explicitly links a new row to an old one,
///   useful when the new content lives under a DIFFERENT key from the
///   old row (cross-key supersession). Default impl forwards to
///   [`Self::store`] for backends without bi-temporal storage.
/// - [`Self::recall_as_of`] returns rows that were currently-true at a
///   given instant. Default impl forwards to [`Self::recall`] —
///   non-bi-temporal backends return present-day truth regardless of
///   the requested timestamp.
#[async_trait]
pub trait Memory: Send + Sync {
    /// Backend name
    fn name(&self) -> &str;

    /// Store a memory entry, optionally scoped to a session
    async fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Recall memories matching a query (keyword search), optionally scoped to a session
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Get a specific memory by key
    async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>>;

    /// List all memory keys, optionally filtered by category and/or session
    async fn list(
        &self,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Remove a memory by key
    async fn forget(&self, key: &str) -> anyhow::Result<bool>;

    /// Count total memories
    async fn count(&self) -> anyhow::Result<usize>;

    /// Health check
    async fn health_check(&self) -> bool;

    /// Bi-temporal: mark `old_id` as superseded (stamp `valid_to = now`)
    /// and insert `new_content` as the live version. Use when the new
    /// content lives under a DIFFERENT key from the old row; key-aligned
    /// supersession is handled transparently by [`Self::store`] on
    /// bi-temporal backends.
    ///
    /// Default impl forwards to [`Self::store`] — non-bi-temporal
    /// backends silently lose the supersession link but keep the new
    /// entry. Returns `Ok(())` when the new entry lands; backends MAY
    /// surface "old_id not found" as an error.
    async fn supersede(
        &self,
        old_id: &str,
        key: &str,
        new_content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let _ = old_id;
        self.store(key, new_content, category, session_id).await
    }

    /// Bi-temporal time travel: recall memories that were currently true
    /// at the given RFC3339 instant. Bi-temporal backends filter rows by
    /// `valid_from <= as_of AND (valid_to IS NULL OR valid_to > as_of)`.
    ///
    /// Default impl forwards to [`Self::recall`] — non-bi-temporal
    /// backends return present-day truth regardless of `as_of`.
    async fn recall_as_of(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
        as_of: &str,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let _ = as_of;
        self.recall(query, limit, session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_category_display_outputs_expected_values() {
        assert_eq!(MemoryCategory::Core.to_string(), "core");
        assert_eq!(MemoryCategory::Daily.to_string(), "daily");
        assert_eq!(MemoryCategory::Conversation.to_string(), "conversation");
        assert_eq!(
            MemoryCategory::Custom("project_notes".into()).to_string(),
            "project_notes"
        );
    }

    #[test]
    fn memory_category_serde_uses_snake_case() {
        let core = serde_json::to_string(&MemoryCategory::Core).unwrap();
        let daily = serde_json::to_string(&MemoryCategory::Daily).unwrap();
        let conversation = serde_json::to_string(&MemoryCategory::Conversation).unwrap();

        assert_eq!(core, "\"core\"");
        assert_eq!(daily, "\"daily\"");
        assert_eq!(conversation, "\"conversation\"");
    }

    #[test]
    fn memory_category_default_is_core() {
        assert_eq!(MemoryCategory::default(), MemoryCategory::Core);
    }

    #[test]
    fn memory_entry_default_has_empty_strings_and_none_bitemporal() {
        let e = MemoryEntry::default();
        assert!(e.id.is_empty());
        assert!(e.key.is_empty());
        assert!(e.content.is_empty());
        assert_eq!(e.category, MemoryCategory::Core);
        assert!(e.valid_from.is_none());
        assert!(e.valid_to.is_none());
        assert!(e.supersedes_id.is_none());
    }

    #[test]
    fn memory_entry_roundtrip_preserves_optional_fields() {
        let entry = MemoryEntry {
            id: "id-1".into(),
            key: "favorite_language".into(),
            content: "Rust".into(),
            category: MemoryCategory::Core,
            timestamp: "2026-02-16T00:00:00Z".into(),
            session_id: Some("session-abc".into()),
            score: Some(0.98),
            valid_from: Some("2026-02-16T00:00:00Z".into()),
            valid_to: None,
            supersedes_id: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: MemoryEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "id-1");
        assert_eq!(parsed.key, "favorite_language");
        assert_eq!(parsed.content, "Rust");
        assert_eq!(parsed.category, MemoryCategory::Core);
        assert_eq!(parsed.session_id.as_deref(), Some("session-abc"));
        assert_eq!(parsed.score, Some(0.98));
        assert_eq!(parsed.valid_from.as_deref(), Some("2026-02-16T00:00:00Z"));
        assert!(parsed.valid_to.is_none());
        assert!(parsed.supersedes_id.is_none());
    }

    #[test]
    fn memory_entry_deserialises_legacy_json_without_bitemporal_fields() {
        // Hand-crafted JSON missing the three bi-temporal fields — must
        // deserialise cleanly with #[serde(default)] giving all None.
        let legacy = r#"{
            "id": "legacy-1",
            "key": "preference",
            "content": "dark mode",
            "category": "core",
            "timestamp": "2026-01-01T00:00:00Z",
            "session_id": null,
            "score": null
        }"#;
        let parsed: MemoryEntry = serde_json::from_str(legacy).expect("legacy json must parse");
        assert_eq!(parsed.id, "legacy-1");
        assert!(parsed.valid_from.is_none());
        assert!(parsed.valid_to.is_none());
        assert!(parsed.supersedes_id.is_none());
    }

    #[test]
    fn memory_entry_serialised_omits_none_bitemporal_fields() {
        // skip_serializing_if keeps the wire format identical to pre-PR
        // for entries that never touched the bi-temporal surface.
        let entry = MemoryEntry {
            id: "id-1".into(),
            key: "k".into(),
            content: "v".into(),
            category: MemoryCategory::Core,
            timestamp: "2026-02-16T00:00:00Z".into(),
            session_id: None,
            score: None,
            valid_from: None,
            valid_to: None,
            supersedes_id: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("valid_from"));
        assert!(!json.contains("valid_to"));
        assert!(!json.contains("supersedes_id"));
    }

    // ── Default trait method behaviour ──────────────────────────────

    struct NoopBackend {
        store_calls: std::sync::atomic::AtomicUsize,
        recall_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl Memory for NoopBackend {
        fn name(&self) -> &str {
            "noop"
        }
        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            self.store_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            self.recall_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }
        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }
        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }
        async fn health_check(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn default_supersede_falls_back_to_store() {
        let backend = NoopBackend {
            store_calls: std::sync::atomic::AtomicUsize::new(0),
            recall_calls: std::sync::atomic::AtomicUsize::new(0),
        };
        backend
            .supersede("old-id", "k", "v", MemoryCategory::Core, None)
            .await
            .unwrap();
        assert_eq!(
            backend
                .store_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "default supersede must delegate to store on non-bi-temporal backends"
        );
    }

    #[tokio::test]
    async fn default_recall_as_of_falls_back_to_recall() {
        let backend = NoopBackend {
            store_calls: std::sync::atomic::AtomicUsize::new(0),
            recall_calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let _ = backend
            .recall_as_of("q", 5, None, "2026-01-01T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(
            backend
                .recall_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "default recall_as_of must delegate to recall on non-bi-temporal backends"
        );
    }
}
