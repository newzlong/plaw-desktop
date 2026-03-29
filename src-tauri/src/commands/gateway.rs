use crate::AppState;

#[tauri::command]
pub async fn gateway_fetch(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 { return Err("Plaw not running".to_string()); }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy().build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    let mut req = client.get(&url);
    if let Some(ref t) = token { req = req.header("Authorization", format!("Bearer {t}")); }
    let resp = req.send().await.map_err(|e| format!("Gateway request failed: {e}"))?;
    if !resp.status().is_success() { return Err(format!("HTTP {}", resp.status())); }
    resp.json::<serde_json::Value>().await.map_err(|e| format!("JSON parse error: {e}"))
}

#[tauri::command]
pub async fn gateway_post(
    state: tauri::State<'_, AppState>,
    path: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 { return Err("Plaw not running".to_string()); }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy().build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    let mut req = client.post(&url).json(&body);
    if let Some(ref t) = token { req = req.header("Authorization", format!("Bearer {t}")); }
    let resp = req.send().await.map_err(|e| format!("Gateway request failed: {e}"))?;
    if !resp.status().is_success() { return Err(format!("HTTP {}", resp.status())); }
    resp.json::<serde_json::Value>().await.map_err(|e| format!("JSON parse error: {e}"))
}

#[tauri::command]
pub async fn gateway_patch(
    state: tauri::State<'_, AppState>,
    path: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 { return Err("Plaw not running".to_string()); }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy().build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    let mut req = client.patch(&url).json(&body);
    if let Some(ref t) = token { req = req.header("Authorization", format!("Bearer {t}")); }
    let resp = req.send().await.map_err(|e| format!("Gateway request failed: {e}"))?;
    if !resp.status().is_success() { return Err(format!("HTTP {}", resp.status())); }
    resp.json::<serde_json::Value>().await.map_err(|e| format!("JSON parse error: {e}"))
}

#[tauri::command]
pub async fn gateway_delete(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 { return Err("Plaw not running".to_string()); }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy().build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    let mut req = client.delete(&url);
    if let Some(ref t) = token { req = req.header("Authorization", format!("Bearer {t}")); }
    let resp = req.send().await.map_err(|e| format!("Gateway request failed: {e}"))?;
    if !resp.status().is_success() { return Err(format!("HTTP {}", resp.status())); }
    resp.json::<serde_json::Value>().await.map_err(|e| format!("JSON parse error: {e}"))
}
