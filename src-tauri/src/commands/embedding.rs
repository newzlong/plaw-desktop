use crate::AppState;

#[tauri::command]
pub async fn start_embedding(state: tauri::State<'_, AppState>) -> Result<u16, String> {
    let mut mgr = state.embedding.lock().await;
    mgr.start().await
}

#[tauri::command]
pub async fn stop_embedding(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.embedding.lock().await;
    mgr.stop().await
}

#[tauri::command]
pub async fn get_embedding_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.embedding.lock().await;
    Ok(mgr.running)
}

#[tauri::command]
pub async fn is_embedding_available(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.embedding.lock().await;
    Ok(mgr.is_available())
}
