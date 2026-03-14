//! WebSocket agent chat handler.
//!
//! Protocol:
//! ```text
//! Client -> Server: {"type":"message","content":"Hello"}
//! Server -> Client: {"type":"chunk","content":"Hi! "}
//! Server -> Client: {"type":"tool_call","name":"shell","args":{...}}
//! Server -> Client: {"type":"tool_result","name":"shell","output":"..."}
//! Server -> Client: {"type":"done","full_response":"..."}
//! ```

use super::AppState;
use crate::agent::loop_::{is_tool_loop_cancelled, run_tool_call_loop, DRAFT_CLEAR_SENTINEL, DRAFT_PROGRESS_SENTINEL};
use crate::agent::loop_::history::{auto_compact_history, summary_has_pending_tasks, trim_history};
use crate::approval::ApprovalManager;
use crate::observability::traits::{Observer, ObserverEvent, ObserverMetric};
use crate::providers::ChatMessage;
use crate::security::prompt_guard::{PromptGuard, GuardAction, GuardResult};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, HeaderMap},
    response::IntoResponse,
};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Observer wrapper that tracks token usage from LlmResponse events.
/// Accumulates totals for billing, and records last-call input tokens
/// for context window usage reporting.
struct UsageTrackingObserver {
    inner: std::sync::Arc<dyn Observer>,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    /// Last single API call's input_tokens — represents current context window usage.
    last_input_tokens: AtomicU64,
}

impl UsageTrackingObserver {
    fn new(inner: std::sync::Arc<dyn Observer>) -> Self {
        Self {
            inner,
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            last_input_tokens: AtomicU64::new(0),
        }
    }

    fn totals(&self) -> (u64, u64) {
        (
            self.input_tokens.load(Ordering::Relaxed),
            self.output_tokens.load(Ordering::Relaxed),
        )
    }

    /// Returns the last API call's input_tokens (current context window usage).
    fn last_input(&self) -> u64 {
        self.last_input_tokens.load(Ordering::Relaxed)
    }
}

impl Observer for UsageTrackingObserver {
    fn record_event(&self, event: &ObserverEvent) {
        if let ObserverEvent::LlmResponse {
            input_tokens,
            output_tokens,
            ..
        } = event
        {
            if let Some(t) = input_tokens {
                self.input_tokens.fetch_add(*t, Ordering::Relaxed);
                self.last_input_tokens.store(*t, Ordering::Relaxed);
            }
            if let Some(t) = output_tokens {
                self.output_tokens.fetch_add(*t, Ordering::Relaxed);
            }
        }
        self.inner.record_event(event);
    }

