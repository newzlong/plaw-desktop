use crate::AppState;

#[tauri::command]
pub fn save_upload(state: tauri::State<AppState>, name: String, data: Vec<u8>) -> Result<String, String> {
    let uploads_dir = state.data_dir.join("uploads");
    std::fs::create_dir_all(&uploads_dir).map_err(|e| format!("Failed to create uploads dir: {e}"))?;
    let safe_name: String = name.chars().map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    let final_name = format!("{now}_{safe_name}");
    let path = uploads_dir.join(&final_name);
    std::fs::write(&path, &data).map_err(|e| format!("Failed to write upload: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_uploads_info(state: tauri::State<AppState>) -> Result<(u64, usize), String> {
    let uploads_dir = state.data_dir.join("uploads");
    if !uploads_dir.exists() { return Ok((0, 0)); }
    let mut total: u64 = 0;
    let mut count: usize = 0;
    let entries = std::fs::read_dir(&uploads_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() { total += meta.len(); count += 1; }
        }
    }
    Ok((total, count))
}

#[tauri::command]
pub fn clear_uploads(state: tauri::State<AppState>) -> Result<(u64, usize), String> {
    let uploads_dir = state.data_dir.join("uploads");
    if !uploads_dir.exists() { return Ok((0, 0)); }
    let mut freed: u64 = 0;
    let mut removed: usize = 0;
    let entries = std::fs::read_dir(&uploads_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                let size = meta.len();
                if std::fs::remove_file(entry.path()).is_ok() { freed += size; removed += 1; }
            }
        }
    }
    Ok((freed, removed))
}
