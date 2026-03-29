use crate::AppState;
use crate::sessions;

#[tauri::command]
pub fn list_sessions(state: tauri::State<AppState>) -> Vec<sessions::SessionSummary> {
    sessions::list_sessions(&state.data_dir)
}

#[tauri::command]
pub fn read_session(state: tauri::State<AppState>, id: String) -> Result<sessions::ChatSession, String> {
    sessions::read_session(&state.data_dir, &id)
}

#[tauri::command]
pub fn save_session(
    state: tauri::State<AppState>,
    id: Option<String>,
    title: String,
    messages: Vec<sessions::ChatMessage>,
    context_used: Option<u64>,
    context_max: Option<u64>,
) -> Result<sessions::ChatSession, String> {
    sessions::save_session(&state.data_dir, id.as_deref(), &title, &messages, context_used.unwrap_or(0), context_max.unwrap_or(0))
}

#[tauri::command]
pub fn delete_session(state: tauri::State<AppState>, id: String) -> Result<(), String> {
    sessions::delete_session(&state.data_dir, &id)
}

#[tauri::command]
pub fn append_session_message(
    state: tauri::State<AppState>,
    session_id: String,
    role: String,
    content: String,
) -> Result<(), String> {
    sessions::append_session_message(
        &state.data_dir,
        &session_id,
        sessions::ChatMessage { role, content, extra: Default::default() },
    )
}

#[tauri::command]
pub fn session_exists(state: tauri::State<AppState>, id: String) -> bool {
    sessions::session_exists(&state.data_dir, &id)
}
