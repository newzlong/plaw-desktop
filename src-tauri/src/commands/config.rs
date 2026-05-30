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

    // Defense in depth: toml::Value has no representation for JSON `null`,
    // so the from_value below would otherwise fail with a cryptic
    // "invalid type: null". Strip nulls first.
    let incoming_json = strip_nulls(config);
    let incoming: toml::Value = serde_json::from_value(incoming_json)
        .map_err(|e| format!("Invalid config: {e}"))?;

    // Deep-merge the incoming partial into whatever is already on disk.
    // Callers (e.g. SecurityConfig.vue) should send ONLY the sections they
    // edit — round-tripping the whole config through JS Number is lossy for
    // large integers (e.g. plaw's [agent].max_tool_iterations may default
    // to a value at/near i64::MAX; JS f64 can't represent it precisely, and
    // the re-serialized number overflows toml's i64 on the way back).
    // Tables merge recursively; arrays + scalars in the incoming partial
    // replace the existing entry outright, so things like
    // `autonomy.auto_approve` updates correctly (incl. deletions).
    let merged = if config_path.exists() {
        let existing_content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read existing config: {e}"))?;
        let mut existing: toml::Value = existing_content
            .parse()
            .map_err(|e| format!("Failed to parse existing config: {e}"))?;
        deep_merge_toml(&mut existing, incoming);
        existing
    } else {
        incoming
    };

    let toml_str = toml::to_string_pretty(&merged)
        .map_err(|e| format!("Failed to serialize TOML: {e}"))?;

    let tmp_path = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &toml_str)
        .map_err(|e| format!("Failed to write temp file: {e}"))?;
    std::fs::rename(&tmp_path, &config_path)
        .map_err(|e| format!("Failed to rename: {e}"))?;
    Ok(())
}

/// Recursively merge `source` into `target`. Tables merge key-by-key
/// (recursing into nested tables); arrays and scalars in `source` replace
/// the entry in `target` outright. Used by `write_config` to apply a
/// partial config patch without forcing callers to round-trip the entire
/// file.
fn deep_merge_toml(target: &mut toml::Value, source: toml::Value) {
    match (target, source) {
        (toml::Value::Table(target_table), toml::Value::Table(source_table)) => {
            for (key, value) in source_table {
                if let Some(existing) = target_table.get_mut(&key) {
                    deep_merge_toml(existing, value);
                } else {
                    target_table.insert(key, value);
                }
            }
        }
        (target_slot, source) => {
            *target_slot = source;
        }
    }
}

/// Recursively drop `null` values from a `serde_json::Value` so the result
/// can be deserialized into `toml::Value` (which has no null type). Empty
/// arrays / objects are preserved — only literal nulls are removed.
fn strip_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let cleaned: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .filter_map(|(k, v)| {
                    if v.is_null() {
                        None
                    } else {
                        Some((k, strip_nulls(v)))
                    }
                })
                .collect();
            serde_json::Value::Object(cleaned)
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.into_iter().filter(|v| !v.is_null()).map(strip_nulls).collect(),
        ),
        other => other,
    }
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
