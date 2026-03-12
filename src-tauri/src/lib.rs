use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tokio::sync::Mutex;

mod plaw;
mod skills;
mod knowledge;
mod embedding;
mod sessions;
mod notifications;
mod cron_watcher;

use plaw::{LogLine, SharedManager, PlawManager};
use embedding::{EmbeddingManager, SharedEmbedding};

pub struct AppState {
    pub data_dir: PathBuf,
    pub manager: SharedManager,
    pub embedding: SharedEmbedding,
    pub health_stop: Arc<AtomicBool>,
    pub sse_stop: Arc<AtomicBool>,
}

/// Get the portable data directory.
/// Priority: exe 旁边的 plaw-data/ (portable mode)
/// Fallback: %LOCALAPPDATA%/lobster-desktop/ (当 exe 目录不可写时，如 Program Files)
fn get_data_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        // Dev mode: use project root / plaw-data
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        return manifest_dir.parent().unwrap().join("plaw-data");
    }

    let exe_path = std::env::current_exe().unwrap();
    let install_dir = exe_path.parent().unwrap();
    let portable_dir = install_dir.join("plaw-data");

    // If the portable dir already exists and is writable, use it
    if portable_dir.exists() && is_dir_writable(&portable_dir) {
        return portable_dir;
    }

    // Try to create the portable dir (will fail in Program Files without admin)
    if !portable_dir.exists() {
        if std::fs::create_dir_all(&portable_dir).is_ok() && is_dir_writable(&portable_dir) {
            return portable_dir;
        }
        // Clean up failed attempt
        let _ = std::fs::remove_dir(&portable_dir);
    }

    // Fallback: %LOCALAPPDATA%/lobster-desktop/
    let fallback = dirs_next::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lobster-desktop");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

/// Test if a directory is writable by creating and removing a temp file
fn is_dir_writable(dir: &std::path::Path) -> bool {
    let test_file = dir.join(".write_test");
    match std::fs::write(&test_file, b"test") {
        Ok(()) => {
            let _ = std::fs::remove_file(&test_file);
            true
        }
        Err(_) => false,
    }
}

/// Try to bind a specific port, returns true if available
fn port_available(port: u16) -> bool {
    std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_ok()
}

