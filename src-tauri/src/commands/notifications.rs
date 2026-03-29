use crate::AppState;
use crate::notifications;

#[tauri::command]
pub fn add_notification(
    state: tauri::State<AppState>,
    session_id: Option<String>,
    source: String,
    job_id: Option<String>,
    job_name: Option<String>,
    content: String,
) -> Result<notifications::PendingNotification, String> {
    notifications::add_notification(&state.data_dir, session_id, &source, job_id, job_name, &content)
}

#[tauri::command]
pub fn get_session_notifications(
    state: tauri::State<AppState>,
    session_id: String,
) -> Vec<notifications::PendingNotification> {
    notifications::get_session_notifications(&state.data_dir, &session_id)
}

#[tauri::command]
pub fn consume_notifications(
    state: tauri::State<AppState>,
    ids: Vec<String>,
) -> Result<(), String> {
    notifications::consume_notifications(&state.data_dir, &ids)
}

#[tauri::command]
pub fn get_all_unconsumed_notifications(
    state: tauri::State<AppState>,
) -> Vec<notifications::PendingNotification> {
    notifications::get_all_unconsumed(&state.data_dir)
}
