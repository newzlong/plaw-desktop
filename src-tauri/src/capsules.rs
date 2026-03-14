use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;

/// Capsule metadata for frontend display (no full content).
#[derive(Debug, Clone, Serialize)]
pub struct CapsuleMeta {
    pub id: String,
    pub session_id: String,
    pub created_at: String,
    pub keywords: Vec<String>,
    pub summary: String,
    pub token_count: u64,
    pub message_count: u64,
}

/// Capsule warehouse stats.
#[derive(Debug, Clone, Serialize)]
pub struct CapsuleStats {
    pub total_count: u64,
    pub total_tokens: u64,
}

fn db_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir
        .join(".plaw")
        .join("workspace")
        .join("memory")
        .join("brain.db")
}

fn open_db(data_dir: &Path) -> Result<Connection, String> {
    let path = db_path(data_dir);
    if !path.exists() {
        return Err("No capsule database found".into());
    }
    Connection::open(&path).map_err(|e| format!("Failed to open capsule DB: {e}"))
}

fn row_to_meta(row: &rusqlite::Row) -> rusqlite::Result<CapsuleMeta> {
    Ok(CapsuleMeta {
        id: row.get(0)?,
        session_id: row.get(1)?,
        created_at: row.get(2)?,
        keywords: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
        summary: row.get(4)?,
        token_count: row.get(5)?,
        message_count: row.get(6)?,
    })
}

/// List all capsules (metadata only), newest first.
pub fn list_capsules(data_dir: &Path, limit: usize) -> Result<Vec<CapsuleMeta>, String> {
    let conn = open_db(data_dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, created_at, keywords, summary,
                    token_count, message_count
             FROM capsules ORDER BY created_at DESC LIMIT ?1",
        )
        .map_err(|e| format!("Query prepare failed: {e}"))?;
    let rows = stmt
        .query_map(params![limit as i64], |row| row_to_meta(row))
        .map_err(|e| format!("Query failed: {e}"))?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| format!("Row parse error: {e}"))?);
    }
    Ok(results)
}

/// Delete a capsule by ID.
pub fn delete_capsule(data_dir: &Path, id: &str) -> Result<bool, String> {
    let conn = open_db(data_dir)?;
    let affected = conn
        .execute("DELETE FROM capsules WHERE id = ?1", params![id])
        .map_err(|e| format!("Delete failed: {e}"))?;
    Ok(affected > 0)
}

/// Get capsule warehouse stats.
pub fn get_capsule_stats(data_dir: &Path) -> Result<CapsuleStats, String> {
    let conn = open_db(data_dir)?;
    let total_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM capsules", [], |row| row.get(0))
        .unwrap_or(0);
    let total_tokens: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(token_count), 0) FROM capsules",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(CapsuleStats {
        total_count: total_count as u64,
        total_tokens: total_tokens as u64,
    })
}