/// Load last used port from port-state.json
fn load_saved_port(data_dir: &std::path::Path) -> Option<u16> {
    let path = data_dir.join("port-state.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("port").and_then(|v| v.as_u64()).map(|p| p as u16)
}

/// Save current port to port-state.json
fn save_port(data_dir: &std::path::Path, port: u16) {
    let path = data_dir.join("port-state.json");
    let json = serde_json::json!({ "port": port });
    let _ = std::fs::write(&path, serde_json::to_string(&json).unwrap_or_default());
}

/// Allocate a port: reuse saved port if available, otherwise pick a random one
fn allocate_port(data_dir: &std::path::Path) -> u16 {
    // Try saved port first
    if let Some(saved) = load_saved_port(data_dir) {
        if saved > 0 && port_available(saved) {
            return saved;
        }
    }
    // Fallback: random port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Patch config.toml on startup to ensure required defaults for Lobster.
/// - gateway.require_pairing = false (local-only, no auth)
/// - web_search/web_fetch/http_request enabled if missing
fn ensure_config_defaults(data_dir: &std::path::Path) {
    let config_path = data_dir.join(".plaw").join("config.toml");
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return, // No config yet, will be created by Setup Wizard
    };
    let mut val: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(_) => return,
    };
    let table = match val.as_table_mut() {
        Some(t) => t,
        None => return,
    };

    let mut changed = false;

    // gateway.require_pairing = false
    {
        let gateway = table
            .entry("gateway")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let Some(gw) = gateway.as_table_mut() {
            if gw.get("require_pairing").and_then(|v| v.as_bool()) != Some(false) {
                gw.insert("require_pairing".to_string(), toml::Value::Boolean(false));
                changed = true;
            }
        }
    }

    // Helper: ensure a section exists and its "enabled" field is true
    let ensure_enabled = |table: &mut toml::map::Map<String, toml::Value>,
                          key: &str,
                          defaults: Vec<(&str, toml::Value)>,
                          changed: &mut bool| {
        let section = table
            .entry(key.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let Some(sec) = section.as_table_mut() {
            if sec.get("enabled").and_then(|v| v.as_bool()) != Some(true) {
                sec.insert("enabled".into(), toml::Value::Boolean(true));
                *changed = true;
            }
            for (k, v) in defaults {
                if !sec.contains_key(k) {
                    sec.insert(k.into(), v);
                    *changed = true;
                }
            }
        }
    };

    // [web_search] — ensure enabled + force bing (duckduckgo blocked in China)
    ensure_enabled(table, "web_search", vec![
        ("provider", toml::Value::String("bing".into())),
        ("max_results", toml::Value::Integer(5)),
        ("timeout_secs", toml::Value::Integer(15)),
    ], &mut changed);
    // Force bing over duckduckgo (duckduckgo requires VPN in China)
    if let Some(ws) = table.get_mut("web_search").and_then(|v| v.as_table_mut()) {
        if ws.get("provider").and_then(|v| v.as_str()) == Some("duckduckgo") {
            ws.insert("provider".into(), toml::Value::String("bing".into()));
            changed = true;
        }
    }

    // [web_fetch] — ensure enabled + default to fast_html2md
    ensure_enabled(table, "web_fetch", vec![
        ("provider", toml::Value::String("fast_html2md".into())),
        ("timeout_secs", toml::Value::Integer(30)),
    ], &mut changed);

    // [http_request] — ensure enabled + default to localhost-only
    ensure_enabled(table, "http_request", vec![
        ("allow_local", toml::Value::Boolean(true)),
        ("timeout_secs", toml::Value::Integer(120)),
    ], &mut changed);

    // autonomy — fix level and ensure allowed_commands is populated
    {
        let autonomy = table
            .entry("autonomy")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let Some(sec) = autonomy.as_table_mut() {
            // Upgrade "readonly" to "supervised" so tools can execute
            if sec.get("level").and_then(|v| v.as_str()) == Some("readonly") {
                sec.insert("level".into(), toml::Value::String("supervised".into()));
                changed = true;
            }
            // Ensure allowed_commands is properly populated
            let level = sec.get("level").and_then(|v| v.as_str()).unwrap_or("supervised");
            let cmds = sec.get("allowed_commands").and_then(|v| v.as_array());
            let cmds_empty = cmds.map(|a| a.is_empty()).unwrap_or(true);
            let has_wildcard = cmds
                .map(|a| a.iter().any(|v| v.as_str() == Some("*")))
                .unwrap_or(false);

            if level == "full" && !has_wildcard {
                // Full autonomy: wildcard allows all commands
                sec.insert("allowed_commands".into(),
                    toml::Value::Array(vec![toml::Value::String("*".into())]));
                changed = true;
            } else if level != "full" && cmds_empty {
                // Supervised: sensible whitelist (empty list blocks everything)
                let defaults: Vec<toml::Value> = [
                    "git", "ls", "cat", "grep", "find", "head", "tail", "wc",
                    "echo", "pwd", "date", "cargo", "npm", "pnpm", "node",
                    "python", "python3", "pip", "mkdir", "cp", "mv", "touch",
                    "rm", "curl", "wget", "tar", "unzip", "which", "env",
                    "sort", "uniq", "awk", "sed", "tr", "cut", "xargs",
                    "du", "df", "file", "basename", "dirname", "realpath",
                ].iter().map(|s| toml::Value::String(s.to_string())).collect();
                sec.insert("allowed_commands".into(), toml::Value::Array(defaults));
                changed = true;
            }
            // workspace_only=true can restrict file tools; for desktop app use false
            if sec.get("workspace_only").and_then(|v| v.as_bool()) == Some(true) {
                sec.insert("workspace_only".into(), toml::Value::Boolean(false));
                changed = true;
            }
        }
    }

    if changed {
        if let Ok(s) = toml::to_string_pretty(&val) {
            let _ = std::fs::write(&config_path, s);
        }
    }
}

// ========================
// Tauri Commands
// ========================

#[tauri::command]
async fn get_gateway_port(state: tauri::State<'_, AppState>) -> Result<u16, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.port)
}

#[tauri::command]
async fn get_plaw_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.running)
}