    fn record_metric(&self, metric: &ObserverMetric) {
        self.inner.record_metric(metric);
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

const EMPTY_WS_RESPONSE_FALLBACK: &str =
    "Tool execution completed, but the model returned no final text response. Please ask me to summarize the result.";

fn sanitize_ws_response(response: &str, tools: &[Box<dyn crate::tools::Tool>]) -> String {
    let sanitized = crate::channels::sanitize_channel_response(response, tools);
    if sanitized.is_empty() && !response.trim().is_empty() {
        "I encountered malformed tool-call output and could not produce a safe reply. Please try again."
            .to_string()
    } else {
        sanitized
    }
}

fn normalize_prompt_tool_results(content: &str) -> Option<String> {
    let mut cleaned_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("<tool_result") || trimmed == "</tool_result>" {
            continue;
        }
        cleaned_lines.push(line.trim_end());
    }

    if cleaned_lines.is_empty() {
        None
    } else {
        Some(cleaned_lines.join("\n"))
    }
}

fn extract_latest_tool_output(history: &[ChatMessage]) -> Option<String> {
    for msg in history.iter().rev() {
        match msg.role.as_str() {
            "tool" => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                    if let Some(content) = value
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                    {
                        return Some(content.to_string());
                    }
                }

                let trimmed = msg.content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            "user" => {
                if let Some(payload) = msg.content.strip_prefix("[Tool results]") {
                    let payload = payload.trim_start_matches('\n');
                    if let Some(cleaned) = normalize_prompt_tool_results(payload) {
                        return Some(cleaned);
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn finalize_ws_response(
    response: &str,
    history: &[ChatMessage],
    tools: &[Box<dyn crate::tools::Tool>],
) -> String {
    let sanitized = sanitize_ws_response(response, tools);
    if !sanitized.trim().is_empty() {
        return sanitized;
    }

    if let Some(tool_output) = extract_latest_tool_output(history) {
        let excerpt = crate::util::truncate_with_ellipsis(tool_output.trim(), 1200);
        return format!(
            "Tool execution completed, but the model returned no final text response.\n\nLatest tool output:\n{excerpt}"
        );
    }

    EMPTY_WS_RESPONSE_FALLBACK.to_string()
}

/// GET /ws/chat — WebSocket upgrade for agent chat
pub async fn handle_ws_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Auth via Authorization header or websocket protocol token.
    if state.pairing.require_pairing() {
        let token = extract_ws_bearer_token(&headers).unwrap_or_default();
        if !state.pairing.is_authenticated(&token) {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "Unauthorized — provide Authorization: Bearer <token> or Sec-WebSocket-Protocol: bearer.<token>",
            )
                .into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();
    // Maintain conversation history for this WebSocket session
    let mut history: Vec<ChatMessage> = Vec::new();

    // Load skills and session config (mutable — embeddings will be computed)
    let (mut all_skills, initial_skill_names, skill_mode, skill_workspace, identity_cfg) = {
        let config_guard = state.config.lock();
        let skills = crate::skills::load_skills_with_config(
            &config_guard.workspace_dir,
            &config_guard,
        );
        let names: std::collections::HashSet<String> =
            skills.iter().map(|s| s.name.clone()).collect();
        let mode = config_guard.skills.prompt_injection_mode;
        let ws_dir = config_guard.workspace_dir.clone();
        let identity = config_guard.identity.clone();
        (skills, names, mode, ws_dir, identity)
    };

    // Track known skills for hot-reload detection
    let mut known_skill_names = initial_skill_names;

    let approval_manager = {
        let config_guard = state.config.lock();
        ApprovalManager::from_config(&config_guard.autonomy)
    };

    // Capsule store for archiving pre-compact conversation context
    let capsule_store: Option<std::sync::Arc<crate::memory::capsules::CapsuleStore>> = {
        let config_guard = state.config.lock();
        match crate::memory::capsules::CapsuleStore::new(&config_guard.workspace_dir) {
            Ok(store) => Some(std::sync::Arc::new(store)),
            Err(e) => {
                eprintln!("[capsule] Failed to initialize capsule store: {e}");
                None
            }
        }
    };

    // Embedding provider for semantic search (capsules + skill matching)
    let embedding_provider: Option<std::sync::Arc<dyn crate::memory::embeddings::EmbeddingProvider>> = {
        let config_guard = state.config.lock();
        let mem_cfg = &config_guard.memory;
        let name = mem_cfg.embedding_provider.trim();
        if !name.is_empty() && name != "none" {
            Some(std::sync::Arc::from(
                crate::memory::embeddings::create_embedding_provider(
                    name,
                    config_guard.api_key.as_deref(),
                    &mem_cfg.embedding_model,
                    mem_cfg.embedding_dimensions,
                ),
            ))
        } else {
            None
        }
    };

    // Pre-compute skill embeddings for per-turn semantic matching
    if let Some(ref emb) = embedding_provider {
        crate::skills::compute_skill_embeddings(&mut all_skills, emb.as_ref()).await;
    }

    // Build initial system prompt with all skills
    let system_prompt = crate::channels::build_system_prompt_with_mode(
        &skill_workspace,
        &state.model,
        &[],
        &all_skills,
        Some(&identity_cfg),
        None,
        false,
        skill_mode,
    );

    // Add system message to history
    history.push(ChatMessage::system(&system_prompt));

    let mut cron_rx = state.event_tx.subscribe();
    // When a user sends a follow-up message while the agent loop is running,
    // we cancel the current loop and stash the new message here so the outer
    // loop picks it up immediately without waiting for another WS read.
    let mut pending_user_msg: Option<String> = None;

    loop {
        let msg: String = if let Some(stashed) = pending_user_msg.take() {
            stashed
        } else {
            tokio::select! {
                ws_msg = ws_rx.next() => {
                    match ws_msg {
                        Some(Ok(Message::Text(text))) => text.to_string(),
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => continue,
                    }
                }
                event = cron_rx.recv() => {
                    // Forward cron_result events to the WebSocket client
                    if let Ok(ev) = event {
                        if ev.get("type").and_then(|t| t.as_str()) == Some("cron_result") {
                            let _ = ws_tx.send(Message::Text(ev.to_string().into())).await;
                        }
                    }
                    continue;
                }
            }
        };

        // Parse incoming message
        let parsed: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => {
                let err = serde_json::json!({"type": "error", "message": "Invalid JSON"});
                let _ = ws_tx.send(Message::Text(err.to_string().into())).await;
                continue;
            }
        };

        let msg_type = parsed["type"].as_str().unwrap_or("");
        if msg_type != "message" {
            continue;
        }

        let content = parsed["content"].as_str().unwrap_or("").to_string();
        if content.is_empty() {
            continue;
        }

        // ── Prompt injection guard (tiered: Block high-score, Warn mid-score) ──
        // High confidence injections (score >= 0.85) are blocked outright.
        // Medium confidence (detected but < 0.85) are logged as warnings and allowed
        // through — the hardened SafetySection in the system prompt handles them.
        {
            let guard = PromptGuard::with_config(GuardAction::Block, 0.85);
            match guard.scan(&content) {
                GuardResult::Blocked(reason) => {
                    tracing::warn!(reason = %reason, "Prompt injection BLOCKED (high confidence)");
                    let err = serde_json::json!({
                        "type": "error",
                        "message": format!("消息被拦截：检测到高风险提示词注入。{reason}")
                    });
                    let _ = ws_tx.send(Message::Text(err.to_string().into())).await;
                    continue;
                }
                GuardResult::Suspicious(patterns, score) => {
                    tracing::warn!(
                        patterns = ?patterns,
                        score = score,
                        "Prompt injection WARNING — suspicious patterns detected (below block threshold)"
                    );
                }
                GuardResult::Safe => {}
            }
        }

        // Extract Plaw Desktop session ID for cron job binding.
        // When creating cron jobs, the AI should pass this as plaw_session so results
        // are delivered back to the originating chat session.
        let plaw_session_id = parsed["session_id"].as_str().map(str::to_string);
        if let Some(ref sid) = plaw_session_id {
            // Update system prompt with session context (only if not already present)
            if let Some(sys_msg) = history.first_mut() {
                if sys_msg.role == "system" && !sys_msg.content.contains("plaw_session") {
                    sys_msg.content.push_str(&format!(
                        "\n\n## Plaw Desktop Context\nCurrent chat session ID: `{sid}`. \
                         When creating cron jobs (cron_add), pass `plaw_session: \"{sid}\"` \
                         so results are delivered back to this conversation."
                    ));
                }
            }
        }

        // Inject conversation history from frontend (restores context across reconnections).
        // Only applied when history has just the system prompt (fresh session).
        if history.len() <= 1 {
            if let Some(hist_arr) = parsed["history"].as_array() {
                for entry in hist_arr {
                    let role = entry["role"].as_str().unwrap_or("");
                    let msg_content = entry["content"].as_str().unwrap_or("");
                    if msg_content.is_empty() {
                        continue;
                    }
                    match role {
                        "user" => history.push(ChatMessage::user(msg_content)),
                        "assistant" => history.push(ChatMessage::assistant(msg_content)),
                        _ => {}
                    }
                }
            }
        }

        // ── Per-turn semantic skill routing ─────────────────────────
        // Embed the user message and inject only semantically relevant skills.
        // Threshold-driven: 0 skills if nothing matches, N skills if N match.
        if !all_skills.is_empty() {
            if let Some(ref emb) = embedding_provider {
                match emb.embed_one(&content).await {
                    Ok(query_vec) => {
                        let indices = crate::skills::select_relevant_skills(
                            &all_skills, &query_vec, 0.5,
                        );
                        // Only rebuild if we actually filtered (fewer than all skills)
                        if indices.len() < all_skills.len() {
                            let filtered: Vec<crate::skills::Skill> =
                                indices.iter().map(|&i| all_skills[i].clone()).collect();
                            tracing::info!(
                                "[skills] semantic routing: {}/{} skills matched for this turn",
                                filtered.len(),
                                all_skills.len(),
                            );
                            let new_prompt = crate::channels::build_system_prompt_with_mode(
                                &skill_workspace,
                                &state.model,
                                &[],
                                &filtered,
                                Some(&identity_cfg),
                                None,
                                false,
                                skill_mode,
                            );
                            if let Some(sys_msg) = history.first_mut() {
                                if sys_msg.role == "system" {
                                    sys_msg.content = new_prompt;
                                }
                            }
                        } else {
                            tracing::info!(
                                "[skills] semantic routing: all {}/{} skills matched (no filtering), threshold may be too low",
                                indices.len(),
                                all_skills.len(),
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[skills] failed to embed user message, using all skills: {e}");
                    }
                }
            }
        }

        // Add user message to history
        history.push(ChatMessage::user(&content));

        // Get provider info
        let provider_label = state
            .config
            .lock()
            .default_provider
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Broadcast agent_start event
        let _ = state.event_tx.send(serde_json::json!({
            "type": "agent_start",
            "provider": provider_label,
            "model": state.model,
        }));

        // Run the agent loop with delta streaming, relaying progress to WebSocket.
        // UsageTrackingObserver accumulates token counts across all LLM calls.
        let usage_observer = UsageTrackingObserver::new(state.observer.clone());
        let cancel_token = CancellationToken::new();

        // Block scope ensures &mut history borrow is released before match.
        let result = {
            let (delta_tx, mut delta_rx) = mpsc::channel::<String>(64);

            let loop_fut = run_tool_call_loop(
                state.provider.as_ref(),
                &mut history,
                state.tools_registry_exec.as_ref(),
                &usage_observer,
                &provider_label,
                &state.model,
                state.temperature,
                true, // silent - no console output
                Some(&approval_manager),
                "webchat",
                &state.multimodal,
                state.max_tool_iterations,
                Some(cancel_token.clone()),
                Some(delta_tx),    // delta streaming — enables real-time progress
                None,              // hooks
                &[],               // excluded tools
            );
            tokio::pin!(loop_fut);

            loop {
                tokio::select! {
                    delta = delta_rx.recv() => {
                        if let Some(msg) = delta {
                            for event in delta_to_ws_events(&msg) {
                                let _ = ws_tx.send(Message::Text(event.into())).await;
                            }
                        }
                    }
                    // Monitor WebSocket for close/cancel/follow-up during agent execution
                    ws_msg = ws_rx.next() => {
                        match ws_msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&*text) {
                                    match v["type"].as_str() {
                                        Some("cancel") => {
                                            tracing::info!("ws_chat: received cancel message from client");
                                            cancel_token.cancel();
                                        }
                                        Some("message") => {
                                            // User sent a follow-up message — interrupt current
                                            // agent loop and stash the raw JSON so the outer loop
                                            // processes it as a normal message (with history injection, etc.).
                                            tracing::info!("ws_chat: user follow-up message received, interrupting agent loop");
                                            pending_user_msg = Some(text.to_string());
                                            cancel_token.cancel();
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                                // Client disconnected — cancel the agent loop
                                tracing::info!("ws_chat: client disconnected, cancelling agent loop");
                                cancel_token.cancel();
                            }
                            _ => {}
                        }
                    }
                    result = &mut loop_fut => {
                        while let Ok(msg) = delta_rx.try_recv() {
                            for event in delta_to_ws_events(&msg) {
                                let _ = ws_tx.send(Message::Text(event.into())).await;
                            }
                        }
                        break result;
                    }
                }
            }
        };

        match result {
            Ok(response) => {
                let safe_response =
                    finalize_ws_response(&response, &history, state.tools_registry_exec.as_ref());
                // The agent loop already pushes assistant messages to history
                // (loop_.rs:968 for no-tool, loop_.rs:1379 for tool-call).
                // Do NOT push again here to avoid duplicate assistant messages.

                let (_total_input, total_output) = usage_observer.totals();
                let last_input = usage_observer.last_input();
                // Context usage = approximate size of the next API request.
                // history already includes the assistant response (pushed above),
                // so estimate_history_tokens gives the full next-request size.
                // API-based: last_input (this call's prompt) + total_output
                // (this call's completion) ≈ next call's prompt_tokens.
                // Take max because Kimi K2.5 sometimes under-reports input_tokens.
                let estimated = crate::agent::loop_::history::estimate_history_tokens(&history) as u64;
                let api_based = last_input + total_output;
                let context_used = std::cmp::max(api_based, estimated);
                let done = serde_json::json!({
                    "type": "done",
                    "full_response": safe_response,
                    "usage": {
                        "context_used": context_used,
                    },
                    "context_window": state.max_context_tokens,
                });
                let ws_alive = ws_tx.send(Message::Text(done.to_string().into())).await.is_ok();

                let _ = state.event_tx.send(serde_json::json!({
                    "type": "agent_end",
                    "provider": provider_label,
                    "model": state.model,
                }));

                // Skip compaction if client already disconnected — avoids wasting a summarizer API call.
                // The next session will re-trigger compaction if the threshold is still exceeded.
                if !ws_alive {
                    tracing::info!("ws_chat: client disconnected before compaction, skipping");
                    trim_history(&mut history, state.max_history_messages);
                } else {

                // Auto-compaction: triggered by message count OR token threshold.
                let last_input_for_compaction = if context_used > 0 { Some(context_used) } else { None };
                if let Ok(true) = auto_compact_history(
                    &mut history,
                    state.provider.as_ref(),
                    &state.model,
                    state.max_history_messages,
                    last_input_for_compaction,
                    state.max_context_tokens,
                    capsule_store.as_ref(),
                    plaw_session_id.as_deref(),
                    embedding_provider.as_ref(),
                ).await {
                    trim_history(&mut history, state.max_history_messages);
                    let remaining_tokens = crate::agent::loop_::history::estimate_history_tokens(&history);
                    // Check if the compaction summary mentions pending tasks
                    let has_pending = history.iter().any(|m| {
                        m.content.starts_with("[Compaction summary]")
                            && summary_has_pending_tasks(&m.content)
                    });
                    let compacted_event = serde_json::json!({
                        "type": "compacted",
                        "remaining_messages": history.len(),
                        "estimated_tokens": remaining_tokens,
                        "has_pending_tasks": has_pending,
                    });
                    let _ = ws_tx.send(Message::Text(compacted_event.to_string().into())).await;
                    tracing::info!("ws_chat: auto-compaction triggered (context_used={context_used}, remaining_messages={}, has_pending={has_pending})", history.len());
                } else {
                    // Still apply hard trim as safety net even without compaction
                    trim_history(&mut history, state.max_history_messages);
                }

                // ── Skills hot-reload: detect newly installed skills ──
                let (current_skills, reload_skill_mode, reload_skill_workspace) = {
                    let cg = state.config.lock();
                    let skills = crate::skills::load_skills_with_config(&cg.workspace_dir, &cg);
                    let mode = cg.skills.prompt_injection_mode;
                    let ws_dir = cg.workspace_dir.clone();
                    (skills, mode, ws_dir)
                }; // lock released here

                let mut new_skills: Vec<_> = current_skills
                    .into_iter()
                    .filter(|s| !known_skill_names.contains(&s.name))
                    .collect();

                if !new_skills.is_empty() {
                    let new_names: Vec<&str> = new_skills.iter().map(|s| s.name.as_str()).collect();
                    tracing::info!("ws_chat: detected {} new skill(s): {:?}", new_skills.len(), new_names);

                    // Compute embeddings for new skills (for per-turn semantic filtering)
                    if let Some(ref emb) = embedding_provider {
                        crate::skills::compute_skill_embeddings(&mut new_skills, emb.as_ref()).await;
                    }

                    let snippet = crate::skills::skills_to_prompt_with_mode(
                        &new_skills,
                        &reload_skill_workspace,
                        reload_skill_mode,
                    );

                    // Inject system message so AI knows about new skills
                    let reload_msg = format!(
                        "[Skills hot-reload] {} new skill(s) installed during this session. \
                         You can now use them immediately.\n\n{}",
                        new_skills.len(),
                        snippet,
                    );
                    history.push(ChatMessage::system(&reload_msg));

                    // Update known set and all_skills for semantic matching
                    for skill in &new_skills {
                        known_skill_names.insert(skill.name.clone());
                    }

                    // Notify frontend (before consuming new_skills)
                    let reload_event = serde_json::json!({
                        "type": "skills_reloaded",
                        "new_skills": new_skills.iter().map(|s| {
                            serde_json::json!({
                                "name": s.name,
                                "description": s.description,
                            })
                        }).collect::<Vec<_>>(),
                        "total_skills": known_skill_names.len(),
                    });
                    let _ = ws_tx.send(Message::Text(reload_event.to_string().into())).await;

                    // Add new skills to the session's skill pool
                    all_skills.extend(new_skills);
                }

                } // end ws_alive else
            }
            Err(e) => {
                if is_tool_loop_cancelled(&e) {
                    let has_followup = pending_user_msg.is_some();
                    tracing::info!(
                        has_followup,
                        "ws_chat: agent loop cancelled by client"
                    );
                    let cancelled = serde_json::json!({
                        "type": "done",
                        "full_response": "",
                        "cancelled": true,
                    });
                    let _ = ws_tx.send(Message::Text(cancelled.to_string().into())).await;
                    let _ = state.event_tx.send(serde_json::json!({
                        "type": "agent_end",
                        "provider": provider_label,
                        "model": state.model,
                        "cancelled": true,
                    }));
                    // Clean up browser processes on cancel
                    crate::tools::cleanup_browser_processes().await;
                    // Stay in the session loop — if there's a pending follow-up message,
                    // the outer loop will pick it up immediately. Otherwise, we wait
                    // for the next client message.
                    continue;
                }

                let sanitized = crate::providers::sanitize_api_error(&e.to_string());
                let err = serde_json::json!({
                    "type": "error",
                    "message": sanitized,
                });
                let _ = ws_tx.send(Message::Text(err.to_string().into())).await;

                let _ = state.event_tx.send(serde_json::json!({
                    "type": "error",
                    "component": "ws_chat",
                    "message": sanitized,
                }));
            }
        }
    }

    // WebSocket session ended (disconnect, refresh, session switch, app close).
    // Clean up any orphaned browser processes so they don't linger.
    crate::tools::cleanup_browser_processes().await;
}

/// Convert an `on_delta` message from the agent loop into WebSocket JSON events.
///
/// Delta message formats from `run_tool_call_loop`:
/// - `"\x00PROGRESS\x00<emoji> Thinking...\n"` — LLM thinking phase
/// - `"\x00PROGRESS\x00<emoji> Got N tool call(s) (Xs)\n"` — tool calls parsed
/// - `"\x00PROGRESS\x00<emoji> tool_name: hint\n"` — tool execution starting (hourglass)
/// - `"\x00PROGRESS\x00<checkmark> tool_name (Xs)\n"` — tool completed
/// - `"\x00CLEAR\x00"` — clear progress (before final answer)
/// - Plain text — final response chunks
fn delta_to_ws_events(delta: &str) -> Vec<String> {
    if delta == DRAFT_CLEAR_SENTINEL {
        return vec![];
    }

    if let Some(progress) = delta.strip_prefix(DRAFT_PROGRESS_SENTINEL) {
        let text = progress.trim();
        if text.is_empty() {
            return vec![];
        }

        // Tool progress: "\x00TOOL_PROGRESS\x00tool_name|message"
        if let Some(rest) = text.strip_prefix("\x00TOOL_PROGRESS\x00") {
            if let Some((name, msg)) = rest.split_once('|') {
                let event = serde_json::json!({
                    "type": "tool_progress",
                    "name": name,
                    "message": msg,
                });
                return vec![event.to_string()];
            }
            return vec![];
        }

        // Strip leading emoji (1-2 chars that are non-ASCII)
        let content = text
            .char_indices()
            .find(|(_, c)| c.is_ascii_alphanumeric() || *c == ' ')
            .map(|(i, _)| text[i..].trim_start())
            .unwrap_or(text);

        // Tool start: "tool_name: hint" or just "tool_name"
        // Preceded by hourglass emoji in original
        if text.starts_with('\u{23f3}') {
            let (name, hint) = match content.split_once(": ") {
                Some((n, h)) => (n.trim(), h.trim()),
                None => (content.trim(), ""),
            };
            let event = serde_json::json!({
                "type": "tool_call",
                "name": name,
                "args": { "hint": hint },
            });
            return vec![event.to_string()];
        }

        // Tool completed: checkmark or cross + "tool_name (Xs)"
        if text.starts_with('\u{2705}') || text.starts_with('\u{274c}') {
            let success = text.starts_with('\u{2705}');
            // Parse "tool_name (Xs)" — extract name and duration
            let (name, duration) = match content.rfind(" (") {
                Some(pos) => (content[..pos].trim(), &content[pos..]),
                None => (content.trim(), ""),
            };
            let output = if success {
                format!("completed{duration}")
            } else {
                format!("failed{duration}")
            };
            let event = serde_json::json!({
                "type": "tool_result",
                "name": name,
                "output": output,
            });
            return vec![event.to_string()];
        }

        // Thinking / other progress — send as thinking event
        let event = serde_json::json!({
            "type": "thinking",
            "content": content,
        });
        return vec![event.to_string()];
    }

    // Plain text — final response chunk
    if !delta.is_empty() {
        let event = serde_json::json!({
            "type": "chunk",
            "content": delta,
        });
        return vec![event.to_string()];
    }

    vec![]
}

fn extract_ws_bearer_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth_header) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
    {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            if !token.trim().is_empty() {
                return Some(token.trim().to_string());
            }
        }
    }

    let offered = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())?;

    for protocol in offered.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(token) = protocol.strip_prefix("bearer.") {
            if !token.trim().is_empty() {
                return Some(token.trim().to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolResult};
    use async_trait::async_trait;
    use axum::http::HeaderValue;

    #[test]
    fn extract_ws_bearer_token_prefers_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer from-auth-header"),
        );
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("plaw.v1, bearer.from-protocol"),
        );

        assert_eq!(
            extract_ws_bearer_token(&headers).as_deref(),
            Some("from-auth-header")
        );
    }

    #[test]
    fn extract_ws_bearer_token_reads_websocket_protocol_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("plaw.v1, bearer.protocol-token"),
        );

        assert_eq!(
            extract_ws_bearer_token(&headers).as_deref(),
            Some("protocol-token")
        );
    }

