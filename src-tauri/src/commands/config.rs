use crate::AppState;

#[tauri::command]
pub fn config_exists(state: tauri::State<AppState>) -> bool {
    state.data_dir.join(".plaw").join("config.toml").exists()
}

#[tauri::command]
pub fn read_config(state: tauri::State<AppState>) -> Result<serde_json::Value, String> {
    let config_path = state.data_dir.join(".plaw").join("config.toml");
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config: {e}"))?;
    let value: toml::Value = content.parse()
        .map_err(|e| format!("Failed to parse TOML: {e}"))?;
    serde_json::to_value(value).map_err(|e| format!("Failed to convert: {e}"))
}

#[tauri::command]
pub fn get_data_dir_path(state: tauri::State<AppState>) -> String {
    state.data_dir.display().to_string()
}

#[tauri::command]
pub fn write_config(state: tauri::State<AppState>, config: serde_json::Value) -> Result<(), String> {
    let config_path = state.data_dir.join(".plaw").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap())
        .map_err(|e| format!("Failed to create config dir: {e}"))?;

    let toml_value: toml::Value = serde_json::from_value(config)
        .map_err(|e| format!("Invalid config: {e}"))?;
    let toml_str = toml::to_string_pretty(&toml_value)
        .map_err(|e| format!("Failed to serialize TOML: {e}"))?;

    let tmp_path = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &toml_str)
        .map_err(|e| format!("Failed to write temp file: {e}"))?;
    std::fs::rename(&tmp_path, &config_path)
        .map_err(|e| format!("Failed to rename: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn get_market_proxy(state: tauri::State<AppState>) -> String {
    let path = state.data_dir.join("settings.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|v| v.get("market_proxy").and_then(|p| p.as_str()).map(|s| s.to_string()))
        .unwrap_or_default()
}

#[tauri::command]
pub fn set_market_proxy(state: tauri::State<AppState>, proxy: String) -> Result<(), String> {
    let path = state.data_dir.join("settings.json");
    let mut obj = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&c).ok())
        .unwrap_or_default();

    if proxy.is_empty() {
        obj.remove("market_proxy");
    } else {
        obj.insert("market_proxy".to_string(), serde_json::Value::String(proxy));
    }

    std::fs::write(&path, serde_json::to_string_pretty(&obj).unwrap_or_default())
        .map_err(|e| format!("Failed to save settings: {e}"))
}
