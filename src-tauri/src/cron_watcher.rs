//! SSE-based cron result watcher.
//!
//! Connects to Plaw's `/api/events` SSE endpoint from the Tauri Rust side.
//! This runs as a tokio task independent of the WebView, so it works even when
//! the window is hidden/minimized to tray.
//!
//! On receiving a `cron_result` event, it:
//! 1. Persists to pending.json (notification queue) and session file
//! 2. Emits a Tauri event (`cron-result`) so the frontend can show in-app toast
//! 3. Sends a system notification via tauri-plugin-notification

use futures_util::StreamExt;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

use crate::notifications;
use crate::sessions;

/// Spawn the SSE watcher as a background tokio task.
pub fn spawn_sse_watcher(
    app: AppHandle,
    port: u16,
    data_dir: PathBuf,
    stop: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if let Err(e) = run_sse_loop(&app, port, &data_dir, &stop).await {
                eprintln!("[cron_watcher] SSE connection error: {e}");
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // Reconnect after a short delay
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        eprintln!("[cron_watcher] SSE watcher stopped");
    });
}

async fn run_sse_loop(
    app: &AppHandle,
    port: u16,
    data_dir: &PathBuf,
    stop: &AtomicBool,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("http://127.0.0.1:{port}/api/events");
    eprintln!("[cron_watcher] Connecting to SSE: {url}");
    let client = Client::builder().no_proxy().build()?;
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(format!("SSE endpoint returned {}", response.status()).into());
    }

    eprintln!("[cron_watcher] SSE connected successfully");
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let bytes = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        // SSE messages are delimited by double newlines
        while let Some(pos) = buffer.find("\n\n") {
            let message = buffer[..pos].to_string();
            buffer = buffer[pos + 2..].to_string();
            handle_sse_message(app, data_dir, &message);
        }
    }

    Ok(())
}

fn handle_sse_message(app: &AppHandle, data_dir: &PathBuf, raw: &str) {
    // SSE format: "data: {...json...}"
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(payload) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(payload.trim());
        }
    }
    if data.is_empty() {
        return;
    }

    let value: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => return,
    };

    let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if event_type != "cron_result" {
        return;
    }

    let job_name = value.get("job_name").and_then(|v| v.as_str()).unwrap_or("cron");
    let job_id = value.get("job_id").and_then(|v| v.as_str());
    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("unknown");
    let output = value.get("output").and_then(|v| v.as_str()).unwrap_or("");
    let session_id = value.get("lobster_session").and_then(|v| v.as_str());
    let ok = status == "ok";
    let icon = if ok { "\u{2705}" } else { "\u{274C}" };

    let preview = if output.len() > 200 { &output[..200] } else { output };
    let content = format!("{icon} \u{5B9A}\u{65F6}\u{4EFB}\u{52A1} \"{job_name}\" \u{6267}\u{884C}\u{5B8C}\u{6210}\n{preview}");

    eprintln!("[cron_watcher] cron_result: job={job_name} status={status} session={session_id:?}");

    // 1. Persist to session file (so it shows in chat history)
    if let Some(sid) = session_id {
        if sessions::session_exists(data_dir, sid) {
            if let Err(e) = sessions::append_session_message(
                data_dir,
                sid,
                sessions::ChatMessage {
                    role: "system".to_string(),
                    content: content.clone(),
                },
            ) {
                eprintln!("[cron_watcher] append_session_message error: {e}");
            }
        }
    }

    // 2. Persist to notification queue (pending.json)
    if let Err(e) = notifications::add_notification(
        data_dir,
        session_id.map(|s| s.to_string()),
        "cron",
        job_id.map(|s| s.to_string()),
        Some(job_name.to_string()),
        &content,
    ) {
        eprintln!("[cron_watcher] add_notification error: {e}");
    }

    // 3. Emit Tauri event for frontend (live toast if window visible)
    let _ = app.emit("cron-result", &value);

    // 4. Show the window if it's hidden (most reliable notification method)
    show_window(app);
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