/// Return the full process state for frontend to query on mount/activate
#[tauri::command]
async fn get_plaw_state(state: tauri::State<'_, AppState>) -> Result<plaw::StatusEvent, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.snapshot(false))
}

#[tauri::command]
async fn get_plaw_started_at(state: tauri::State<'_, AppState>) -> Result<Option<u64>, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.started_at)
}

#[tauri::command]
async fn start_plaw(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<u16, String> {
    // Patch config defaults (no pairing, web_search enabled, etc.)
    ensure_config_defaults(&state.data_dir);

    let port = allocate_port(&state.data_dir);
    let mut mgr = state.manager.lock().await;
    let actual_port = mgr.start(port).await?;

    // Persist the port for next launch
    save_port(&state.data_dir, actual_port);

    // Take stderr/stdout handles for log capture
    if let Some(ref mut child) = mgr.child {
        if let Some(stderr) = child.stderr.take() {
            plaw::spawn_log_reader(state.manager.clone(), stderr, app_handle.clone());
        }
        if let Some(stdout) = child.stdout.take() {
            plaw::spawn_stdout_reader(state.manager.clone(), stdout);
        }
    }

    // Emit started (not yet healthy) status
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
        // Check if process died during startup
        let mut mgr = state.manager.lock().await;
        if !mgr.check_alive().await {
            let ev = mgr.snapshot(true);
            drop(mgr);
            let _ = app_handle.emit("plaw-status", &ev);
            return Err("Plaw process exited during startup".into());
        }
    }

    // Update health state
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
        eprintln!("[lobster] Plaw started but health check not passing after 15s");
    }

    // Start health watcher (reset stop flag)
    state.health_stop.store(false, Ordering::Relaxed);
    plaw::spawn_health_watcher(
        state.manager.clone(),
        app_handle.clone(),
        state.health_stop.clone(),
    );

    // Start SSE watcher for cron notifications (works even when window is hidden)
    if became_healthy {
        state.sse_stop.store(false, Ordering::Relaxed);
        cron_watcher::spawn_sse_watcher(
            app_handle,
            actual_port,
            state.data_dir.clone(),
            state.sse_stop.clone(),
        );
    }

    // Auto-start embedding server if available
    {
        let mut emb = state.embedding.lock().await;
        if emb.is_available() && !emb.running {
            if let Err(e) = emb.start().await {
                eprintln!("[lobster] Embedding server auto-start failed: {e}");
            } else {
                eprintln!("[lobster] Embedding server auto-started on port {}", emb.port);
            }
        }
    }

    Ok(actual_port)
}

