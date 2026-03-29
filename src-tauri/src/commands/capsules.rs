use crate::AppState;
use crate::capsules;

#[tauri::command]
pub fn list_capsules(state: tauri::State<AppState>, limit: Option<usize>) -> Result<Vec<capsules::CapsuleMeta>, String> {
    capsules::list_capsules(&state.data_dir, limit.unwrap_or(100))
}

#[tauri::command]
pub fn delete_capsule(state: tauri::State<AppState>, id: String) -> Result<bool, String> {
    capsules::delete_capsule(&state.data_dir, &id)
}

#[tauri::command]
pub fn get_capsule_stats(state: tauri::State<AppState>) -> Result<capsules::CapsuleStats, String> {
    capsules::get_capsule_stats(&state.data_dir)
}
