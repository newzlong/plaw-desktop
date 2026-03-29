use crate::AppState;
use crate::knowledge;

#[tauri::command]
pub fn list_knowledge(state: tauri::State<AppState>) -> Vec<knowledge::KnowledgeEntry> {
    knowledge::list_entries(&state.data_dir)
}

#[tauri::command]
pub fn search_knowledge(state: tauri::State<AppState>, query: String) -> Vec<knowledge::KnowledgeEntry> {
    knowledge::search_entries(&state.data_dir, &query)
}

#[tauri::command]
pub fn read_knowledge_entry(
    state: tauri::State<AppState>,
    id: String,
) -> Result<(knowledge::KnowledgeEntry, String), String> {
    knowledge::read_entry(&state.data_dir, &id)
}

#[tauri::command]
pub fn delete_knowledge_entry(state: tauri::State<AppState>, id: String) -> Result<(), String> {
    knowledge::delete_entry(&state.data_dir, &id)
}

#[tauri::command]
pub fn save_knowledge_entry(
    state: tauri::State<AppState>,
    title: String,
    tags: Vec<String>,
    content: String,
    id: Option<String>,
) -> Result<knowledge::KnowledgeEntry, String> {
    knowledge::save_entry(&state.data_dir, &title, &tags, &content, id.as_deref())
}

#[tauri::command]
pub fn get_knowledge_stats(state: tauri::State<AppState>) -> knowledge::KnowledgeStats {
    knowledge::get_stats(&state.data_dir)
}