#[tauri::command]
async fn stop_plaw(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    state.health_stop.store(true, Ordering::Relaxed);
    state.sse_stop.store(true, Ordering::Relaxed);

    // Emit stopping state immediately so frontend reacts instantly
    {
        let mut mgr = state.manager.lock().await;
        mgr.state = plaw::ProcessState::Stopping;
        let ev = mgr.snapshot(false);
        drop(mgr);
        let _ = app_handle.emit("plaw-status", &ev);
    }

    // Stop embedding server alongside Plaw
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
async fn restart_plaw(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<u16, String> {
    // Emit restarting state immediately
    {
        let mut mgr = state.manager.lock().await;
        mgr.state = plaw::ProcessState::Restarting;
        let ev = mgr.snapshot(false);
        drop(mgr);
        let _ = app_handle.emit("plaw-status", &ev);
    }

    // Stop old health watcher + SSE watcher + process
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
fn config_exists(state: tauri::State<AppState>) -> bool {
    state.data_dir.join(".plaw").join("config.toml").exists()
}

#[tauri::command]
fn read_config(state: tauri::State<AppState>) -> Result<serde_json::Value, String> {
    let config_path = state.data_dir.join(".plaw").join("config.toml");
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config: {e}"))?;
    let value: toml::Value = content.parse()
        .map_err(|e| format!("Failed to parse TOML: {e}"))?;
    serde_json::to_value(value).map_err(|e| format!("Failed to convert: {e}"))
}

#[tauri::command]
fn get_data_dir_path(state: tauri::State<AppState>) -> String {
    state.data_dir.display().to_string()
}

#[tauri::command]
fn write_config(state: tauri::State<AppState>, config: serde_json::Value) -> Result<(), String> {
    let config_path = state.data_dir.join(".plaw").join("config.toml");
    eprintln!("[lobster] write_config to: {}", config_path.display());
    eprintln!("[lobster] data_dir: {}", state.data_dir.display());
    std::fs::create_dir_all(config_path.parent().unwrap())
        .map_err(|e| format!("Failed to create config dir: {e}"))?;

    let toml_value: toml::Value = serde_json::from_value(config)
        .map_err(|e| format!("Invalid config: {e}"))?;
    let toml_str = toml::to_string_pretty(&toml_value)
        .map_err(|e| format!("Failed to serialize TOML: {e}"))?;

    // Atomic write
    let tmp_path = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &toml_str)
        .map_err(|e| format!("Failed to write temp file: {e}"))?;
    std::fs::rename(&tmp_path, &config_path)
        .map_err(|e| format!("Failed to rename: {e}"))?;
    Ok(())
}

#[tauri::command]
async fn get_recent_logs(
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
async fn test_provider_connection(
    state: tauri::State<'_, AppState>,
    provider: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
    proxy: Option<String>,
) -> Result<String, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15));

    // 1) User-supplied proxy from the UI form
    let proxy_url = proxy.filter(|s| !s.is_empty());

    // 2) Fallback: detect_proxy (settings.json > env vars > config.toml, skips enc2:)
    let proxy_url = proxy_url.or_else(|| detect_proxy(&state.data_dir));

    if let Some(ref url) = proxy_url {
        eprintln!("[lobster] using proxy: {url}");
        if let Ok(proxy) = reqwest::Proxy::all(url) {
            builder = builder.proxy(proxy);
        }
    }

    let client = builder.build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    // Determine base URL
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

    // Determine test model
    let test_model = model.filter(|m| !m.is_empty()).unwrap_or_else(|| {
        match provider.as_str() {
            "kimi-coder" => "k2p5".to_string(),
            "kimi-moonshot" => "kimi-k2.5".to_string(),
            "anthropic" => "claude-3-5-haiku-20241022".to_string(),
            "openai" => "gpt-4o-mini".to_string(),
            _ => "test".to_string(),
        }
    });

    // Use Anthropic format for kimi/anthropic/custom, OpenAI format for others
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
            eprintln!("[lobster] test_provider_connection network error: {e}");
            if let Some(ref url) = proxy_url {
                eprintln!("[lobster] proxy detected: {url}");
            } else {
                eprintln!("[lobster] no proxy env var detected");
            }
            Err(format!("network_error:{e}"))
        }
    }
}

// ========================
// Skills Commands
// ========================

#[tauri::command]
fn list_local_skills(state: tauri::State<AppState>) -> Vec<skills::SkillEntry> {
    skills::list_local_skills(&state.data_dir)
}

#[tauri::command]
async fn install_skill(
    state: tauri::State<'_, AppState>,
    path_or_url: String,
) -> Result<String, String> {
    let proxy_url = detect_proxy(&state.data_dir);

    if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
        skills::install_skill_from_url(
            &state.data_dir,
            &path_or_url,
            proxy_url.as_deref(),
        ).await
    } else {
        skills::install_skill_from_path(
            &state.data_dir,
            std::path::Path::new(&path_or_url),
        )
    }
}

#[tauri::command]
fn uninstall_skill(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    skills::uninstall_skill(&state.data_dir, &name)
}

