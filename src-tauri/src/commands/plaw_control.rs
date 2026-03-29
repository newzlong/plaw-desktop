use std::sync::atomic::Ordering;
use tauri::Emitter;
use crate::{AppState, plaw, cron_watcher};
use crate::plaw::LogLine;
use crate::services::config::ensure_config_defaults;
use crate::services::port::{allocate_port, save_port};
use crate::services::proxy::detect_proxy;

#[tauri::command]
pub async fn get_gateway_port(state: tauri::State<'_, AppState>) -> Result<u16, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.port)
}

#[tauri::command]
pub async fn get_plaw_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.running)
}

#[tauri::command]
pub async fn get_plaw_state(state: tauri::State<'_, AppState>) -> Result<plaw::StatusEvent, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.snapshot(false))
}

#[tauri::command]
pub async fn get_plaw_started_at(state: tauri::State<'_, AppState>) -> Result<Option<u64>, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.started_at)
}

#[tauri::command]
pub async fn start_plaw(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<u16, String> {
    ensure_config_defaults(&state.data_dir);

    // Start embedding server early so model loads while Plaw boots
    {
        let mut emb = state.embedding.lock().await;
        if emb.is_available() && !emb.running {
            match emb.start().await {
                Ok(_) => eprintln!("[plaw] Embedding server ready on port {}", emb.port),
                Err(e) => eprintln!("[plaw] Embedding server auto-start failed: {e}"),
            }
        }
    }

    let port = allocate_port(&state.data_dir);
    let mut mgr = state.manager.lock().await;
    let actual_port = mgr.start(port).await?;

    save_port(&state.data_dir, actual_port);

    if let Some(ref mut child) = mgr.child {
        if let Some(stderr) = child.stderr.take() {
            plaw::spawn_log_reader(state.manager.clone(), stderr, app_handle.clone());
        }
        if let Some(stdout) = child.stdout.take() {
            plaw::spawn_stdout_reader(state.manager.clone(), stdout);
        }
    }

    let ev = mgr.snapshot(false);
    drop(mgr);
    let _ = app_handle.emit("plaw-status", &ev);

    // Wait for gateway to become healthy (up to 15s)
    let mut became_healthy = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if plaw::health_check(actual_port).await {
            became_healthy = true;
            break;
        }
        let mut mgr = state.manager.lock().await;
        if !mgr.check_alive().await {
            let ev = mgr.snapshot(true);
            drop(mgr);
            let _ = app_handle.emit("plaw-status", &ev);
            return Err("Plaw process exited during startup".into());
        }
    }

    {
        let mut mgr = state.manager.lock().await;
        mgr.healthy = became_healthy;
        if became_healthy {
            mgr.state = plaw::ProcessState::Healthy;
        }
        let ev = mgr.snapshot(false);
        drop(mgr);
        let _ = app_handle.emit("plaw-status", &ev);
    }

    if !became_healthy {
        eprintln!("[plaw] Plaw started but health check not passing after 15s");
    }

    state.health_stop.store(false, Ordering::Relaxed);
    plaw::spawn_health_watcher(
        state.manager.clone(),
        app_handle.clone(),
        state.health_stop.clone(),
    );

    if became_healthy {
        state.sse_stop.store(false, Ordering::Relaxed);
        cron_watcher::spawn_sse_watcher(
            app_handle,
            actual_port,
            state.data_dir.clone(),
            state.sse_stop.clone(),
        );
    }

    Ok(actual_port)
}

#[tauri::command]
pub async fn stop_plaw(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    state.health_stop.store(true, Ordering::Relaxed);
    state.sse_stop.store(true, Ordering::Relaxed);

    {
        let mut mgr = state.manager.lock().await;
        mgr.state = plaw::ProcessState::Stopping;
        let ev = mgr.snapshot(false);
        drop(mgr);
        let _ = app_handle.emit("plaw-status", &ev);
    }

    {
        let mut emb = state.embedding.lock().await;
        if emb.running {
            let _ = emb.stop().await;
        }
    }

    let mut mgr = state.manager.lock().await;
    mgr.stop().await?;
    let ev = mgr.snapshot(false);
    drop(mgr);
    let _ = app_handle.emit("plaw-status", &ev);
    Ok(())
}

