use std::path::{Path, PathBuf};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Extra fields from frontend (steps, thinking, etc.) – pass through transparently
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub context_used: u64,
    #[serde(default)]
    pub context_max: u64,
}

#[derive(Clone, serde::Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub message_count: usize,
    pub created_at: u64,
    pub updated_at: u64,
}

fn sessions_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sessions")
}

fn session_path(data_dir: &Path, id: &str) -> PathBuf {
    sessions_dir(data_dir).join(format!("{id}.json"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// List all sessions (summaries only, sorted by updated_at desc)
pub fn list_sessions(data_dir: &Path) -> Vec<SessionSummary> {
    let dir = sessions_dir(data_dir);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut summaries: Vec<SessionSummary> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| {
            let content = std::fs::read_to_string(e.path()).ok()?;
            let session: ChatSession = serde_json::from_str(&content).ok()?;
            Some(SessionSummary {
                id: session.id,
                title: session.title,
                message_count: session.messages.len(),
                created_at: session.created_at,
                updated_at: session.updated_at,
            })
        })
        .collect();

    summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    summaries
}

/// Read a full session by ID
pub fn read_session(data_dir: &Path, id: &str) -> Result<ChatSession, String> {
    let path = session_path(data_dir, id);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read session: {e}"))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse session: {e}"))
}

/// Save (create or update) a session
pub fn save_session(
    data_dir: &Path,
    id: Option<&str>,
    title: &str,
    messages: &[ChatMessage],
    context_used: u64,
    context_max: u64,
) -> Result<ChatSession, String> {
    let dir = sessions_dir(data_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create sessions dir: {e}"))?;

    let now = now_secs();
    let session = if let Some(existing_id) = id {
        // Update existing
        let mut session = read_session(data_dir, existing_id).unwrap_or_else(|_| ChatSession {
            id: existing_id.to_string(),
            title: title.to_string(),
            messages: vec![],
            created_at: now,
            updated_at: now,
            context_used: 0,
            context_max: 0,
        });
        session.title = title.to_string();
        session.messages = messages.to_vec();
        session.updated_at = now;
        session.context_used = context_used;
        session.context_max = context_max;
        session
    } else {
        // Create new
        let new_id = format!("{now}-{:04x}", rand_u16());
        ChatSession {
            id: new_id,
            title: title.to_string(),
            messages: messages.to_vec(),
            created_at: now,
            updated_at: now,
            context_used,
            context_max,
        }
    };

    let path = session_path(data_dir, &session.id);
    let json = serde_json::to_string_pretty(&session)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write session: {e}"))?;

    Ok(session)
}

/// Delete a session
pub fn delete_session(data_dir: &Path, id: &str) -> Result<(), String> {
    let path = session_path(data_dir, id);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete session: {e}"))?;
    }
    Ok(())
}

/// Append a message to an existing session file (for cron results delivery)
pub fn append_session_message(
    data_dir: &Path,
    session_id: &str,
    message: ChatMessage,
) -> Result<(), String> {
    let mut session = read_session(data_dir, session_id)?;
    session.messages.push(message);
    session.updated_at = now_secs();

    let path = session_path(data_dir, session_id);
    let json = serde_json::to_string_pretty(&session)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write session: {e}"))?;
    Ok(())
}

/// Check if a session exists
pub fn session_exists(data_dir: &Path, id: &str) -> bool {
    session_path(data_dir, id).exists()
}

/// Simple pseudo-random u16 (no external dependency)
fn rand_u16() -> u16 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (t.subsec_nanos() & 0xFFFF) as u16
}
