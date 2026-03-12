use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingNotification {
    /// Unique notification ID
    pub id: String,
    /// Target session ID (None = global/unbound)
    pub session_id: Option<String>,
    /// Notification source (e.g. "cron")
    pub source: String,
    /// Source job ID (for cron results)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Source job name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_name: Option<String>,
    /// Message content
    pub content: String,
    /// Unix timestamp (seconds)
    pub timestamp: u64,
    /// Whether this has been consumed by the frontend
    #[serde(default)]
    pub consumed: bool,
}

fn pending_path(data_dir: &Path) -> PathBuf {
    data_dir.join("notifications").join("pending.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn rand_id() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("notif-{}-{:04x}", t.as_secs(), t.subsec_nanos() & 0xFFFF)
}

/// Read all pending notifications from disk
pub fn read_pending(data_dir: &Path) -> Vec<PendingNotification> {
    let path = pending_path(data_dir);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Write pending notifications to disk (atomic)
fn write_pending(data_dir: &Path, items: &[PendingNotification]) -> Result<(), String> {
    let path = pending_path(data_dir);
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| format!("Failed to create notifications dir: {e}"))?;

    let json = serde_json::to_string_pretty(items)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)
        .map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("Failed to rename: {e}"))?;
    Ok(())
}

/// Add a notification to the pending queue
pub fn add_notification(
    data_dir: &Path,
    session_id: Option<String>,
    source: &str,
    job_id: Option<String>,
    job_name: Option<String>,
    content: &str,
) -> Result<PendingNotification, String> {
    let mut items = read_pending(data_dir);
    let notif = PendingNotification {
        id: rand_id(),
        session_id,
        source: source.to_string(),
        job_id,
        job_name,
        content: content.to_string(),
        timestamp: now_secs(),
        consumed: false,
    };
    items.push(notif.clone());

    // Cap at 200 to avoid unbounded growth
    if items.len() > 200 {
        items.drain(0..items.len() - 200);
    }

    write_pending(data_dir, &items)?;
    Ok(notif)
}

/// Get unconsumed notifications for a specific session (or global ones)
pub fn get_session_notifications(
    data_dir: &Path,
    session_id: &str,
) -> Vec<PendingNotification> {
    read_pending(data_dir)
        .into_iter()
        .filter(|n| {
            !n.consumed
                && (n.session_id.as_deref() == Some(session_id) || n.session_id.is_none())
        })
        .collect()
}

/// Mark specific notifications as consumed
pub fn consume_notifications(
    data_dir: &Path,
    ids: &[String],
) -> Result<(), String> {
    let mut items = read_pending(data_dir);
    for item in &mut items {
        if ids.contains(&item.id) {
            item.consumed = true;
        }
    }
    // Remove consumed items older than 24h to keep the file tidy
    let cutoff = now_secs().saturating_sub(86400);
    items.retain(|n| !n.consumed || n.timestamp > cutoff);
    write_pending(data_dir, &items)
}

/// Get all unconsumed notifications (for tray bubble / global check)
pub fn get_all_unconsumed(data_dir: &Path) -> Vec<PendingNotification> {
    read_pending(data_dir)
        .into_iter()
        .filter(|n| !n.consumed)
        .collect()
}