#[tauri::command]
pub async fn restart_plaw(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<u16, String> {
    {
        let mut mgr = state.manager.lock().await;
        mgr.state = plaw::ProcessState::Restarting;
        let ev = mgr.snapshot(false);
        drop(mgr);
        let _ = app_handle.emit("plaw-status", &ev);
    }

    state.health_stop.store(true, Ordering::Relaxed);
    state.sse_stop.store(true, Ordering::Relaxed);
    {
        let mut mgr = state.manager.lock().await;
        mgr.stop().await.ok();
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    start_plaw(state, app_handle).await
}

#[tauri::command]
pub async fn get_recent_logs(
    state: tauri::State<'_, AppState>,
    count: usize,
    level: Option<String>,
    _keyword: Option<String>,
) -> Result<Vec<LogLine>, String> {
    let mgr = state.manager.lock().await;
    let mut logs = mgr.logs.recent(count);
    if let Some(ref lvl) = level {
        logs.retain(|l| l.level == *lvl);
    }
    Ok(logs)
}

#[tauri::command]
pub async fn get_bearer_token(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.bearer_token.clone())
}

#[tauri::command]
pub async fn check_plaw_health(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let port = {
        let mgr = state.manager.lock().await;
        mgr.port
    };
    Ok(plaw::health_check(port).await)
}

#[tauri::command]
pub async fn cancel_active_chat(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let port = {
        let mgr = state.manager.lock().await;
        if !mgr.running || mgr.port == 0 {
            return Ok(());
        }
        mgr.port
    };

    use tokio_tungstenite::connect_async;
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let url = format!("ws://127.0.0.1:{port}/ws/chat");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        connect_async(&url),
    ).await;

    match result {
        Ok(Ok((mut ws_stream, _))) => {
            let cancel_msg = serde_json::json!({"type": "cancel"});
            let _ = ws_stream.send(Message::Text(cancel_msg.to_string())).await;
            let _ = ws_stream.close(None).await;
            Ok(())
        }
        _ => Ok(()),
    }
}

#[tauri::command]
pub async fn test_provider_connection(
    state: tauri::State<'_, AppState>,
    provider: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
    proxy: Option<String>,
) -> Result<String, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15));

    let proxy_url = proxy.filter(|s| !s.is_empty())
        .or_else(|| detect_proxy(&state.data_dir));

    if let Some(ref url) = proxy_url {
        eprintln!("[plaw] using proxy: {url}");
        if let Ok(proxy) = reqwest::Proxy::all(url) {
            builder = builder.proxy(proxy);
        }
    }

    let client = builder.build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let url_base = match provider.as_str() {
        "kimi-coder" => "https://api.kimi.com/coding".to_string(),
        "kimi-moonshot" => "https://api.moonshot.cn".to_string(),
        "anthropic" => "https://api.anthropic.com".to_string(),
        "openai" => "https://api.openai.com".to_string(),
        "openrouter" => "https://openrouter.ai/api".to_string(),
        "ollama" => base_url.clone().unwrap_or_else(|| "http://localhost:11434".to_string()),
        "custom" => base_url.clone().unwrap_or_default(),
        _ => return Err("Unknown provider".to_string()),
    };

    let test_model = model.filter(|m| !m.is_empty()).unwrap_or_else(|| {
        match provider.as_str() {
            "kimi-coder" => "k2p5".to_string(),
            "kimi-moonshot" => "kimi-k2.5".to_string(),
            "anthropic" => "claude-3-5-haiku-20241022".to_string(),
            "openai" => "gpt-4o-mini".to_string(),
            _ => "test".to_string(),
        }
    });

    let is_anthropic = matches!(provider.as_str(), "kimi-coder" | "kimi-moonshot" | "anthropic")
        || (provider == "custom" && !base_url.as_deref().unwrap_or("").contains("openai"));
    let is_ollama = provider == "ollama";

    let res = if is_anthropic && !is_ollama {
        let url = format!("{url_base}/v1/messages");
        let body = serde_json::json!({
            "model": test_model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "hi"}]
        });
        client.post(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
    } else {
        let url = format!("{url_base}/v1/chat/completions");
        let body = serde_json::json!({
            "model": test_model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "hi"}]
        });
        client.post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await
    };

    match res {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if status == 200 || status == 400 {
                Ok("ok".to_string())
            } else if status == 401 || status == 403 {
                Err(format!("auth_failed:{status}"))
            } else {
                Err(format!("http_error:{status}"))
            }
        }
        Err(e) => {
            eprintln!("[plaw] test_provider_connection network error: {e}");
            Err(format!("network_error:{e}"))
        }
    }
}
