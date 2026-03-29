use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;
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
mod capsules;
mod services;
mod commands;

use plaw::{PlawManager, SharedManager};
use embedding::{EmbeddingManager, SharedEmbedding};
use services::data_dir::get_data_dir;
use services::bundle::{extract_bundle_if_needed, kill_browser_orphans};

pub struct AppState {
    pub data_dir: PathBuf,
    pub manager: SharedManager,
    pub embedding: SharedEmbedding,
    pub health_stop: Arc<AtomicBool>,
    pub sse_stop: Arc<AtomicBool>,
}

pub fn run() {
    let data_dir = get_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);

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
            let show = MenuItemBuilder::with_id("show", "Show Plaw").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show)
                .separator()
                .item(&quit)
                .build()?;

            let tray_icon = app
                .default_window_icon()
                .cloned()
                .unwrap_or_else(|| tauri::image::Image::new(&[], 0, 0));
            let _tray = TrayIconBuilder::new()
                .icon(tray_icon)
                .tooltip("Plaw Desktop")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => {
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
            // Plaw process control
            commands::get_gateway_port,
            commands::get_plaw_status,
            commands::get_plaw_state,
            commands::get_plaw_started_at,
            commands::start_plaw,
            commands::stop_plaw,
            commands::restart_plaw,
            commands::get_recent_logs,
            commands::get_bearer_token,
            commands::check_plaw_health,
            commands::test_provider_connection,
            commands::cancel_active_chat,
            // Config
            commands::config_exists,
            commands::read_config,
            commands::write_config,
            commands::get_data_dir_path,
            commands::get_market_proxy,
            commands::set_market_proxy,
            // Skills
            commands::list_local_skills,
            commands::install_skill,
            commands::uninstall_skill,
            commands::audit_skill,
            commands::audit_all_unaudited,
            commands::search_registry_skills,
            commands::sync_skills_registry,
            // Gateway proxy
            commands::gateway_fetch,
            commands::gateway_post,
            commands::gateway_patch,
            commands::gateway_delete,
            // Knowledge
            commands::list_knowledge,
            commands::search_knowledge,
            commands::read_knowledge_entry,
            commands::delete_knowledge_entry,
            commands::save_knowledge_entry,
            commands::get_knowledge_stats,
            // Sessions
            commands::list_sessions,
            commands::read_session,
            commands::save_session,
            commands::delete_session,
            commands::append_session_message,
            commands::session_exists,
            // Capsules
            commands::list_capsules,
            commands::delete_capsule,
            commands::get_capsule_stats,
            // Uploads
            commands::save_upload,
            commands::get_uploads_info,
            commands::clear_uploads,
            // Notifications
            commands::add_notification,
            commands::get_session_notifications,
            commands::consume_notifications,
            commands::get_all_unconsumed_notifications,
            // Embedding
            commands::start_embedding,
            commands::stop_embedding,
            commands::get_embedding_status,
            commands::is_embedding_available,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
