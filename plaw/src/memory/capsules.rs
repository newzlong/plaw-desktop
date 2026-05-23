//! Memory Capsule storage — archives pre-compact conversation context.
//!
//! Each capsule preserves the full original messages that would otherwise
//! be discarded during context compaction, along with metadata (keywords,
//! summary, token/message counts) for efficient retrieval.

use anyhow::{Context, Result};
use chrono::Local;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

use super::vector;

/// A single memory capsule — an archived conversation segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capsule {
    pub id: String,
    pub session_id: String,
    pub created_at: String,
    pub keywords: Vec<String>,
    pub summary: String,
    pub content: String,
    pub token_count: u64,
    pub message_count: u64,
}

/// Lightweight capsule metadata for list/search results (no full content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsuleMeta {
    pub id: String,
    pub session_id: String,
    pub created_at: String,
    pub keywords: Vec<String>,
    pub summary: String,
    pub token_count: u64,
    pub message_count: u64,
}

/// Persistent capsule warehouse backed by SQLite.
pub struct CapsuleStore {
    conn: Arc<Mutex<Connection>>,
}

/// Versioned schema migrations for the `capsules` table. Each entry runs
/// in its own transaction via [`crate::db::migrate`]. Add new migrations
/// to the END of the slice with strictly increasing `version`; never
/// reorder or rewrite existing entries (users already at version N would
/// skip the changes).
///
/// **v1 baseline** uses `IF NOT EXISTS` everywhere so the migration is
/// idempotent on pre-framework users who already have these objects.
const CAPSULES_MIGRATIONS: &[crate::db::Migration] = &[crate::db::Migration {
    version: 1,
    description: "baseline capsules table + FTS5 index + sync triggers",
    sql: "CREATE TABLE IF NOT EXISTS capsules (
              id            TEXT PRIMARY KEY,
              session_id    TEXT NOT NULL,
              created_at    TEXT NOT NULL,
              keywords      TEXT NOT NULL DEFAULT '[]',
              summary       TEXT NOT NULL,
              content       TEXT NOT NULL,
              token_count   INTEGER NOT NULL DEFAULT 0,
              message_count INTEGER NOT NULL DEFAULT 0
          );
          CREATE INDEX IF NOT EXISTS idx_capsules_session  ON capsules(session_id);
          CREATE INDEX IF NOT EXISTS idx_capsules_created  ON capsules(created_at);

          CREATE VIRTUAL TABLE IF NOT EXISTS capsules_fts USING fts5(
              keywords, summary, content=capsules, content_rowid=rowid
          );

          CREATE TRIGGER IF NOT EXISTS capsules_ai AFTER INSERT ON capsules BEGIN
              INSERT INTO capsules_fts(rowid, keywords, summary)
              VALUES (new.rowid, new.keywords, new.summary);
          END;
          CREATE TRIGGER IF NOT EXISTS capsules_ad AFTER DELETE ON capsules BEGIN
              INSERT INTO capsules_fts(capsules_fts, rowid, keywords, summary)
              VALUES ('delete', old.rowid, old.keywords, old.summary);
          END;",
}];

impl CapsuleStore {
    /// Open (or create) the capsule store inside the workspace memory directory.
    /// Reuses the same `brain.db` file as the memory system.
    pub fn new(workspace_dir: &Path) -> Result<Self> {
        let db_path = workspace_dir.join("memory").join("brain.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn =
            Connection::open(&db_path).context("CapsuleStore: failed to open brain.db")?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous  = NORMAL;",
        )?;

        // Versioned schema migrations via the crate-wide `db::migrate`
        // framework. Reference impl for the other 10 rusqlite stores in
        // plaw — see PR #10 for the framework + [[reference-gh-pr-create-flow]]
        // for the cascade pattern.
        crate::db::migrate(&conn, "capsules", CAPSULES_MIGRATIONS)
            .context("CapsuleStore: schema migration failed")?;

        // Legacy ad-hoc column add. Kept OUTSIDE the versioned migration
        // slice because SQLite has no `ALTER TABLE ADD COLUMN IF NOT EXISTS`,
        // and existing pre-framework users already have this column from a
        // separate runtime check. Folding it into a v2 Migration would fail
        // on those users' next launch. Future schema changes (new columns,
        // new tables) should go straight into CAPSULES_MIGRATIONS as v2+.
        Self::ensure_embedding_column(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Idempotent: adds the `embedding` column if it isn't there. Pre-dates
    /// the [`crate::db::migrate`] framework — see comment in [`Self::new`].
    fn ensure_embedding_column(conn: &Connection) -> Result<()> {
        let has_embedding: bool = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('capsules') WHERE name='embedding'")?
            .query_row([], |row| row.get::<_, i64>(0))
            .unwrap_or(0)
            > 0;
        if !has_embedding {
            conn.execute_batch("ALTER TABLE capsules ADD COLUMN embedding BLOB")?;
        }
        Ok(())
    }

    /// Store a new capsule. Returns the generated capsule ID.
    pub fn store(&self, capsule: &Capsule) -> Result<String> {
        let id = if capsule.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            capsule.id.clone()
        };
        let keywords_json = serde_json::to_string(&capsule.keywords)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO capsules
             (id, session_id, created_at, keywords, summary, content, token_count, message_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                capsule.session_id,
                capsule.created_at,
                keywords_json,
                capsule.summary,
                capsule.content,
                capsule.token_count,
                capsule.message_count,
            ],
        )?;
        Ok(id)
    }