#[tauri::command]
async fn audit_skill(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<skills::AuditResult, String> {
    // name can be either a directory slug (e.g. "agent-browser") or display name (e.g. "Agent Browser")
    let skill_md = skills::resolve_skill_md(&state.data_dir, &name)?;
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;

    let proxy_url = detect_proxy(&state.data_dir);
    let result = skills::audit_skill_content(
        &state.data_dir,
        &content,
        proxy_url.as_deref(),
    ).await?;

    // Write tags back using the resolved SKILL.md path
    let new_content = skills::inject_audit_tags(&content, &result.compatibility, &result.risk);
    std::fs::write(&skill_md, new_content)
        .map_err(|e| format!("Failed to write tags to SKILL.md: {e}"))?;

    Ok(result)
}

/// Detect proxy URL with priority: user-configured > env vars > config.toml
fn detect_proxy(data_dir: &std::path::Path) -> Option<String> {
    // 1. User-configured proxy (from UI settings, saved in settings.json)
    let settings_path = data_dir.join("settings.json");
    if let Ok(content) = std::fs::read_to_string(&settings_path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(proxy) = val.get("market_proxy").and_then(|v| v.as_str()) {
                if !proxy.is_empty() {
                    return Some(proxy.to_string());
                }
            }
        }
    }

    // 2. Environment variables
    let from_env = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .ok()
        .filter(|s| !s.is_empty());

    if from_env.is_some() {
        return from_env;
    }

    // 3. config.toml [proxy] section
    let config_path = data_dir.join(".plaw").join("config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let val: toml::Value = content.parse().ok()?;
    let proxy_section = val.get("proxy")?;

    if proxy_section.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }

    proxy_section
        .get("https_proxy")
        .or_else(|| proxy_section.get("http_proxy"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        // Skip encrypted values (enc2:...) — not usable as URLs
        .filter(|s| s.starts_with("http://") || s.starts_with("https://") || s.starts_with("socks"))
        .map(|s| s.to_string())
}

#[tauri::command]
fn get_market_proxy(state: tauri::State<AppState>) -> String {
    let path = state.data_dir.join("settings.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|v| v.get("market_proxy").and_then(|p| p.as_str()).map(|s| s.to_string()))
        .unwrap_or_default()
}

#[tauri::command]
fn set_market_proxy(state: tauri::State<AppState>, proxy: String) -> Result<(), String> {
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

#[derive(serde::Serialize)]
struct RegistrySearchResult {
    skills: Vec<skills::RegistrySkill>,
    /// "online" if fetched from GitHub, "local" if fallback
    source: String,
    /// Error message if GitHub failed (for UI display)
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[tauri::command]
async fn search_registry_skills(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<RegistrySearchResult, String> {
    let proxy_url = detect_proxy(&state.data_dir);

    // Try online GitHub API first
    match skills::fetch_github_skills(proxy_url.as_deref()).await {
        Ok(online_skills) => {
            let query_lower = query.to_lowercase();
            let results = online_skills.into_iter()
                .filter(|s| {
                    query_lower.is_empty()
                        || s.name.to_lowercase().contains(&query_lower)
                        || s.description.to_lowercase().contains(&query_lower)
                })
                .collect();
            Ok(RegistrySearchResult {
                skills: results,
                source: "online".to_string(),
                error: None,
            })
        }
        Err(e) => {
            eprintln!("[lobster] GitHub API failed, falling back to local: {e}");
            let results = skills::search_local_skills(&state.data_dir, &query);
            Ok(RegistrySearchResult {
                skills: results,
                source: "local".to_string(),
                error: Some(e),
            })
        }
    }
}

#[tauri::command]
async fn sync_skills_registry(
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    let open_skills_dir = state.data_dir.join("open-skills");

    if open_skills_dir.join(".git").exists() {
        // Git pull to update
        let output = std::process::Command::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(&open_skills_dir)
            .output()
            .map_err(|e| format!("Failed to run git pull: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git pull failed: {stderr}"));
        }
    } else {
        // Clone fresh
        std::fs::create_dir_all(&open_skills_dir)
            .map_err(|e| format!("Failed to create dir: {e}"))?;

        let output = std::process::Command::new("git")
            .args([
                "clone", "--depth", "1",
                "https://github.com/besoeasy/open-skills.git",
                &open_skills_dir.display().to_string(),
            ])
            .output()
            .map_err(|e| format!("Failed to run git clone: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git clone failed: {stderr}"));
        }
    }

    // Return count of skills found
    let skills_dir = open_skills_dir.join("skills");
    let count = skills::scan_skills_dir(&skills_dir, "open-skills").len() as u32;
    Ok(count)
}

#[tauri::command]
async fn get_bearer_token(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    let mgr = state.manager.lock().await;
    Ok(mgr.bearer_token.clone())
}

#[tauri::command]
async fn check_plaw_health(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let port = {
        let mgr = state.manager.lock().await;
        mgr.port
    };
    Ok(plaw::health_check(port).await)
}

/// Send cancel to Plaw via a temporary WS connection.
/// Called from frontend beforeunload / onDeactivated as a failsafe
/// to ensure the agent loop stops even if the main WS dropped.
#[tauri::command]
async fn cancel_active_chat(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let port = {
        let mgr = state.manager.lock().await;
        if !mgr.running || mgr.port == 0 {
            return Ok(()); // Nothing to cancel
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
        _ => Ok(()), // Timeout or connection failed — Plaw may already be idle
    }
}

/// Proxy GET requests to Plaw gateway (avoids CORS in dev mode)
#[tauri::command]
async fn gateway_fetch(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 {
        return Err("Plaw not running".to_string());
    }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut req = client.get(&url);
    if let Some(ref t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await
        .map_err(|e| format!("Gateway request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.json::<serde_json::Value>().await
        .map_err(|e| format!("JSON parse error: {e}"))
}

/// Proxy POST requests to Plaw gateway
#[tauri::command]
async fn gateway_post(
    state: tauri::State<'_, AppState>,
    path: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 {
        return Err("Plaw not running".to_string());
    }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut req = client.post(&url).json(&body);
    if let Some(ref t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await
        .map_err(|e| format!("Gateway request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.json::<serde_json::Value>().await
        .map_err(|e| format!("JSON parse error: {e}"))
}

/// Proxy PATCH requests to Plaw gateway
#[tauri::command]
async fn gateway_patch(
    state: tauri::State<'_, AppState>,
    path: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 {
        return Err("Plaw not running".to_string());
    }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut req = client.patch(&url).json(&body);
    if let Some(ref t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await
        .map_err(|e| format!("Gateway request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.json::<serde_json::Value>().await
        .map_err(|e| format!("JSON parse error: {e}"))
}

/// Proxy DELETE requests to Plaw gateway
#[tauri::command]
async fn gateway_delete(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let (port, token) = {
        let mgr = state.manager.lock().await;
        (mgr.port, mgr.bearer_token.clone())
    };
    if port == 0 {
        return Err("Plaw not running".to_string());
    }
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut req = client.delete(&url);
    if let Some(ref t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await
        .map_err(|e| format!("Gateway request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.json::<serde_json::Value>().await
        .map_err(|e| format!("JSON parse error: {e}"))
}

// ========================
// Knowledge Commands
// ========================

#[tauri::command]
fn list_knowledge(state: tauri::State<AppState>) -> Vec<knowledge::KnowledgeEntry> {
    knowledge::list_entries(&state.data_dir)
}

#[tauri::command]
fn search_knowledge(state: tauri::State<AppState>, query: String) -> Vec<knowledge::KnowledgeEntry> {
    knowledge::search_entries(&state.data_dir, &query)
}

#[tauri::command]
fn read_knowledge_entry(
    state: tauri::State<AppState>,
    id: String,
) -> Result<(knowledge::KnowledgeEntry, String), String> {
    knowledge::read_entry(&state.data_dir, &id)
}

#[tauri::command]
fn delete_knowledge_entry(state: tauri::State<AppState>, id: String) -> Result<(), String> {
    knowledge::delete_entry(&state.data_dir, &id)
}

#[tauri::command]
fn save_knowledge_entry(
    state: tauri::State<AppState>,
    title: String,
    tags: Vec<String>,
    content: String,
    id: Option<String>,
) -> Result<knowledge::KnowledgeEntry, String> {
    knowledge::save_entry(&state.data_dir, &title, &tags, &content, id.as_deref())
}

#[tauri::command]
fn get_knowledge_stats(state: tauri::State<AppState>) -> knowledge::KnowledgeStats {
    knowledge::get_stats(&state.data_dir)
}

// ========================
// Session Commands
// ========================

#[tauri::command]
fn list_sessions(state: tauri::State<AppState>) -> Vec<sessions::SessionSummary> {
    sessions::list_sessions(&state.data_dir)
}

#[tauri::command]
fn read_session(state: tauri::State<AppState>, id: String) -> Result<sessions::ChatSession, String> {
    sessions::read_session(&state.data_dir, &id)
}

#[tauri::command]
fn save_session(
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
fn delete_session(state: tauri::State<AppState>, id: String) -> Result<(), String> {
    sessions::delete_session(&state.data_dir, &id)
}

#[tauri::command]
fn append_session_message(
    state: tauri::State<AppState>,
    session_id: String,
    role: String,
    content: String,
) -> Result<(), String> {
    sessions::append_session_message(
        &state.data_dir,
        &session_id,
        sessions::ChatMessage { role, content },
    )
}

#[tauri::command]
fn session_exists(state: tauri::State<AppState>, id: String) -> bool {
    sessions::session_exists(&state.data_dir, &id)
}

// ========================
// Notification Commands
// ========================

#[tauri::command]
fn add_notification(
    state: tauri::State<AppState>,
    session_id: Option<String>,
    source: String,
    job_id: Option<String>,
    job_name: Option<String>,
    content: String,
) -> Result<notifications::PendingNotification, String> {
    notifications::add_notification(
        &state.data_dir,
        session_id,
        &source,
        job_id,
        job_name,
        &content,
    )
}

#[tauri::command]
fn get_session_notifications(
    state: tauri::State<AppState>,
    session_id: String,
) -> Vec<notifications::PendingNotification> {
    notifications::get_session_notifications(&state.data_dir, &session_id)
}

#[tauri::command]
fn consume_notifications(
    state: tauri::State<AppState>,
    ids: Vec<String>,
) -> Result<(), String> {
    notifications::consume_notifications(&state.data_dir, &ids)
}

#[tauri::command]
fn get_all_unconsumed_notifications(
    state: tauri::State<AppState>,
) -> Vec<notifications::PendingNotification> {
    notifications::get_all_unconsumed(&state.data_dir)
}

// ========================
// Embedding Commands
// ========================

#[tauri::command]
async fn start_embedding(state: tauri::State<'_, AppState>) -> Result<u16, String> {
    let mut mgr = state.embedding.lock().await;
    mgr.start().await
}

#[tauri::command]
async fn stop_embedding(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.embedding.lock().await;
    mgr.stop().await
}

#[tauri::command]
async fn get_embedding_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.embedding.lock().await;
    Ok(mgr.running)
}

#[tauri::command]
async fn is_embedding_available(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.embedding.lock().await;
    Ok(mgr.is_available())
}

pub fn run() {
    let data_dir = get_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);

    // Extract bundled tar.gz archives on first run (preserves directory structure)
    extract_bundle_if_needed(&data_dir);

    let manager = Arc::new(Mutex::new(PlawManager::new(data_dir.clone())));
    let embedding = Arc::new(Mutex::new(EmbeddingManager::new(data_dir.clone())));
    let health_stop = Arc::new(AtomicBool::new(false));
    let sse_stop = Arc::new(AtomicBool::new(false));

    let state = AppState {
        data_dir,
        manager,
        embedding,
        health_stop,
        sse_stop,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .setup(|app| {
            let show = MenuItemBuilder::with_id("show", "Show Lobster").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show)
                .separator()
                .item(&quit)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("Lobster Desktop")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => {
                            // Cleanup before exit
                            if let Some(state) = app.try_state::<AppState>() {
                                state.health_stop.store(true, Ordering::Relaxed);
                                state.sse_stop.store(true, Ordering::Relaxed);
                                let emb = state.embedding.clone();
                                let mgr: SharedManager = state.manager.clone();
                                tauri::async_runtime::block_on(async move {
                                    let mut emb_guard = emb.lock().await;
                                    emb_guard.force_kill();
                                    drop(emb_guard);
                                    let mut mgr_guard = mgr.lock().await;
                                    let _ = mgr_guard.stop().await;
                                    // Kill orphaned browser daemon + chrome processes
                                    kill_browser_orphans().await;
                                });
                            }
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        ..
                    } = event
                    {
                        if let Some(w) = tray.app_handle().get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_gateway_port,
            get_plaw_status,
            get_plaw_state,
            get_plaw_started_at,
            start_plaw,
            stop_plaw,
            restart_plaw,
            config_exists,
            read_config,
            write_config,
            get_recent_logs,
            get_bearer_token,
            check_plaw_health,
            test_provider_connection,
            list_local_skills,
            install_skill,
            uninstall_skill,
            audit_skill,
            search_registry_skills,
            sync_skills_registry,
            get_market_proxy,
            set_market_proxy,
            get_data_dir_path,
            cancel_active_chat,
            gateway_fetch,
            gateway_post,
            gateway_patch,
            gateway_delete,
            list_knowledge,
            search_knowledge,
            read_knowledge_entry,
            delete_knowledge_entry,
            save_knowledge_entry,
            get_knowledge_stats,
            list_sessions,
            read_session,
            save_session,
            delete_session,
            append_session_message,
            session_exists,
            add_notification,
            get_session_notifications,
            consume_notifications,
            get_all_unconsumed_notifications,
            start_embedding,
            stop_embedding,
            get_embedding_status,
            is_embedding_available,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Hide to tray instead of exiting
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Extract bundled tar.gz archives on first run.
/// Tauri's resource glob flattens directory structure, so we ship
/// agent-browser/node_modules and browsers as tar.gz and extract here.
fn extract_bundle_if_needed(data_dir: &std::path::Path) {
    let bundles: &[(&str, &str)] = &[
        ("agent-browser-bundle.tar.gz", "agent-browser"),
        ("browsers-bundle.tar.gz", "browsers"),
    ];
    for (archive_name, check_dir) in bundles {
        let archive_path = data_dir.join(archive_name);
        let target_dir = data_dir.join(check_dir);
        // Skip if archive doesn't exist (dev mode) or already extracted
        if !archive_path.exists() {
            continue;
        }
        // Check if the target directory has meaningful content
        // (agent-browser/node_modules/agent-browser/dist/ or browsers/chromium_*/chrome-headless-shell-win64/)
        if target_dir.is_dir() && dir_has_subdirs(&target_dir) {
            continue;
        }
        eprintln!("[lobster] Extracting {} ...", archive_name);
        if let Err(e) = extract_tar_gz(&archive_path, data_dir) {
            eprintln!("[lobster] Failed to extract {}: {}", archive_name, e);
        } else {
            eprintln!("[lobster] Extracted {} successfully", archive_name);
            // Remove the archive to save disk space
            let _ = std::fs::remove_file(&archive_path);
        }
    }
}

/// Check if a directory has at least one subdirectory (indicates proper structure)
fn dir_has_subdirs(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Extract a .tar.gz archive into target_dir
fn extract_tar_gz(
    archive_path: &std::path::Path,
    target_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(archive_path)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(target_dir)?;
    Ok(())
}

/// Kill orphaned browser daemon (node.exe daemon.js) and chrome-headless-shell processes.
async fn kill_browser_orphans() {
    use tokio::process::Command;
    use std::process::Stdio;
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "chrome-headless-shell.exe"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status().await;
        let _ = Command::new("wmic")
            .args(["process", "where",
                   "name='node.exe' and commandline like '%daemon.js%'",
                   "delete"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status().await;
    }
}