    #[test]
    fn extract_ws_bearer_token_rejects_empty_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer    "),
        );
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("plaw.v1, bearer."),
        );

        assert!(extract_ws_bearer_token(&headers).is_none());
    }

    struct MockScheduleTool;

    #[async_trait]
    impl Tool for MockScheduleTool {
        fn name(&self) -> &str {
            "schedule"
        }

        fn description(&self) -> &str {
            "Mock schedule tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string" }
                }
            })
        }

        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
            Ok(ToolResult {
                success: true,
                output: "ok".to_string(),
                error: None,
            })
        }
    }

    #[test]
    fn sanitize_ws_response_removes_tool_call_tags() {
        let input = r#"Before
<tool_call>
{"name":"schedule","arguments":{"action":"create"}}
</tool_call>
After"#;

        let result = sanitize_ws_response(input, &[]);
        let normalized = result
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(normalized, "Before\nAfter");
        assert!(!result.contains("<tool_call>"));
        assert!(!result.contains("\"name\":\"schedule\""));
    }

    #[test]
    fn sanitize_ws_response_removes_isolated_tool_json_artifacts() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(MockScheduleTool)];
        let input = r#"{"name":"schedule","parameters":{"action":"create"}}
{"result":{"status":"scheduled"}}
Reminder set successfully."#;

        let result = sanitize_ws_response(input, &tools);
        assert_eq!(result, "Reminder set successfully.");
        assert!(!result.contains("\"name\":\"schedule\""));
        assert!(!result.contains("\"result\""));
    }

    #[test]
    fn finalize_ws_response_uses_prompt_mode_tool_output_when_final_text_empty() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(MockScheduleTool)];
        let history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user(
                "[Tool results]\n<tool_result name=\"schedule\">\nDisk usage: 72%\n</tool_result>",
            ),
        ];

        let result = finalize_ws_response("", &history, &tools);
        assert!(result.contains("Latest tool output:"));
        assert!(result.contains("Disk usage: 72%"));
        assert!(!result.contains("<tool_result"));
    }

    #[test]
    fn finalize_ws_response_uses_native_tool_message_output_when_final_text_empty() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(MockScheduleTool)];
        let history = vec![ChatMessage {
            role: "tool".to_string(),
            content: r#"{"tool_call_id":"call_1","content":"Filesystem /dev/disk3s1: 210G free"}"#
                .to_string(),
        }];

        let result = finalize_ws_response("", &history, &tools);
        assert!(result.contains("Latest tool output:"));
        assert!(result.contains("/dev/disk3s1"));
    }

    #[test]
    fn finalize_ws_response_uses_static_fallback_when_nothing_available() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(MockScheduleTool)];
        let history = vec![ChatMessage::system("sys")];

        let result = finalize_ws_response("", &history, &tools);
        assert_eq!(result, EMPTY_WS_RESPONSE_FALLBACK);
    }
}