    /// Search capsules by keyword query (FTS5 full-text search on keywords + summary).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<CapsuleMeta>> {
        let conn = self.conn.lock();
        // Use FTS5 match with BM25 ranking
        let mut stmt = conn.prepare(
            "SELECT c.id, c.session_id, c.created_at, c.keywords, c.summary,
                    c.token_count, c.message_count
             FROM capsules_fts f
             JOIN capsules c ON c.rowid = f.rowid
             WHERE capsules_fts MATCH ?1
             ORDER BY bm25(capsules_fts) LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(CapsuleMeta {
                id: row.get(0)?,
                session_id: row.get(1)?,
                created_at: row.get(2)?,
                keywords: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                summary: row.get(4)?,
                token_count: row.get(5)?,
                message_count: row.get(6)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get a single capsule by ID (including full content).
    pub fn get(&self, id: &str) -> Result<Option<Capsule>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, created_at, keywords, summary, content,
                    token_count, message_count
             FROM capsules WHERE id = ?1",
        )?;
        let result = stmt
            .query_row(params![id], |row| {
                Ok(Capsule {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    created_at: row.get(2)?,
                    keywords: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                    summary: row.get(4)?,
                    content: row.get(5)?,
                    token_count: row.get(6)?,
                    message_count: row.get(7)?,
                })
            })
            .ok();
        Ok(result)
    }

    /// List all capsules (metadata only, no content), newest first.
    pub fn list(&self, limit: usize) -> Result<Vec<CapsuleMeta>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, created_at, keywords, summary,
                    token_count, message_count
             FROM capsules ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(CapsuleMeta {
                id: row.get(0)?,
                session_id: row.get(1)?,
                created_at: row.get(2)?,
                keywords: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                summary: row.get(4)?,
                token_count: row.get(5)?,
                message_count: row.get(6)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Delete a capsule by ID.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let affected = conn.execute("DELETE FROM capsules WHERE id = ?1", params![id])?;
        Ok(affected > 0)
    }

    /// Count total capsules.
    pub fn count(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM capsules", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Store an embedding vector for an existing capsule.
    pub fn store_embedding(&self, id: &str, embedding: &[f32]) -> Result<()> {
        let bytes = vector::vec_to_bytes(embedding);
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE capsules SET embedding = ?1 WHERE id = ?2",
            params![bytes, id],
        )?;
        Ok(())
    }

    /// Semantic vector search: loads all capsule embeddings and ranks by cosine similarity.
    /// Returns `(id, cosine_score)` pairs, highest first.
    pub fn vector_search(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(String, f32)>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT id, embedding FROM capsules WHERE embedding IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((id, blob))
        })?;

        let mut scored: Vec<(String, f32)> = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            if blob.is_empty() {
                continue;
            }
            let emb = vector::bytes_to_vec(&blob);
            let sim = vector::cosine_similarity(query_vec, &emb);
            if sim > 0.0 {
                scored.push((id, sim));
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    /// Total token count across all capsules.
    pub fn total_tokens(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let total: i64 = conn.query_row(
            "SELECT COALESCE(SUM(token_count), 0) FROM capsules",
            [],
            |row| row.get(0),
        )?;
        Ok(total as u64)
    }

    /// Helper: create a capsule from compact context.
    /// If `embedding` is provided, it will be stored alongside the capsule.
    pub fn create_from_compact(
        &self,
        session_id: &str,
        keywords: Vec<String>,
        summary: &str,
        serialized_messages: &str,
        token_count: u64,
        message_count: u64,
        embedding: Option<Vec<f32>>,
    ) -> Result<String> {
        let capsule = Capsule {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            created_at: Local::now().to_rfc3339(),
            keywords,
            summary: summary.to_string(),
            content: serialized_messages.to_string(),
            token_count,
            message_count,
        };
        let id = self.store(&capsule)?;
        if let Some(emb) = embedding {
            self.store_embedding(&id, &emb)?;
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_capsule() -> Capsule {
        Capsule {
            id: String::new(),
            session_id: "sess-1".into(),
            created_at: "2026-05-23T00:00:00Z".into(),
            keywords: vec!["k1".into(), "k2".into()],
            summary: "test capsule summary".into(),
            content: "full conversation body".into(),
            token_count: 42,
            message_count: 3,
        }
    }

    #[test]
    fn new_on_fresh_dir_creates_schema_and_sets_user_version_to_one() {
        let tmp = TempDir::new().unwrap();
        let store = CapsuleStore::new(tmp.path()).expect("open should create schema");

        // Inserting + retrieving validates schema is operational.
        let id = store.store(&sample_capsule()).unwrap();
        let loaded = store.get(&id).unwrap().expect("just-stored capsule must round-trip");
        assert_eq!(loaded.summary, "test capsule summary");

        // user_version should be 1 (set by db::migrate from the baseline migration).
        let conn = store.conn.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1, "baseline migration must bump user_version to 1");
    }

    #[test]
    fn reopen_is_idempotent_no_schema_rebuild() {
        let tmp = TempDir::new().unwrap();
        {
            let store = CapsuleStore::new(tmp.path()).unwrap();
            let _ = store.store(&sample_capsule()).unwrap();
        }
        // Reopen — db::migrate sees user_version >= target, skips re-running.
        // If the migration ran again, CREATE TABLE IF NOT EXISTS would still
        // succeed but ALTER TABLE for embedding (legacy ad-hoc) would fail
        // if it weren't guarded. The fact that reopen works proves both the
        // versioned migration and the ad-hoc embedding patch are idempotent.
        let store = CapsuleStore::new(tmp.path()).expect("reopen must work");
        let conn = store.conn.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1, "user_version unchanged after no-op migrate");

        // And the previously-stored capsule is still there.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM capsules;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
