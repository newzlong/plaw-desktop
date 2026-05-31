use crate::approval::{ApprovalManager, ApprovalRequest, ApprovalResponse};
use crate::config::Config;
use crate::memory::{self, Memory, MemoryCategory};
use crate::multimodal;
use crate::observability::{self, runtime_trace, Observer, ObserverEvent};
use crate::providers::{
    self, ChatMessage, ChatRequest, Provider, ProviderCapabilityError,
};
#[cfg(test)]
use crate::providers::ToolCall;
use crate::runtime;
use crate::security::SecurityPolicy;
use crate::tools::{self, Tool};
use crate::util::truncate_with_ellipsis;
use anyhow::Result;
use rustyline::error::ReadlineError;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod autosave;
mod budgets;
mod context;
mod credentials;
mod errors;
mod execution;
pub(crate) mod history;
mod native_tools;
mod non_cli_approval;
mod parsing;
mod shell_policy;
mod streaming;
mod tool_instructions;
mod tool_io;
mod tool_taxonomy;
mod wrappers;

use autosave::autosave_memory_key;
use context::{build_context, build_hardware_context};
pub(crate) use credentials::scrub_credentials;
pub(crate) use errors::{
    is_tool_iteration_limit_error, is_tool_loop_cancelled, ToolIterationLimit, ToolLoopCancelled,
};
use execution::{
    execute_tools_parallel, execute_tools_sequential, should_execute_tools_in_parallel,
    ToolExecutionOutcome,
};
#[cfg(test)]
use history::{apply_compaction_summary, build_compaction_transcript};
use history::{auto_compact_history, mid_loop_trim_if_needed, trim_history};
#[allow(unused_imports)]
use parsing::{
    default_param_for_tool, detect_tool_call_parse_issue, extract_json_values, map_tool_name_alias,
    parse_arguments_value, parse_glm_shortened_body, parse_glm_style_tool_calls,
    parse_perl_style_tool_calls, parse_structured_tool_calls, parse_tool_call_value,
    parse_tool_calls, parse_tool_calls_from_json_value, tool_call_signature, ParsedToolCall,
};
use budgets::{
    AUTOSAVE_MIN_MESSAGE_CHARS, DEFAULT_MAX_TOOL_ITERATIONS, STREAM_CHUNK_MIN_CHARS,
};
use native_tools::{
    build_native_assistant_history, build_native_assistant_history_from_parsed_calls,
};
pub(crate) use non_cli_approval::{NonCliApprovalContext, NonCliApprovalPrompt};
use non_cli_approval::await_non_cli_approval_decision;
pub(crate) use shell_policy::build_shell_policy_instructions;
pub(crate) use streaming::{DRAFT_CLEAR_SENTINEL, DRAFT_PROGRESS_SENTINEL};
pub(crate) use tool_instructions::{build_tool_instructions, build_tool_instructions_from_specs};
pub(crate) use wrappers::{
    run_tool_call_loop_with_non_cli_approval_context, with_resume_lineage, ResumeLineage,
};
use tool_io::{
    append_calibration_reminder, maybe_inject_cron_add_delivery, tag_injected_content,
    truncate_tool_args_for_progress, wrap_as_untrusted_data,
};
use tool_taxonomy::{ANTI_LOOP_EXEMPT_TOOLS, MAX_SAME_TOOL_PER_TURN, TIGHT_LOOP_TOOLS};

/// Execute a single turn of the agent loop: send messages, parse tool calls,
/// execute tools, and loop until the LLM produces a final text response.
/// When `silent` is true, suppresses stdout (for channel use).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn agent_turn(
    provider: &dyn Provider,
    history: &mut Vec<ChatMessage>,
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    provider_name: &str,
    model: &str,
    temperature: f64,
    silent: bool,
    multimodal_config: &crate::config::MultimodalConfig,
    max_tool_iterations: usize,
) -> Result<String> {
    run_tool_call_loop(
        provider,
        history,
        tools_registry,
        observer,
        provider_name,
        model,
        temperature,
        silent,
        None,
        "channel",
        multimodal_config,
        max_tool_iterations,
        None,
        None,
        None,
        &[],
    )
    .await
}

// ── Agent Tool-Call Loop ──────────────────────────────────────────────────
// Core agentic iteration: send conversation to the LLM, parse any tool
// calls from the response, execute them, append results to history, and
// repeat until the LLM produces a final text-only answer.
//
// Loop invariant: at the start of each iteration, `history` contains the
// full conversation so far (system prompt + user messages + prior tool
// results). The loop exits when:
//   • the LLM returns no tool calls (final answer), or
//   • max_iterations is reached (runaway safety), or
//   • the cancellation token fires (external abort).

/// Execute a single turn of the agent loop: send messages, parse tool calls,
/// execute tools, and loop until the LLM produces a final text response.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_tool_call_loop(
    provider: &dyn Provider,
    history: &mut Vec<ChatMessage>,
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    provider_name: &str,
    model: &str,
    temperature: f64,
    silent: bool,
    approval: Option<&ApprovalManager>,
    channel_name: &str,
    multimodal_config: &crate::config::MultimodalConfig,
    max_tool_iterations: usize,
    cancellation_token: Option<CancellationToken>,
    on_delta: Option<tokio::sync::mpsc::Sender<String>>,
    hooks: Option<&crate::hooks::HookRunner>,
    excluded_tools: &[String],
) -> Result<String> {
    let non_cli_approval_context = wrappers::current_non_cli_approval_context();
    let channel_reply_target = wrappers::current_reply_target().or_else(|| {
        non_cli_approval_context
            .as_ref()
            .map(|ctx| ctx.reply_target.clone())
    });

    let max_iterations = if max_tool_iterations == 0 {
        DEFAULT_MAX_TOOL_ITERATIONS
    } else {
        max_tool_iterations
    };

    let tool_specs: Vec<crate::tools::ToolSpec> = tools_registry
        .iter()
        .filter(|tool| !excluded_tools.iter().any(|ex| ex == tool.name()))
        .map(|tool| tool.spec())
        .collect();
    let use_native_tools = provider.supports_native_tools() && !tool_specs.is_empty();
    let turn_id = Uuid::new_v4().to_string();
    let mut seen_tool_signatures: HashSet<(String, String)> = HashSet::new();
    let mut tool_call_counts: HashMap<String, usize> = HashMap::new();
    let bypass_non_cli_approval_for_turn =
        approval.is_some_and(|mgr| channel_name != "cli" && mgr.consume_non_cli_allow_all_once());
    if bypass_non_cli_approval_for_turn {
        runtime_trace::record_event(
            "approval_bypass_one_time_all_tools_consumed",
            Some(channel_name),
            Some(provider_name),
            Some(model),
            Some(&turn_id),
            Some(true),
            Some("consumed one-time non-cli allow-all approval token"),
            serde_json::json!({}),
        );
    }

    for iteration in 0..max_iterations {
        if cancellation_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ToolLoopCancelled.into());
        }

        let image_marker_count = multimodal::count_image_markers(history);
        if image_marker_count > 0 && !provider.supports_vision() {
            return Err(ProviderCapabilityError {
                provider: provider_name.to_string(),
                capability: "vision".to_string(),
                message: format!(
                    "received {image_marker_count} image marker(s), but this provider does not support vision input"
                ),
            }
            .into());
        }

        let prepared_messages =
            multimodal::prepare_messages_for_provider(history, multimodal_config).await?;

        // ── Progress: LLM thinking ────────────────────────────
        if let Some(ref tx) = on_delta {
            let phase = if iteration == 0 {
                "\u{1f914} Thinking...\n".to_string()
            } else {
                format!("\u{1f914} Thinking (round {})...\n", iteration + 1)
            };
            let _ = tx.send(format!("{DRAFT_PROGRESS_SENTINEL}{phase}")).await;
        }

        observer.record_event(&ObserverEvent::LlmRequest {
            provider: provider_name.to_string(),
            model: model.to_string(),
            messages_count: history.len(),
        });
        runtime_trace::record_event(
            "llm_request",
            Some(channel_name),
            Some(provider_name),
            Some(model),
            Some(&turn_id),
            None,
            None,
            serde_json::json!({
                "iteration": iteration + 1,
                "messages_count": history.len(),
            }),
        );

        let llm_started_at = Instant::now();

        // Fire void observational hook (existing observers rely on this for
        // logging the unmodified `history`; we keep it firing for back-compat).
        if let Some(hooks) = hooks {
            hooks.fire_llm_input(history, model).await;
        }

        // Unified path via Provider::chat so provider-specific native tool logic
        // (OpenAI/Anthropic/OpenRouter/compatible adapters) is honored.
        let request_tools = if use_native_tools {
            Some(tool_specs.as_slice())
        } else {
            None
        };

        // Modifying `before_llm_call` hook: lets hooks rewrite the message
        // list (e.g. redact, summarize) or swap the model. Always clones so
        // the existing borrow chain `&prepared_messages.messages` stays
        // intact when hooks=None; allocation cost is negligible next to a
        // network round-trip. Cancel from any hook aborts the turn.
        let (effective_messages, effective_model) = if let Some(hooks) = hooks {
            match hooks
                .run_before_llm_call(prepared_messages.messages.clone(), model.to_string())
                .await
            {
                crate::hooks::HookResult::Continue(pair) => pair,
                crate::hooks::HookResult::Cancel(reason) => {
                    return Err(anyhow::anyhow!(
                        "LLM call cancelled by before_llm_call hook: {reason}"
                    ));
                }
            }
        } else {
            (prepared_messages.messages.clone(), model.to_string())
        };

        // Use the streaming-capable entry point: providers that override it
        // (Anthropic today, others later) push each text_delta to on_delta
        // as it arrives over SSE — chunk events flow to the WebSocket
        // before the full response finishes. Providers that don't override
        // chat_streaming inherit the default impl that delegates to chat(),
        // so non-streaming providers behave identically to before.
        let chat_future = provider.chat_streaming(
            ChatRequest {
                messages: &effective_messages,
                tools: request_tools,
            },
            &effective_model,
            temperature,
            on_delta.as_ref(),
        );

        let chat_result = if let Some(token) = cancellation_token.as_ref() {
            tokio::select! {
                () = token.cancelled() => return Err(ToolLoopCancelled.into()),
                result = chat_future => result,
            }
        } else {
            chat_future.await
        };

        let (response_text, parsed_text, tool_calls, assistant_history_content, native_tool_calls) =
            match chat_result {
                Ok(resp) => {
                    let (resp_input_tokens, resp_output_tokens) = resp
                        .usage
                        .as_ref()
                        .map(|u| (u.input_tokens, u.output_tokens))
                        .unwrap_or((None, None));

                    observer.record_event(&ObserverEvent::LlmResponse {
                        provider: provider_name.to_string(),
                        model: model.to_string(),
                        duration: llm_started_at.elapsed(),
                        success: true,
                        error_message: None,
                        input_tokens: resp_input_tokens,
                        output_tokens: resp_output_tokens,
                    });

                    let response_text = resp.text_or_empty().to_string();
                    // First try native structured tool calls (OpenAI-format).
                    // Fall back to text-based parsing (XML tags, markdown blocks,
                    // GLM format) only if the provider returned no native calls —
                    // this ensures we support both native and prompt-guided models.
                    let mut calls = parse_structured_tool_calls(&resp.tool_calls);
                    let mut parsed_text = String::new();

                    if calls.is_empty() {
                        let (fallback_text, fallback_calls) = parse_tool_calls(&response_text);
                        if !fallback_text.is_empty() {
                            parsed_text = fallback_text;
                        }
                        calls = fallback_calls;
                    }

                    if let Some(parse_issue) = detect_tool_call_parse_issue(&response_text, &calls)
                    {
                        runtime_trace::record_event(
                            "tool_call_parse_issue",
                            Some(channel_name),
                            Some(provider_name),
                            Some(model),
                            Some(&turn_id),
                            Some(false),
                            Some(&parse_issue),
                            serde_json::json!({
                                "iteration": iteration + 1,
                                "response_excerpt": truncate_with_ellipsis(
                                    &scrub_credentials(&response_text),
                                    600
                                ),
                            }),
                        );
                    }

                    runtime_trace::record_event(
                        "llm_response",
                        Some(channel_name),
                        Some(provider_name),
                        Some(model),
                        Some(&turn_id),
                        Some(true),
                        None,
                        serde_json::json!({
                            "iteration": iteration + 1,
                            "duration_ms": llm_started_at.elapsed().as_millis(),
                            "input_tokens": resp_input_tokens,
                            "output_tokens": resp_output_tokens,
                            "raw_response": scrub_credentials(&response_text),
                            "native_tool_calls": resp.tool_calls.len(),
                            "parsed_tool_calls": calls.len(),
                        }),
                    );

                    // Preserve native tool call IDs in assistant history so role=tool
                    // follow-up messages can reference the exact call id.
                    let reasoning_content = resp.reasoning_content.clone();
                    let assistant_history_content = if resp.tool_calls.is_empty() {
                        if use_native_tools {
                            build_native_assistant_history_from_parsed_calls(
                                &response_text,
                                &calls,
                                reasoning_content.as_deref(),
                            )
                            .unwrap_or_else(|| response_text.clone())
                        } else {
                            response_text.clone()
                        }
                    } else {
                        build_native_assistant_history(
                            &response_text,
                            &resp.tool_calls,
                            reasoning_content.as_deref(),
                        )
                    };

                    let native_calls = resp.tool_calls;
                    (
                        response_text,
                        parsed_text,
                        calls,
                        assistant_history_content,
                        native_calls,
                    )
                }
                Err(e) => {
                    let safe_error = crate::providers::sanitize_api_error(&e.to_string());
                    observer.record_event(&ObserverEvent::LlmResponse {
                        provider: provider_name.to_string(),
                        model: model.to_string(),
                        duration: llm_started_at.elapsed(),
                        success: false,
                        error_message: Some(safe_error.clone()),
                        input_tokens: None,
                        output_tokens: None,
                    });
                    runtime_trace::record_event(
                        "llm_response",
                        Some(channel_name),
                        Some(provider_name),
                        Some(model),
                        Some(&turn_id),
                        Some(false),
                        Some(&safe_error),
                        serde_json::json!({
                            "iteration": iteration + 1,
                            "duration_ms": llm_started_at.elapsed().as_millis(),
                        }),
                    );
                    return Err(e);
                }
            };

        // When the response carries tool calls, prefer parsed_text (narrative
        // with tool-call XML stripped). Falling back to raw response_text would
        // leak the prompt-mode `<tool_call>` payload into user-visible streams.
        // Only fall back to response_text when there are no tool calls (and
        // parsed_text is therefore not yet populated).
        let display_text = if !tool_calls.is_empty() {
            parsed_text
        } else if parsed_text.is_empty() {
            response_text.clone()
        } else {
            parsed_text
        };

        // ── Progress: LLM responded ─────────────────────────────
        if let Some(ref tx) = on_delta {
            let llm_secs = llm_started_at.elapsed().as_secs();
            if !tool_calls.is_empty() {
                let _ = tx
                    .send(format!(
                        "{DRAFT_PROGRESS_SENTINEL}\u{1f4ac} Got {} tool call(s) ({llm_secs}s)\n",
                        tool_calls.len()
                    ))
                    .await;
            }
        }

        if tool_calls.is_empty() {
            runtime_trace::record_event(
                "turn_final_response",
                Some(channel_name),
                Some(provider_name),
                Some(model),
                Some(&turn_id),
                Some(true),
                None,
                serde_json::json!({
                    "iteration": iteration + 1,
                    "text": scrub_credentials(&display_text),
                }),
            );
            // No tool calls — this is the final response.
            // If a streaming sender is provided, relay the text in small chunks
            // so the channel can progressively update the draft message.
            if let Some(ref tx) = on_delta {
                // Clear accumulated progress lines before streaming the final answer.
                let _ = tx.send(DRAFT_CLEAR_SENTINEL.to_string()).await;
                // Split on whitespace boundaries, accumulating chunks of at least
                // STREAM_CHUNK_MIN_CHARS characters for progressive draft updates.
                let mut chunk = String::new();
                for word in display_text.split_inclusive(char::is_whitespace) {
                    if cancellation_token
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
                    {
                        return Err(ToolLoopCancelled.into());
                    }
                    chunk.push_str(word);
                    if chunk.len() >= STREAM_CHUNK_MIN_CHARS
                        && tx.send(std::mem::take(&mut chunk)).await.is_err()
                    {
                        break; // receiver dropped
                    }
                }
                if !chunk.is_empty() {
                    let _ = tx.send(chunk).await;
                }
            }
            // Only push non-empty assistant messages to history.
            // Empty messages cause 400 errors on some providers (e.g. Kimi).
            if !response_text.trim().is_empty() {
                history.push(ChatMessage::assistant(response_text.clone()));
            }
            return Ok(display_text);
        }

        // Send any text the LLM produced alongside tool calls to both
        // stdout (CLI) and on_delta (WebSocket/gateway).
        if !display_text.is_empty() {
            if !silent {
                print!("{display_text}");
                let _ = std::io::stdout().flush();
            }
            // Relay intermediate text to the frontend so it's visible before tool execution
            if let Some(ref tx) = on_delta {
                let mut chunk = String::new();
                for word in display_text.split_inclusive(char::is_whitespace) {
                    chunk.push_str(word);
                    if chunk.len() >= STREAM_CHUNK_MIN_CHARS {
                        let _ = tx.send(std::mem::take(&mut chunk)).await;
                    }
                }
                if !chunk.is_empty() {
                    let _ = tx.send(chunk).await;
                }
            }
        }

        // Execute tool calls and build results. `individual_results` tracks per-call output so
        // native-mode history can emit one role=tool message per tool call with the correct ID.
        //
        // When multiple tool calls are present and interactive CLI approval is not needed, run
        // tool executions concurrently for lower wall-clock latency.
        let mut tool_results = String::new();
        let mut individual_results: Vec<(Option<String>, String)> = Vec::new();
        let mut ordered_results: Vec<Option<(String, Option<String>, ToolExecutionOutcome)>> =
            (0..tool_calls.len()).map(|_| None).collect();
        let allow_parallel_execution =
            should_execute_tools_in_parallel(&tool_calls, tools_registry, approval);
        let mut executable_indices: Vec<usize> = Vec::new();
        let mut executable_calls: Vec<ParsedToolCall> = Vec::new();

        for (idx, call) in tool_calls.iter().enumerate() {
            // ── Hook: before_tool_call (modifying) ──────────
            let mut tool_name = call.name.clone();
            let mut tool_args = call.arguments.clone();
            if let Some(hooks) = hooks {
                match hooks
                    .run_before_tool_call(tool_name.clone(), tool_args.clone())
                    .await
                {
                    crate::hooks::HookResult::Cancel(reason) => {
                        tracing::info!(tool = %call.name, %reason, "tool call cancelled by hook");
                        let cancelled = format!("Cancelled by hook: {reason}");
                        runtime_trace::record_event(
                            "tool_call_result",
                            Some(channel_name),
                            Some(provider_name),
                            Some(model),
                            Some(&turn_id),
                            Some(false),
                            Some(&cancelled),
                            serde_json::json!({
                                "iteration": iteration + 1,
                                "tool": call.name,
                                "arguments": scrub_credentials(&tool_args.to_string()),
                            }),
                        );
                        ordered_results[idx] = Some((
                            call.name.clone(),
                            call.tool_call_id.clone(),
                            ToolExecutionOutcome {
                                output: cancelled,
                                success: false,
                                error_reason: Some(scrub_credentials(&reason)),
                                duration: Duration::ZERO,
                            },
                        ));
                        continue;
                    }
                    crate::hooks::HookResult::Continue((name, args)) => {
                        tool_name = name;
                        tool_args = args;
                    }
                }
            }

            maybe_inject_cron_add_delivery(
                &tool_name,
                &mut tool_args,
                channel_name,
                channel_reply_target.as_deref(),
            );

            if excluded_tools.iter().any(|ex| ex == &tool_name) {
                let blocked = format!("Tool '{tool_name}' is not available in this channel.");
                runtime_trace::record_event(
                    "tool_call_result",
                    Some(channel_name),
                    Some(provider_name),
                    Some(model),
                    Some(&turn_id),
                    Some(false),
                    Some(&blocked),
                    serde_json::json!({
                        "iteration": iteration + 1,
                        "tool": tool_name.clone(),
                        "arguments": scrub_credentials(&tool_args.to_string()),
                        "blocked_by_channel_policy": true,
                    }),
                );
                ordered_results[idx] = Some((
                    tool_name.clone(),
                    call.tool_call_id.clone(),
                    ToolExecutionOutcome {
                        output: blocked.clone(),
                        success: false,
                        error_reason: Some(blocked),
                        duration: Duration::ZERO,
                    },
                ));
                continue;
            }

            // ── Approval hook ────────────────────────────────
            if let Some(mgr) = approval {
                if bypass_non_cli_approval_for_turn {
                    mgr.record_decision(
                        &tool_name,
                        &tool_args,
                        ApprovalResponse::Yes,
                        channel_name,
                    );
                } else if mgr.needs_approval(&tool_name, &tool_args) {
                    let request = ApprovalRequest {
                        tool_name: tool_name.clone(),
                        arguments: tool_args.clone(),
                    };

                    let decision = if channel_name == "cli" {
                        mgr.prompt_cli(&request)
                    } else if let Some(ctx) = non_cli_approval_context.as_ref() {
                        // Fingerprint = canonical JSON form of the call's
                        // arguments. Passed to dedup so two parallel calls
                        // of the same tool with different args (e.g.
                        // git_operations status vs git_operations diff)
                        // each get their own pending + their own action
                        // card, instead of collapsing into one shared
                        // approval that auto-resolves both when the user
                        // clicks the first card.
                        let args_fingerprint = tool_args.to_string();
                        let pending = mgr.create_non_cli_pending_request(
                            &tool_name,
                            &ctx.sender,
                            channel_name,
                            &ctx.reply_target,
                            Some(&args_fingerprint),
                            Some(
                                "interactive approval required for supervised non-cli tool execution"
                                    .to_string(),
                            ),
                        );

                        let _ = ctx.prompt_tx.send(NonCliApprovalPrompt {
                            request_id: pending.request_id.clone(),
                            tool_name: tool_name.clone(),
                            arguments: tool_args.clone(),
                        });

                        await_non_cli_approval_decision(
                            mgr,
                            &pending.request_id,
                            &ctx.sender,
                            channel_name,
                            &ctx.reply_target,
                            cancellation_token.as_ref(),
                        )
                        .await
                    } else {
                        ApprovalResponse::No
                    };

                    mgr.record_decision(&tool_name, &tool_args, decision, channel_name);

                    if decision == ApprovalResponse::No {
                        let denied = "Denied by user.".to_string();
                        runtime_trace::record_event(
                            "tool_call_result",
                            Some(channel_name),
                            Some(provider_name),
                            Some(model),
                            Some(&turn_id),
                            Some(false),
                            Some(&denied),
                            serde_json::json!({
                                "iteration": iteration + 1,
                                "tool": tool_name.clone(),
                                "arguments": scrub_credentials(&tool_args.to_string()),
                            }),
                        );
                        ordered_results[idx] = Some((
                            tool_name.clone(),
                            call.tool_call_id.clone(),
                            ToolExecutionOutcome {
                                output: denied.clone(),
                                success: false,
                                error_reason: Some(denied),
                                duration: Duration::ZERO,
                            },
                        ));
                        continue;
                    }
                }
            }

            let signature = tool_call_signature(&tool_name, &tool_args);
            if !seen_tool_signatures.insert(signature) {
                let duplicate = format!(
                    "Skipped duplicate tool call '{tool_name}' with identical arguments in this turn."
                );
                runtime_trace::record_event(
                    "tool_call_result",
                    Some(channel_name),
                    Some(provider_name),
                    Some(model),
                    Some(&turn_id),
                    Some(false),
                    Some(&duplicate),
                    serde_json::json!({
                        "iteration": iteration + 1,
                        "tool": tool_name.clone(),
                        "arguments": scrub_credentials(&tool_args.to_string()),
                        "deduplicated": true,
                    }),
                );
                ordered_results[idx] = Some((
                    tool_name.clone(),
                    call.tool_call_id.clone(),
                    ToolExecutionOutcome {
                        output: duplicate.clone(),
                        success: false,
                        error_reason: Some(duplicate),
                        duration: Duration::ZERO,
                    },
                ));
                continue;
            }

            // Anti-loop: track per-tool-name call frequency across the entire turn.
            // Browser and file tools are exempt — they naturally need many sequential calls.
            let count = tool_call_counts.entry(tool_name.clone()).or_insert(0);
            *count += 1;
            let is_exempt = ANTI_LOOP_EXEMPT_TOOLS.contains(&tool_name.as_str());
            let tight_limit = TIGHT_LOOP_TOOLS
                .iter()
                .find(|(n, _)| *n == tool_name)
                .map(|(_, lim)| *lim);
            let per_tool_limit = tight_limit.unwrap_or(MAX_SAME_TOOL_PER_TURN);
            if !is_exempt && *count > per_tool_limit {
                let anti_loop_msg = format!(
                    "Anti-loop protection: tool '{}' has been called {} times this turn (limit {}). \
                     Skipping further calls. Summarize what you have so far and present results to the user.",
                    tool_name, count, per_tool_limit
                );
                tracing::warn!(
                    tool = %tool_name,
                    count = *count,
                    limit = per_tool_limit,
                    "Anti-loop: tool call frequency exceeded"
                );
                runtime_trace::record_event(
                    "tool_call_result",
                    Some(channel_name),
                    Some(provider_name),
                    Some(model),
                    Some(&turn_id),
                    Some(false),
                    Some(&anti_loop_msg),
                    serde_json::json!({
                        "iteration": iteration + 1,
                        "tool": tool_name.clone(),
                        "anti_loop": true,
                        "call_count": *count,
                    }),
                );
                ordered_results[idx] = Some((
                    tool_name.clone(),
                    call.tool_call_id.clone(),
                    ToolExecutionOutcome {
                        output: anti_loop_msg.clone(),
                        success: false,
                        error_reason: Some(anti_loop_msg),
                        duration: Duration::ZERO,
                    },
                ));
                continue;
            }

            runtime_trace::record_event(
                "tool_call_start",
                Some(channel_name),
                Some(provider_name),
                Some(model),
                Some(&turn_id),
                None,
                None,
                serde_json::json!({
                    "iteration": iteration + 1,
                    "tool": tool_name.clone(),
                    "arguments": scrub_credentials(&tool_args.to_string()),
                }),
            );

            // ── Progress: tool start ────────────────────────────
            if let Some(ref tx) = on_delta {
                let hint = truncate_tool_args_for_progress(&tool_name, &tool_args, 60);
                let progress = if hint.is_empty() {
                    format!("\u{23f3} {}\n", tool_name)
                } else {
                    format!("\u{23f3} {}: {hint}\n", tool_name)
                };
                tracing::debug!(tool = %tool_name, "Sending progress start to draft");
                let _ = tx
                    .send(format!("{DRAFT_PROGRESS_SENTINEL}{progress}"))
                    .await;
            }

            executable_indices.push(idx);
            executable_calls.push(ParsedToolCall {
                name: tool_name,
                arguments: tool_args,
                tool_call_id: call.tool_call_id.clone(),
            });
        }

        let executed_outcomes = if allow_parallel_execution && executable_calls.len() > 1 {
            execute_tools_parallel(
                &executable_calls,
                tools_registry,
                observer,
                cancellation_token.as_ref(),
                on_delta.as_ref(),
            )
            .await?
        } else {
            execute_tools_sequential(
                &executable_calls,
                tools_registry,
                observer,
                cancellation_token.as_ref(),
                on_delta.as_ref(),
            )
            .await?
        };

        for ((idx, call), outcome) in executable_indices
            .iter()
            .zip(executable_calls.iter())
            .zip(executed_outcomes.into_iter())
        {
            runtime_trace::record_event(
                "tool_call_result",
                Some(channel_name),
                Some(provider_name),
                Some(model),
                Some(&turn_id),
                Some(outcome.success),
                outcome.error_reason.as_deref(),
                serde_json::json!({
                    "iteration": iteration + 1,
                    "tool": call.name.clone(),
                    "duration_ms": outcome.duration.as_millis(),
                    "output": scrub_credentials(&outcome.output),
                }),
            );

            // ── Hook: after_tool_call (void) ─────────────────
            if let Some(hooks) = hooks {
                let tool_result_obj = crate::tools::ToolResult {
                    success: outcome.success,
                    output: outcome.output.clone(),
                    error: None,
                };
                hooks
                    .fire_after_tool_call(&call.name, &tool_result_obj, outcome.duration)
                    .await;
            }

            // ── Progress: tool completion ───────────────────────
            if let Some(ref tx) = on_delta {
                let secs = outcome.duration.as_secs();
                let icon = if outcome.success {
                    "\u{2705}"
                } else {
                    "\u{274c}"
                };
                tracing::debug!(tool = %call.name, secs, "Sending progress complete to draft");
                let _ = tx
                    .send(format!(
                        "{DRAFT_PROGRESS_SENTINEL}{icon} {} ({secs}s)\n",
                        call.name
                    ))
                    .await;
            }

            ordered_results[*idx] = Some((call.name.clone(), call.tool_call_id.clone(), outcome));
        }

        for (tool_name, tool_call_id, outcome) in ordered_results.into_iter().flatten() {
            // Scan external tool results for prompt injection and tag if detected
            let tagged_output = tag_injected_content(&tool_name, outcome.output);
            // Wrap external content in <untrusted_data source="..."> delimiters
            // so the model is structurally aware of which content is external
            // (web/HTTP/search/browser/pdf) vs. trusted plaw-side instructions.
            // Paired with the system-prompt rule injected by
            // `build_untrusted_data_rule_block` so the LLM has both
            // structural and policy signals.
            let wrapped_output = wrap_as_untrusted_data(&tool_name, tagged_output);
            // Append calibration reminder to fight post-tool-call confabulation
            // on external content (see T-2 in phase-2-targets.md).
            let calibrated_output = append_calibration_reminder(&tool_name, wrapped_output);
            individual_results.push((tool_call_id, calibrated_output.clone()));
            let _ = writeln!(
                tool_results,
                "<tool_result name=\"{}\">\n{}\n</tool_result>",
                tool_name, calibrated_output
            );
        }

        // Add assistant message with tool calls + tool results to history.
        // Native mode: use JSON-structured messages so convert_messages() can
        // reconstruct proper OpenAI-format tool_calls and tool result messages.
        // Prompt mode: use XML-based text format as before.
        history.push(ChatMessage::assistant(assistant_history_content));
        if native_tool_calls.is_empty() {
            let all_results_have_ids = use_native_tools
                && !individual_results.is_empty()
                && individual_results
                    .iter()
                    .all(|(tool_call_id, _)| tool_call_id.is_some());
            if all_results_have_ids {
                for (tool_call_id, result) in &individual_results {
                    let tool_msg = serde_json::json!({
                        "tool_call_id": tool_call_id,
                        "content": result,
                    });
                    history.push(ChatMessage::tool(tool_msg.to_string()));
                }
            } else {
                history.push(ChatMessage::user(format!("[Tool results]\n{tool_results}")));
            }
        } else {
            for (native_call, (_, result)) in
                native_tool_calls.iter().zip(individual_results.iter())
            {
                let tool_msg = serde_json::json!({
                    "tool_call_id": native_call.id,
                    "content": result,
                });
                history.push(ChatMessage::tool(tool_msg.to_string()));
            }
        }

        // ── Mid-loop trim: prevent context overflow during long tool chains ──
        if mid_loop_trim_if_needed(history) {
            if let Some(ref tx) = on_delta {
                let _ = tx
                    .send(format!(
                        "{DRAFT_PROGRESS_SENTINEL}\u{2702}\u{fe0f} Context trimmed (keeping recent messages)\n"
                    ))
                    .await;
            }
        }

        // ── Checkpoint: durable snapshot at iteration boundary ──
        // Best-effort: a failing checkpoint writer logs and the loop
        // continues. Losing a snapshot is strictly better than failing
        // the turn over a transient filesystem error. See
        // [`crate::agent::checkpoint`].
        if let Some(writer) = wrappers::current_checkpoint_writer() {
            let mut snapshot = crate::agent::checkpoint::Snapshot::new(
                turn_id.clone(),
                iteration,
                history.clone(),
            );
            // Cross-turn lineage: `plaw resume <parent> --from-iteration N`
            // sets TOOL_LOOP_RESUME_LINEAGE; stamp it on iteration 0 so
            // a future tree-view UI can render the turn forest. Only
            // iteration 0 carries this — later iterations use
            // intra-turn `parent_iteration`.
            if iteration == 0 {
                if let Some(lineage) = wrappers::current_resume_lineage() {
                    snapshot = snapshot.with_lineage(
                        lineage.parent_turn_id,
                        lineage.branched_from_iteration,
                    );
                }
            }
            if let Err(err) = writer.put(&snapshot) {
                tracing::warn!(
                    %turn_id,
                    %iteration,
                    error = %err,
                    "checkpoint write failed; agent loop continues"
                );
            }
        }
    }

    runtime_trace::record_event(
        "tool_loop_exhausted",
        Some(channel_name),
        Some(provider_name),
        Some(model),
        Some(&turn_id),
        Some(false),
        Some("agent exceeded maximum tool iterations"),
        serde_json::json!({
            "max_iterations": max_iterations,
        }),
    );
    Err(ToolIterationLimit {
        limit: max_iterations,
    }
    .into())
}

/// Build the tool instruction block for the system prompt from concrete tool
/// specs so the LLM knows how to invoke tools.
// ── CLI Entrypoint ───────────────────────────────────────────────────────
// Wires up all subsystems (observer, runtime, security, memory, tools,
// provider, hardware RAG, peripherals) and enters either single-shot or
// interactive REPL mode. The interactive loop manages history compaction
// and hard trimming to keep the context window bounded.

/// Single-turn agent entry point with optional snapshot resume.
///
/// When `resume_from_turn_id` is `Some(turn_id)`, the agent loop's
/// `history` is seeded from a snapshot of that turn instead of starting
/// fresh — used by the `plaw resume` CLI for crash-recovery / mid-task
/// continuation / time-travel branching.
///
/// `resume_from_iteration`:
/// - `None` → resume from the LATEST snapshot (default crash-recovery
///   semantic; same as PR #61 behaviour).
/// - `Some(N)` → resume from iteration `N` exactly (time-travel branch).
///   Errors if iteration `N` was never persisted for that turn.
///
/// The resumed loop generates its OWN fresh turn_id; new snapshots land
/// under that new directory, preserving the original turn's history
/// untouched. Cross-turn lineage is recorded on the first snapshot
/// (`parent_turn_id` + `branched_from_iteration`) so debug UIs can
/// render the turn forest.
///
/// Interactive REPL mode does not yet support resume; pass `None` from
/// the REPL path.
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub async fn run(
    config: Config,
    message: Option<String>,
    provider_override: Option<String>,
    model_override: Option<String>,
    temperature: f64,
    peripheral_overrides: Vec<String>,
    interactive: bool,
    resume_from_turn_id: Option<String>,
    resume_from_iteration: Option<usize>,
) -> Result<String> {
    // ── Wire up agnostic subsystems ──────────────────────────────
    let base_observer = observability::create_observer(&config.observability);
    let observer: Arc<dyn Observer> = Arc::from(base_observer);
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(SecurityPolicy::from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));

    // ── Memory (the brain) ────────────────────────────────────────
    let mem: Arc<dyn Memory> = Arc::from(memory::create_memory_with_storage(
        &config.memory,
        Some(&config.storage.provider.config),
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);
    tracing::info!(backend = mem.name(), "Memory initialized");

    // ── Peripherals (merge peripheral tools into registry) ─
    if !peripheral_overrides.is_empty() {
        tracing::info!(
            peripherals = ?peripheral_overrides,
            "Peripheral overrides from CLI (config boards take precedence)"
        );
    }

    // ── Tools (including memory tools and peripherals) ────────────
    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };
    let mut tools_registry = tools::all_tools_with_runtime(
        Arc::new(config.clone()),
        &security,
        runtime,
        mem.clone(),
        composio_key,
        composio_entity_id,
        &config.browser,
        &config.http_request,
        &config.web_fetch,
        &config.workspace_dir,
        &config.agents,
        config.api_key.as_deref(),
        &config,
    );

    let peripheral_tools: Vec<Box<dyn Tool>> =
        crate::peripherals::create_peripheral_tools(&config.peripherals).await?;
    if !peripheral_tools.is_empty() {
        tracing::info!(count = peripheral_tools.len(), "Peripheral tools added");
        tools_registry.extend(peripheral_tools);
    }

    // ── Resolve provider ─────────────────────────────────────────
    let provider_name = provider_override
        .as_deref()
        .or(config.default_provider.as_deref())
        .unwrap_or("openrouter");

    let model_name = model_override
        .as_deref()
        .or(config.default_model.as_deref())
        .unwrap_or("anthropic/claude-sonnet-4");

    let provider_runtime_options = providers::ProviderRuntimeOptions {
        auth_profile_override: None,
        provider_api_url: config.api_url.clone(),
        plaw_dir: config.config_path.parent().map(std::path::PathBuf::from),
        secrets_encrypt: config.secrets.encrypt,
        reasoning_enabled: config.runtime.reasoning_enabled,
        reasoning_level: config.effective_provider_reasoning_level(),
        custom_provider_api_mode: config.provider_api.map(|mode| mode.as_compatible_mode()),
        max_tokens_override: None,
        model_support_vision: config.model_support_vision,
    };

    let provider: Box<dyn Provider> = providers::create_routed_provider_with_options(
        provider_name,
        config.api_key.as_deref(),
        config.api_url.as_deref(),
        &config.reliability,
        &config.model_routes,
        model_name,
        &provider_runtime_options,
    )?;

    observer.record_event(&ObserverEvent::AgentStart {
        provider: provider_name.to_string(),
        model: model_name.to_string(),
    });

    // ── Hardware RAG (datasheet retrieval when peripherals + datasheet_dir) ──
    let hardware_rag: Option<crate::rag::HardwareRag> = config
        .peripherals
        .datasheet_dir
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(|dir| crate::rag::HardwareRag::load(&config.workspace_dir, dir.trim()))
        .and_then(Result::ok)
        .filter(|r: &crate::rag::HardwareRag| !r.is_empty());
    if let Some(ref rag) = hardware_rag {
        tracing::info!(chunks = rag.len(), "Hardware RAG loaded");
    }

    let board_names: Vec<String> = config
        .peripherals
        .boards
        .iter()
        .map(|b| b.board.clone())
        .collect();

    // ── Build system prompt from workspace MD files (OpenClaw framework) ──
    let skills = crate::skills::load_skills_with_config(&config.workspace_dir, &config);
    let mut tool_descs: Vec<(&str, &str)> = vec![
        (
            "shell",
            "Execute terminal commands. Use when: running local checks, build/test commands, diagnostics. Don't use when: a safer dedicated tool exists, or command is destructive without approval.",
        ),
        (
            "file_read",
            "Read file contents. Use when: inspecting project files, configs, logs. Don't use when: a targeted search is enough.",
        ),
        (
            "file_write",
            "Write file contents. Use when: applying focused edits, scaffolding files, updating docs/code. Don't use when: side effects are unclear or file ownership is uncertain.",
        ),
        (
            "memory_store",
            "Save to memory. Use when: preserving durable preferences, decisions, key context. Don't use when: information is transient/noisy/sensitive without need.",
        ),
        (
            "memory_recall",
            "Search memory. Use when: retrieving prior decisions, user preferences, historical context. Don't use when: answer is already in current context.",
        ),
        (
            "memory_forget",
            "Delete a memory entry. Use when: memory is incorrect/stale or explicitly requested for removal. Don't use when: impact is uncertain.",
        ),
    ];
    tool_descs.push((
        "cron_add",
        "Create a cron job. Supports schedule kinds: cron, at, every; and job types: shell or agent.",
    ));
    tool_descs.push((
        "cron_list",
        "List all cron jobs with schedule, status, and metadata.",
    ));
    tool_descs.push(("cron_remove", "Remove a cron job by job_id."));
    tool_descs.push((
        "cron_update",
        "Patch a cron job (schedule, enabled, command/prompt, model, delivery, session_target).",
    ));
    tool_descs.push((
        "cron_run",
        "Force-run a cron job immediately and record a run history entry.",
    ));
    tool_descs.push(("cron_runs", "Show recent run history for a cron job."));
    tool_descs.push((
        "screenshot",
        "Capture a screenshot of the current screen. Returns file path and base64-encoded PNG. Use when: visual verification, UI inspection, debugging displays.",
    ));
    tool_descs.push((
        "image_info",
        "Read image file metadata (format, dimensions, size) and optionally base64-encode it. Use when: inspecting images, preparing visual data for analysis.",
    ));
    if config.browser.enabled {
        tool_descs.push((
            "browser_open",
            "Open approved HTTPS URLs in system browser (allowlist-only, no scraping)",
        ));
    }
    if config.composio.enabled {
        tool_descs.push((
            "composio",
            "Execute actions on 1000+ apps via Composio (Gmail, Notion, GitHub, Slack, etc.). Use action='list' to discover, 'execute' to run (optionally with connected_account_id), 'connect' to OAuth.",
        ));
    }
    tool_descs.push((
        "schedule",
        "Manage scheduled tasks (create/list/get/cancel/pause/resume). Supports recurring cron and one-shot delays.",
    ));
    tool_descs.push((
        "model_routing_config",
        "Configure default model, scenario routing, and delegate agents. Use for natural-language requests like: 'set conversation to kimi and coding to gpt-5.3-codex'.",
    ));
    if !config.agents.is_empty() {
        tool_descs.push((
            "delegate",
            "Delegate a sub-task to a specialized agent. Use when: task needs different model/capability, or to parallelize work.",
        ));
    }
    if config.peripherals.enabled && !config.peripherals.boards.is_empty() {
        tool_descs.push((
            "gpio_read",
            "Read GPIO pin value (0 or 1) on connected hardware (STM32, Arduino). Use when: checking sensor/button state, LED status.",
        ));
        tool_descs.push((
            "gpio_write",
            "Set GPIO pin high (1) or low (0) on connected hardware. Use when: turning LED on/off, controlling actuators.",
        ));
        tool_descs.push((
            "arduino_upload",
            "Upload agent-generated Arduino sketch. Use when: user asks for 'make a heart', 'blink pattern', or custom LED behavior on Arduino. You write the full .ino code; Plaw compiles and uploads it. Pin 13 = built-in LED on Uno.",
        ));
        tool_descs.push((
            "hardware_memory_map",
            "Return flash and RAM address ranges for connected hardware. Use when: user asks for 'upper and lower memory addresses', 'memory map', or 'readable addresses'.",
        ));
        tool_descs.push((
            "hardware_board_info",
            "Return full board info (chip, architecture, memory map) for connected hardware. Use when: user asks for 'board info', 'what board do I have', 'connected hardware', 'chip info', or 'what hardware'.",
        ));
        tool_descs.push((
            "hardware_memory_read",
            "Read actual memory/register values from Nucleo via USB. Use when: user asks to 'read register values', 'read memory', 'dump lower memory 0-126', 'give address and value'. Params: address (hex, default 0x20000000), length (bytes, default 128).",
        ));
        tool_descs.push((
            "hardware_capabilities",
            "Query connected hardware for reported GPIO pins and LED pin. Use when: user asks what pins are available.",
        ));
    }
    let bootstrap_max_chars = if config.agent.compact_context {
        Some(6000)
    } else {
        None
    };
    let native_tools = provider.supports_native_tools();
    let mut system_prompt = crate::channels::build_system_prompt_with_mode(
        &config.workspace_dir,
        model_name,
        &tool_descs,
        &skills,
        Some(&config.identity),
        bootstrap_max_chars,
        native_tools,
        config.skills.prompt_injection_mode,
    );

    // Append structured tool-use instructions with schemas (only for non-native providers)
    if !native_tools {
        system_prompt.push_str(&build_tool_instructions(&tools_registry));
    }
    system_prompt.push_str(&build_shell_policy_instructions(&config.autonomy));

    // ── Approval manager (supervised mode) ───────────────────────
    let approval_manager = if interactive {
        Some(ApprovalManager::from_config(&config.autonomy))
    } else {
        None
    };
    let channel_name = if interactive { "cli" } else { "daemon" };

    // ── Execute ──────────────────────────────────────────────────
    let start = Instant::now();

    let mut final_output = String::new();

    if let Some(turn_id) = resume_from_turn_id.as_deref() {
        // ── Resume path: seed history from a snapshot for this turn
        // and continue the loop. The system prompt is already in the
        // snapshot; do not re-prepend it.
        //
        // `resume_from_iteration`:
        // - None → latest snapshot (crash-recovery)
        // - Some(N) → iteration N exactly (time-travel branch)
        //
        // `message` (when present) appends as a new user turn after
        // the loaded history so users can nudge the resumed agent.
        // Interactive resume in the REPL is still deferred.
        let snapshot = match resume_from_iteration {
            Some(iter) => crate::agent::checkpoint::load_snapshot_at_iteration(
                &config, turn_id, iter,
            )?,
            None => crate::agent::checkpoint::load_resume_snapshot(&config, turn_id)?,
        };
        let resumed_iter = snapshot.iteration;
        let mut history = snapshot.history;
        if let Some(msg) = &message {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z");
            history.push(ChatMessage::user(format!("[{now}] {msg}")));
        }
        let branch_label = if resume_from_iteration.is_some() {
            " (branch)"
        } else {
            ""
        };
        eprintln!(
            "▶ Resuming turn {turn_id} from iteration {resumed_iter}{branch_label} \
             ({} messages in history)",
            history.len()
        );

        // Scope cross-turn lineage so the loop's iteration-0 snapshot
        // can stamp parent_turn_id + branched_from_iteration. No-op for
        // non-checkpoint runs (writer returns None → lineage never read).
        let lineage = ResumeLineage {
            parent_turn_id: turn_id.to_string(),
            branched_from_iteration: resumed_iter,
        };
        let response = with_resume_lineage(
            Some(lineage),
            run_tool_call_loop(
                provider.as_ref(),
                &mut history,
                &tools_registry,
                observer.as_ref(),
                provider_name,
                model_name,
                temperature,
                false,
                approval_manager.as_ref(),
                channel_name,
                &config.multimodal,
                config.agent.max_tool_iterations,
                None,
                None,
                None,
                &[],
            ),
        )
        .await?;
        final_output = response.clone();
        println!("{response}");
        observer.record_event(&ObserverEvent::TurnComplete);
    } else if let Some(msg) = message {
        // Auto-save user message to memory (skip short/trivial messages)
        if config.memory.auto_save && msg.chars().count() >= AUTOSAVE_MIN_MESSAGE_CHARS {
            let user_key = autosave_memory_key("user_msg");
            let _ = mem
                .store(&user_key, &msg, MemoryCategory::Conversation, None)
                .await;
        }

        // Inject memory + hardware RAG context into user message
        let mem_context =
            build_context(mem.as_ref(), &msg, config.memory.min_relevance_score).await;
        let rag_limit = if config.agent.compact_context { 2 } else { 5 };
        let hw_context = hardware_rag
            .as_ref()
            .map(|r| build_hardware_context(r, &msg, &board_names, rag_limit))
            .unwrap_or_default();
        let context = format!("{mem_context}{hw_context}");
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z");
        let enriched = if context.is_empty() {
            format!("[{now}] {msg}")
        } else {
            format!("{context}[{now}] {msg}")
        };

        let mut history = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(&enriched),
        ];

        let response = run_tool_call_loop(
            provider.as_ref(),
            &mut history,
            &tools_registry,
            observer.as_ref(),
            provider_name,
            model_name,
            temperature,
            false,
            approval_manager.as_ref(),
            channel_name,
            &config.multimodal,
            config.agent.max_tool_iterations,
            None,
            None,
            None,
            &[],
        )
        .await?;
        final_output = response.clone();
        println!("{response}");
        observer.record_event(&ObserverEvent::TurnComplete);
    } else {
        println!("🦀 Plaw Interactive Mode");
        println!("Type /help for commands.\n");
        let cli = crate::channels::CliChannel::new();

        // Persistent conversation history across turns
        let mut history = vec![ChatMessage::system(&system_prompt)];
        // Reusable readline editor for UTF-8 input support
        let mut rl = rustyline::DefaultEditor::new()?;

        loop {
            let input = match rl.readline("> ") {
                Ok(line) => line,
                Err(ReadlineError::Interrupted | ReadlineError::Eof) => {
                    break;
                }
                Err(e) => {
                    eprintln!("\nError reading input: {e}\n");
                    break;
                }
            };

            let user_input = input.trim().to_string();
            if user_input.is_empty() {
                continue;
            }
            rl.add_history_entry(&input)?;
            match user_input.as_str() {
                "/quit" | "/exit" => break,
                "/help" => {
                    println!("Available commands:");
                    println!("  /help        Show this help message");
                    println!("  /clear /new  Clear conversation history");
                    println!("  /quit /exit  Exit interactive mode\n");
                    continue;
                }
                "/clear" | "/new" => {
                    println!(
                        "This will clear the current conversation and delete all session memory."
                    );
                    println!("Core memories (long-term facts/preferences) will be preserved.");
                    let confirm = rl.readline("Continue? [y/N] ").unwrap_or_default();

                    if !matches!(confirm.trim().to_lowercase().as_str(), "y" | "yes") {
                        println!("Cancelled.\n");
                        continue;
                    }

                    // Ensure prior prompts are not navigable after reset.
                    rl.clear_history()?;
                    history.clear();
                    history.push(ChatMessage::system(&system_prompt));
                    // Clear conversation and daily memory
                    let mut cleared = 0;
                    for category in [MemoryCategory::Conversation, MemoryCategory::Daily] {
                        let entries = mem.list(Some(&category), None).await.unwrap_or_default();
                        for entry in entries {
                            if mem.forget(&entry.key).await.unwrap_or(false) {
                                cleared += 1;
                            }
                        }
                    }
                    if cleared > 0 {
                        println!("Conversation cleared ({cleared} memory entries removed).\n");
                    } else {
                        println!("Conversation cleared.\n");
                    }
                    continue;
                }
                _ => {}
            }

            // Auto-save conversation turns (skip short/trivial messages)
            if config.memory.auto_save && user_input.chars().count() >= AUTOSAVE_MIN_MESSAGE_CHARS {
                let user_key = autosave_memory_key("user_msg");
                let _ = mem
                    .store(&user_key, &user_input, MemoryCategory::Conversation, None)
                    .await;
            }

            // Inject memory + hardware RAG context into user message
            let mem_context =
                build_context(mem.as_ref(), &user_input, config.memory.min_relevance_score).await;
            let rag_limit = if config.agent.compact_context { 2 } else { 5 };
            let hw_context = hardware_rag
                .as_ref()
                .map(|r| build_hardware_context(r, &user_input, &board_names, rag_limit))
                .unwrap_or_default();
            let context = format!("{mem_context}{hw_context}");
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z");
            let enriched = if context.is_empty() {
                format!("[{now}] {user_input}")
            } else {
                format!("{context}[{now}] {user_input}")
            };

            history.push(ChatMessage::user(&enriched));

            let response = match run_tool_call_loop(
                provider.as_ref(),
                &mut history,
                &tools_registry,
                observer.as_ref(),
                provider_name,
                model_name,
                temperature,
                false,
                approval_manager.as_ref(),
                channel_name,
                &config.multimodal,
                config.agent.max_tool_iterations,
                None,
                None,
                None,
                &[],
            )
            .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    if is_tool_iteration_limit_error(&e) {
                        let pause_notice = format!(
                            "⚠️ Reached tool-iteration limit ({}). Context and progress are preserved. \
                            Reply \"continue\" to resume, or increase `agent.max_tool_iterations` in config.",
                            config.agent.max_tool_iterations.max(DEFAULT_MAX_TOOL_ITERATIONS)
                        );
                        history.push(ChatMessage::assistant(&pause_notice));
                        eprintln!("\n{pause_notice}\n");
                        continue;
                    }
                    eprintln!("\nError: {e}\n");
                    continue;
                }
            };
            final_output = response.clone();
            if let Err(e) = crate::channels::Channel::send(
                &cli,
                &crate::channels::traits::SendMessage::new(format!("\n{response}\n"), "user"),
            )
            .await
            {
                eprintln!("\nError sending CLI response: {e}\n");
            }
            observer.record_event(&ObserverEvent::TurnComplete);

            // Auto-compaction before hard trimming to preserve long-context signal.
            if let Ok(compacted) = auto_compact_history(
                &mut history,
                provider.as_ref(),
                model_name,
                config.agent.max_history_messages,
                None, // CLI loop doesn't track input_tokens per-turn
                config.agent.max_context_tokens,
                None, // No capsule store in CLI mode
                None, // No session_id in CLI mode
                None, // No embedding provider in CLI mode
                false, // not forced
            )
            .await
            {
                if compacted {
                    println!("🧹 Auto-compaction complete");
                }
            }

            // Hard cap as a safety net.
            trim_history(&mut history, config.agent.max_history_messages);
        }
    }

    let duration = start.elapsed();
    observer.record_event(&ObserverEvent::AgentEnd {
        provider: provider_name.to_string(),
        model: model_name.to_string(),
        duration,
        tokens_used: None,
        cost_usd: None,
    });

    Ok(final_output)
}

/// Process a single message through the full agent (with tools, peripherals, memory).
/// Used by channels (Telegram, Discord, etc.) to enable hardware and tool use.
pub async fn process_message(config: Config, message: &str) -> Result<String> {
    let observer: Arc<dyn Observer> =
        Arc::from(observability::create_observer(&config.observability));
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(SecurityPolicy::from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));
    let mem: Arc<dyn Memory> = Arc::from(memory::create_memory_with_storage(
        &config.memory,
        Some(&config.storage.provider.config),
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);

    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };
    let mut tools_registry = tools::all_tools_with_runtime(
        Arc::new(config.clone()),
        &security,
        runtime,
        mem.clone(),
        composio_key,
        composio_entity_id,
        &config.browser,
        &config.http_request,
        &config.web_fetch,
        &config.workspace_dir,
        &config.agents,
        config.api_key.as_deref(),
        &config,
    );
    let peripheral_tools: Vec<Box<dyn Tool>> =
        crate::peripherals::create_peripheral_tools(&config.peripherals).await?;
    tools_registry.extend(peripheral_tools);

    let provider_name = config.default_provider.as_deref().unwrap_or("openrouter");
    let model_name = config
        .default_model
        .clone()
        .unwrap_or_else(|| "anthropic/claude-sonnet-4-20250514".into());
    let provider_runtime_options = providers::ProviderRuntimeOptions {
        auth_profile_override: None,
        provider_api_url: config.api_url.clone(),
        plaw_dir: config.config_path.parent().map(std::path::PathBuf::from),
        secrets_encrypt: config.secrets.encrypt,
        reasoning_enabled: config.runtime.reasoning_enabled,
        reasoning_level: config.effective_provider_reasoning_level(),
        custom_provider_api_mode: config.provider_api.map(|mode| mode.as_compatible_mode()),
        max_tokens_override: None,
        model_support_vision: config.model_support_vision,
    };
    let provider: Box<dyn Provider> = providers::create_routed_provider_with_options(
        provider_name,
        config.api_key.as_deref(),
        config.api_url.as_deref(),
        &config.reliability,
        &config.model_routes,
        &model_name,
        &provider_runtime_options,
    )?;

    let hardware_rag: Option<crate::rag::HardwareRag> = config
        .peripherals
        .datasheet_dir
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(|dir| crate::rag::HardwareRag::load(&config.workspace_dir, dir.trim()))
        .and_then(Result::ok)
        .filter(|r: &crate::rag::HardwareRag| !r.is_empty());
    let board_names: Vec<String> = config
        .peripherals
        .boards
        .iter()
        .map(|b| b.board.clone())
        .collect();

    let skills = crate::skills::load_skills_with_config(&config.workspace_dir, &config);
    let mut tool_descs: Vec<(&str, &str)> = vec![
        ("shell", "Execute terminal commands."),
        ("file_read", "Read file contents."),
        ("file_write", "Write file contents."),
        ("memory_store", "Save to memory."),
        ("memory_recall", "Search memory."),
        ("memory_forget", "Delete a memory entry."),
        (
            "model_routing_config",
            "Configure default model, scenario routing, and delegate agents.",
        ),
        ("screenshot", "Capture a screenshot."),
        ("image_info", "Read image metadata."),
    ];
    if config.browser.enabled {
        tool_descs.push(("browser_open", "Open approved URLs in browser."));
    }
    if config.composio.enabled {
        tool_descs.push(("composio", "Execute actions on 1000+ apps via Composio."));
    }
    if config.peripherals.enabled && !config.peripherals.boards.is_empty() {
        tool_descs.push(("gpio_read", "Read GPIO pin value on connected hardware."));
        tool_descs.push((
            "gpio_write",
            "Set GPIO pin high or low on connected hardware.",
        ));
        tool_descs.push((
            "arduino_upload",
            "Upload Arduino sketch. Use for 'make a heart', custom patterns. You write full .ino code; Plaw uploads it.",
        ));
        tool_descs.push((
            "hardware_memory_map",
            "Return flash and RAM address ranges. Use when user asks for memory addresses or memory map.",
        ));
        tool_descs.push((
            "hardware_board_info",
            "Return full board info (chip, architecture, memory map). Use when user asks for board info, what board, connected hardware, or chip info.",
        ));
        tool_descs.push((
            "hardware_memory_read",
            "Read actual memory/register values from Nucleo. Use when user asks to read registers, read memory, dump lower memory 0-126, or give address and value.",
        ));
        tool_descs.push((
            "hardware_capabilities",
            "Query connected hardware for reported GPIO pins and LED pin. Use when user asks what pins are available.",
        ));
    }
    let bootstrap_max_chars = if config.agent.compact_context {
        Some(6000)
    } else {
        None
    };
    let native_tools = provider.supports_native_tools();
    let mut system_prompt = crate::channels::build_system_prompt_with_mode(
        &config.workspace_dir,
        &model_name,
        &tool_descs,
        &skills,
        Some(&config.identity),
        bootstrap_max_chars,
        native_tools,
        config.skills.prompt_injection_mode,
    );
    if !native_tools {
        system_prompt.push_str(&build_tool_instructions(&tools_registry));
    }
    system_prompt.push_str(&build_shell_policy_instructions(&config.autonomy));

    let mem_context = build_context(mem.as_ref(), message, config.memory.min_relevance_score).await;
    let rag_limit = if config.agent.compact_context { 2 } else { 5 };
    let hw_context = hardware_rag
        .as_ref()
        .map(|r| build_hardware_context(r, message, &board_names, rag_limit))
        .unwrap_or_default();
    let context = format!("{mem_context}{hw_context}");
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z");
    let enriched = if context.is_empty() {
        format!("[{now}] {message}")
    } else {
        format!("{context}[{now}] {message}")
    };

    let mut history = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user(&enriched),
    ];

    agent_turn(
        provider.as_ref(),
        &mut history,
        &tools_registry,
        observer.as_ref(),
        provider_name,
        &model_name,
        config.default_temperature,
        true,
        &config.multimodal,
        config.agent.max_tool_iterations,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::budgets::DEFAULT_MAX_HISTORY_MESSAGES;
    use async_trait::async_trait;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn test_scrub_credentials() {
        let input = "API_KEY=sk-1234567890abcdef; token: 1234567890; password=\"secret123456\"";
        let scrubbed = scrub_credentials(input);
        assert!(scrubbed.contains("API_KEY=sk-1*[REDACTED]"));
        assert!(scrubbed.contains("token: 1234*[REDACTED]"));
        assert!(scrubbed.contains("password=\"secr*[REDACTED]\""));
        assert!(!scrubbed.contains("abcdef"));
        assert!(!scrubbed.contains("secret123456"));
    }

    #[test]
    fn test_scrub_credentials_json() {
        let input = r#"{"api_key": "sk-1234567890", "other": "public"}"#;
        let scrubbed = scrub_credentials(input);
        assert!(scrubbed.contains("\"api_key\": \"sk-1*[REDACTED]\""));
        assert!(scrubbed.contains("public"));
    }

    #[test]
    fn maybe_inject_cron_add_delivery_populates_agent_delivery_from_channel_context() {
        let mut args = serde_json::json!({
            "job_type": "agent",
            "prompt": "remind me later"
        });

        maybe_inject_cron_add_delivery("cron_add", &mut args, "telegram", Some("-10012345"));

        assert_eq!(args["delivery"]["mode"], "announce");
        assert_eq!(args["delivery"]["channel"], "telegram");
        assert_eq!(args["delivery"]["to"], "-10012345");
    }

    #[test]
    fn maybe_inject_cron_add_delivery_does_not_override_explicit_target() {
        let mut args = serde_json::json!({
            "job_type": "agent",
            "prompt": "remind me later",
            "delivery": {
                "mode": "announce",
                "channel": "discord",
                "to": "C123"
            }
        });

        maybe_inject_cron_add_delivery("cron_add", &mut args, "telegram", Some("-10012345"));

        assert_eq!(args["delivery"]["channel"], "discord");
        assert_eq!(args["delivery"]["to"], "C123");
    }

    #[test]
    fn maybe_inject_cron_add_delivery_skips_shell_jobs() {
        let mut args = serde_json::json!({
            "job_type": "shell",
            "command": "echo hello"
        });

        maybe_inject_cron_add_delivery("cron_add", &mut args, "telegram", Some("-10012345"));

        assert!(args.get("delivery").is_none());
    }

    use crate::memory::{Memory, MemoryCategory, SqliteMemory};
    use crate::observability::NoopObserver;
    use crate::providers::traits::ProviderCapabilities;
    use crate::providers::ChatResponse;
    use tempfile::TempDir;

    struct NonVisionProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Provider for NonVisionProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok("ok".to_string())
        }
    }

    struct VisionProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Provider for VisionProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                native_tool_calling: false,
                vision: true,
            }
        }

        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok("ok".to_string())
        }

        async fn chat(
            &self,
            request: ChatRequest<'_>,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<ChatResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let marker_count = crate::multimodal::count_image_markers(request.messages);
            if marker_count == 0 {
                anyhow::bail!("expected image markers in request messages");
            }

            if request.tools.is_some() {
                anyhow::bail!("no tools should be attached for this test");
            }

            Ok(ChatResponse {
                text: Some("vision-ok".to_string()),
                tool_calls: Vec::new(),
                usage: None,
                reasoning_content: None,
            })
        }
    }

    struct ScriptedProvider {
        responses: Arc<Mutex<VecDeque<ChatResponse>>>,
        capabilities: ProviderCapabilities,
    }

    impl ScriptedProvider {
        fn from_text_responses(responses: Vec<&str>) -> Self {
            let scripted = responses
                .into_iter()
                .map(|text| ChatResponse {
                    text: Some(text.to_string()),
                    tool_calls: Vec::new(),
                    usage: None,
                    reasoning_content: None,
                })
                .collect();
            Self {
                responses: Arc::new(Mutex::new(scripted)),
                capabilities: ProviderCapabilities::default(),
            }
        }

        fn with_native_tool_support(mut self) -> Self {
            self.capabilities.native_tool_calling = true;
            self
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            self.capabilities.clone()
        }

        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            anyhow::bail!("chat_with_system should not be used in scripted provider tests");
        }

        async fn chat(
            &self,
            _request: ChatRequest<'_>,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<ChatResponse> {
            let mut responses = self
                .responses
                .lock()
                .expect("responses lock should be valid");
            responses
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("scripted provider exhausted responses"))
        }
    }

    struct CountingTool {
        name: String,
        invocations: Arc<AtomicUsize>,
    }

    impl CountingTool {
        fn new(name: &str, invocations: Arc<AtomicUsize>) -> Self {
            Self {
                name: name.to_string(),
                invocations,
            }
        }
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Counts executions for loop-stability tests"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                }
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
        ) -> anyhow::Result<crate::tools::ToolResult> {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            let value = args
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Ok(crate::tools::ToolResult {
                success: true,
                output: format!("counted:{value}"),
                error: None,
            })
        }
    }

    struct DelayTool {
        name: String,
        delay_ms: u64,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    }

    impl DelayTool {
        fn new(
            name: &str,
            delay_ms: u64,
            active: Arc<AtomicUsize>,
            max_active: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                name: name.to_string(),
                delay_ms,
                active,
                max_active,
            }
        }
    }

    #[async_trait]
    impl Tool for DelayTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Delay tool for testing parallel tool execution"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            // Permissive schema — DelayTool is a generic mock registered under
            // various names ("shell", "delay_a", etc.) and tests pass diverse
            // arg shapes ({"command":...} for shell-impersonation tests,
            // {"value":...} for parallel-execution tests). Tool::execute_validated
            // would reject {"command":...} if we required "value" here.
            serde_json::json!({
                "type": "object",
                "additionalProperties": true
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
        ) -> anyhow::Result<crate::tools::ToolResult> {
            let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(now_active, Ordering::SeqCst);

            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;

            self.active.fetch_sub(1, Ordering::SeqCst);

            let value = args
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();

            Ok(crate::tools::ToolResult {
                success: true,
                output: format!("ok:{value}"),
                error: None,
            })
        }
    }

    #[tokio::test]
    async fn run_tool_call_loop_returns_structured_error_for_non_vision_provider() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = NonVisionProvider {
            calls: Arc::clone(&calls),
        };

        let mut history = vec![ChatMessage::user(
            "please inspect [IMAGE:data:image/png;base64,iVBORw0KGgo=]".to_string(),
        )];
        let tools_registry: Vec<Box<dyn Tool>> = Vec::new();
        let observer = NoopObserver;

        let err = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            None,
            "cli",
            &crate::config::MultimodalConfig::default(),
            3,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect_err("provider without vision support should fail");

        assert!(err.to_string().contains("provider_capability_error"));
        assert!(err.to_string().contains("capability=vision"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn run_tool_call_loop_rejects_oversized_image_payload() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = VisionProvider {
            calls: Arc::clone(&calls),
        };

        let oversized_payload = STANDARD.encode(vec![0_u8; (1024 * 1024) + 1]);
        let mut history = vec![ChatMessage::user(format!(
            "[IMAGE:data:image/png;base64,{oversized_payload}]"
        ))];

        let tools_registry: Vec<Box<dyn Tool>> = Vec::new();
        let observer = NoopObserver;
        let multimodal = crate::config::MultimodalConfig {
            max_images: 4,
            max_image_size_mb: 1,
            allow_remote_fetch: false,
        };

        let err = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            None,
            "cli",
            &multimodal,
            3,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect_err("oversized payload must fail");

        assert!(err
            .to_string()
            .contains("multimodal image size limit exceeded"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn run_tool_call_loop_accepts_valid_multimodal_request_flow() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = VisionProvider {
            calls: Arc::clone(&calls),
        };

        let mut history = vec![ChatMessage::user(
            "Analyze this [IMAGE:data:image/png;base64,iVBORw0KGgo=]".to_string(),
        )];
        let tools_registry: Vec<Box<dyn Tool>> = Vec::new();
        let observer = NoopObserver;

        let result = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            None,
            "cli",
            &crate::config::MultimodalConfig::default(),
            3,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect("valid multimodal payload should pass");

        assert_eq!(result, "vision-ok");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Test-only mock whose only purpose is to declare a chosen
    /// `SideEffectClass` for the parallel-gate tests below. Execute is
    /// unreachable — the gate inspects metadata, not behavior.
    struct SideEffectMockTool {
        name: String,
        side_effects: crate::tools::traits::SideEffectClass,
    }

    impl SideEffectMockTool {
        fn new(name: &str, side_effects: crate::tools::traits::SideEffectClass) -> Self {
            Self {
                name: name.to_string(),
                side_effects,
            }
        }
    }

    #[async_trait]
    impl Tool for SideEffectMockTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "side-effect-only test mock"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
        ) -> anyhow::Result<crate::tools::ToolResult> {
            unreachable!("side-effect mock is only inspected, never executed")
        }
        fn side_effects(&self) -> crate::tools::traits::SideEffectClass {
            self.side_effects
        }
    }

    #[test]
    fn should_execute_tools_in_parallel_returns_false_for_single_call() {
        let calls = vec![ParsedToolCall {
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "a.txt"}),
            tool_call_id: None,
        }];

        assert!(!should_execute_tools_in_parallel(&calls, &[], None));
    }

    #[test]
    fn should_execute_tools_in_parallel_returns_false_when_approval_is_required() {
        let calls = vec![
            ParsedToolCall {
                name: "shell".to_string(),
                arguments: serde_json::json!({"command": "pwd"}),
                tool_call_id: None,
            },
            ParsedToolCall {
                name: "http_request".to_string(),
                arguments: serde_json::json!({"url": "https://example.com"}),
                tool_call_id: None,
            },
        ];
        let approval_cfg = crate::config::AutonomyConfig::default();
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);

        assert!(!should_execute_tools_in_parallel(
            &calls,
            &[],
            Some(&approval_mgr)
        ));
    }

    #[test]
    fn should_execute_tools_in_parallel_returns_true_for_all_readonly_tools_with_approvals_waived() {
        // Two ReadOnly tools (e.g. file_read + glob_search) + every approval
        // waived via the "*" wildcard => the parallel gate opens.
        let calls = vec![
            ParsedToolCall {
                name: "file_read".to_string(),
                arguments: serde_json::json!({"path": "a.txt"}),
                tool_call_id: None,
            },
            ParsedToolCall {
                name: "glob_search".to_string(),
                arguments: serde_json::json!({"pattern": "*.rs"}),
                tool_call_id: None,
            },
        ];
        let registry: Vec<Box<dyn Tool>> = vec![
            Box::new(SideEffectMockTool::new(
                "file_read",
                crate::tools::traits::SideEffectClass::ReadOnly,
            )),
            Box::new(SideEffectMockTool::new(
                "glob_search",
                crate::tools::traits::SideEffectClass::ReadOnly,
            )),
        ];
        // Stage 6 collapse: "no interactive approvals" is opt-in via the
        // "*" wildcard in auto_approve, not via the tier.
        let approval_cfg = crate::config::AutonomyConfig {
            level: crate::security::AutonomyLevel::Full,
            auto_approve: vec!["*".into()],
            always_ask: vec![],
            ..crate::config::AutonomyConfig::default()
        };
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);

        assert!(should_execute_tools_in_parallel(
            &calls,
            &registry,
            Some(&approval_mgr)
        ));
    }

    #[test]
    fn should_execute_tools_in_parallel_returns_false_when_any_tool_is_not_readonly() {
        // Even with every approval waived, mixing a mutating tool
        // (shell = LocalExecute) into a parallel batch must force sequential
        // — otherwise two `shell` calls touching the same path would
        // interleave non-deterministically. The same applies to
        // LocalWrite / NetworkWrite / Spawn / Unknown.
        let calls = vec![
            ParsedToolCall {
                name: "shell".to_string(),
                arguments: serde_json::json!({"command": "pwd"}),
                tool_call_id: None,
            },
            ParsedToolCall {
                name: "http_request".to_string(),
                arguments: serde_json::json!({"url": "https://example.com"}),
                tool_call_id: None,
            },
        ];
        let registry: Vec<Box<dyn Tool>> = vec![
            Box::new(SideEffectMockTool::new(
                "shell",
                crate::tools::traits::SideEffectClass::LocalExecute,
            )),
            Box::new(SideEffectMockTool::new(
                "http_request",
                crate::tools::traits::SideEffectClass::NetworkWrite,
            )),
        ];
        let approval_cfg = crate::config::AutonomyConfig {
            level: crate::security::AutonomyLevel::Full,
            auto_approve: vec!["*".into()],
            always_ask: vec![],
            ..crate::config::AutonomyConfig::default()
        };
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);

        assert!(!should_execute_tools_in_parallel(
            &calls,
            &registry,
            Some(&approval_mgr)
        ));
    }

    #[test]
    fn should_execute_tools_in_parallel_returns_false_for_unknown_tool_name() {
        // Unknown tool name => conservatively non-ReadOnly => sequential.
        // The dispatcher will reject the unknown name shortly afterwards,
        // but the gate must not optimistically widen the parallel window.
        let calls = vec![
            ParsedToolCall {
                name: "file_read".to_string(),
                arguments: serde_json::json!({"path": "a.txt"}),
                tool_call_id: None,
            },
            ParsedToolCall {
                name: "made_up_tool".to_string(),
                arguments: serde_json::json!({}),
                tool_call_id: None,
            },
        ];
        let registry: Vec<Box<dyn Tool>> = vec![Box::new(SideEffectMockTool::new(
            "file_read",
            crate::tools::traits::SideEffectClass::ReadOnly,
        ))];
        let approval_cfg = crate::config::AutonomyConfig {
            auto_approve: vec!["*".into()],
            ..crate::config::AutonomyConfig::default()
        };
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);

        assert!(!should_execute_tools_in_parallel(
            &calls,
            &registry,
            Some(&approval_mgr)
        ));
    }

    #[tokio::test]
    async fn run_tool_call_loop_executes_multiple_tools_with_ordered_results() {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"delay_a","arguments":{"value":"A"}}
</tool_call>
<tool_call>
{"name":"delay_b","arguments":{"value":"B"}}
</tool_call>"#,
            "done",
        ]);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![
            Box::new(DelayTool::new(
                "delay_a",
                200,
                Arc::clone(&active),
                Arc::clone(&max_active),
            )),
            Box::new(DelayTool::new(
                "delay_b",
                200,
                Arc::clone(&active),
                Arc::clone(&max_active),
            )),
        ];

        // Stage 6 collapse: opt out of prompts via auto_approve = ["*"].
        let approval_cfg = crate::config::AutonomyConfig {
            level: crate::security::AutonomyLevel::Full,
            auto_approve: vec!["*".into()],
            always_ask: vec![],
            ..crate::config::AutonomyConfig::default()
        };
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run tool calls"),
        ];
        let observer = NoopObserver;

        let result = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            Some(&approval_mgr),
            "telegram",
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect("parallel execution should complete");

        assert_eq!(result, "done");
        assert!(
            max_active.load(Ordering::SeqCst) >= 1,
            "tools should execute successfully"
        );

        let tool_results_message = history
            .iter()
            .find(|msg| msg.role == "user" && msg.content.starts_with("[Tool results]"))
            .expect("tool results message should be present");
        let idx_a = tool_results_message
            .content
            .find("name=\"delay_a\"")
            .expect("delay_a result should be present");
        let idx_b = tool_results_message
            .content
            .find("name=\"delay_b\"")
            .expect("delay_b result should be present");
        assert!(
            idx_a < idx_b,
            "tool results should preserve input order for tool call mapping"
        );
    }

    #[tokio::test]
    async fn run_tool_call_loop_denies_supervised_tools_on_non_cli_channels() {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hi"}}
</tool_call>"#,
            "done",
        ]);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(DelayTool::new(
            "shell",
            50,
            Arc::clone(&active),
            Arc::clone(&max_active),
        ))];

        let approval_mgr = ApprovalManager::from_config(&crate::config::AutonomyConfig::default());

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run shell"),
        ];
        let observer = NoopObserver;

        let result = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            Some(&approval_mgr),
            "telegram",
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect("tool loop should complete with denied tool execution");

        assert_eq!(result, "done");
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            0,
            "shell tool must not execute when approval is unavailable on non-CLI channels"
        );
    }

    #[tokio::test]
    async fn run_tool_call_loop_waits_for_non_cli_approval_resolution() {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hi"}}
</tool_call>"#,
            "done",
        ]);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(DelayTool::new(
            "shell",
            50,
            Arc::clone(&active),
            Arc::clone(&max_active),
        ))];

        let approval_mgr = Arc::new(ApprovalManager::from_config(
            &crate::config::AutonomyConfig::default(),
        ));
        let (prompt_tx, mut prompt_rx) =
            tokio::sync::mpsc::unbounded_channel::<NonCliApprovalPrompt>();
        let approval_mgr_for_task = Arc::clone(&approval_mgr);
        let approval_task = tokio::spawn(async move {
            let prompt = prompt_rx
                .recv()
                .await
                .expect("approval prompt should arrive");
            approval_mgr_for_task
                .confirm_non_cli_pending_request(
                    &prompt.request_id,
                    "alice",
                    "telegram",
                    "chat-approval",
                )
                .expect("pending approval should confirm");
            approval_mgr_for_task
                .record_non_cli_pending_resolution(&prompt.request_id, ApprovalResponse::Yes);
        });

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run shell"),
        ];
        let observer = NoopObserver;

        let result = run_tool_call_loop_with_non_cli_approval_context(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            Some(approval_mgr.as_ref()),
            "telegram",
            Some(NonCliApprovalContext {
                sender: "alice".to_string(),
                reply_target: "chat-approval".to_string(),
                prompt_tx,
            }),
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
            None, // no checkpoint writer
            None, // no resume lineage
        )
        .await
        .expect("tool loop should continue after non-cli approval");

        approval_task.await.expect("approval task should complete");
        assert_eq!(result, "done");
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            1,
            "shell tool should execute after non-cli approval is resolved"
        );
    }

    #[tokio::test]
    async fn run_tool_call_loop_consumes_one_time_non_cli_allow_all_token() {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hi"}}
</tool_call>"#,
            "done",
        ]);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(DelayTool::new(
            "shell",
            50,
            Arc::clone(&active),
            Arc::clone(&max_active),
        ))];

        let approval_mgr = ApprovalManager::from_config(&crate::config::AutonomyConfig::default());
        approval_mgr.grant_non_cli_allow_all_once();
        assert_eq!(approval_mgr.non_cli_allow_all_once_remaining(), 1);

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run shell once"),
        ];
        let observer = NoopObserver;

        let result = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            Some(&approval_mgr),
            "telegram",
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect("tool loop should consume one-time allow-all token");

        assert_eq!(result, "done");
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            1,
            "shell tool should execute after consuming one-time allow-all token"
        );
        assert_eq!(approval_mgr.non_cli_allow_all_once_remaining(), 0);
    }

    #[tokio::test]
    async fn run_tool_call_loop_blocks_tools_excluded_for_channel() {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hi"}}
</tool_call>"#,
            "done",
        ]);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(DelayTool::new(
            "shell",
            50,
            Arc::clone(&active),
            Arc::clone(&max_active),
        ))];

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run shell"),
        ];
        let observer = NoopObserver;
        let excluded_tools = vec!["shell".to_string()];

        let result = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            None,
            "telegram",
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &excluded_tools,
        )
        .await
        .expect("tool loop should complete with blocked tool execution");

        assert_eq!(result, "done");
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            0,
            "excluded tool must not execute even if the model requests it"
        );

        let tool_results_message = history
            .iter()
            .find(|msg| msg.role == "user" && msg.content.starts_with("[Tool results]"))
            .expect("tool results message should be present");
        assert!(
            tool_results_message
                .content
                .contains("not available in this channel"),
            "blocked reason should be visible to the model"
        );
    }

    #[tokio::test]
    async fn run_tool_call_loop_deduplicates_repeated_tool_calls() {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>
<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>"#,
            "done",
        ]);

        let invocations = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(CountingTool::new(
            "count_tool",
            Arc::clone(&invocations),
        ))];

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run tool calls"),
        ];
        let observer = NoopObserver;

        let result = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            None,
            "cli",
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect("loop should finish after deduplicating repeated calls");

        assert_eq!(result, "done");
        assert_eq!(
            invocations.load(Ordering::SeqCst),
            1,
            "duplicate tool call with same args should not execute twice"
        );

        let tool_results = history
            .iter()
            .find(|msg| msg.role == "user" && msg.content.starts_with("[Tool results]"))
            .expect("prompt-mode tool result payload should be present");
        assert!(tool_results.content.contains("counted:A"));
        assert!(tool_results.content.contains("Skipped duplicate tool call"));
    }

    #[tokio::test]
    async fn run_tool_call_loop_native_mode_preserves_fallback_tool_call_ids() {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"{"content":"Need to call tool","tool_calls":[{"id":"call_abc","name":"count_tool","arguments":"{\"value\":\"X\"}"}]}"#,
            "done",
        ])
        .with_native_tool_support();

        let invocations = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(CountingTool::new(
            "count_tool",
            Arc::clone(&invocations),
        ))];

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run tool calls"),
        ];
        let observer = NoopObserver;

        let result = run_tool_call_loop(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            None,
            "cli",
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
        )
        .await
        .expect("native fallback id flow should complete");

        assert_eq!(result, "done");
        assert_eq!(invocations.load(Ordering::SeqCst), 1);
        assert!(
            history.iter().any(|msg| {
                msg.role == "tool" && msg.content.contains("\"tool_call_id\":\"call_abc\"")
            }),
            "tool result should preserve parsed fallback tool_call_id in native mode"
        );
        assert!(
            history
                .iter()
                .all(|msg| !(msg.role == "user" && msg.content.starts_with("[Tool results]"))),
            "native mode should use role=tool history instead of prompt fallback wrapper"
        );
    }

    #[test]
    fn parse_tool_calls_extracts_single_call() {
        let response = r#"Let me check that.
<tool_call>
{"name": "shell", "arguments": {"command": "ls -la"}}
</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(text, "Let me check that.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "ls -la"
        );
    }

    #[test]
    fn parse_tool_calls_extracts_multiple_calls() {
        let response = r#"<tool_call>
{"name": "file_read", "arguments": {"path": "a.txt"}}
</tool_call>
<tool_call>
{"name": "file_read", "arguments": {"path": "b.txt"}}
</tool_call>"#;

        let (_, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[1].name, "file_read");
    }

    #[test]
    fn parse_tool_calls_returns_text_only_when_no_calls() {
        let response = "Just a normal response with no tools.";
        let (text, calls) = parse_tool_calls(response);
        assert_eq!(text, "Just a normal response with no tools.");
        assert!(calls.is_empty());
    }

    #[test]
    fn parse_tool_calls_handles_malformed_json() {
        let response = r#"<tool_call>
not valid json
</tool_call>
Some text after."#;

        let (text, calls) = parse_tool_calls(response);
        assert!(calls.is_empty());
        assert!(text.contains("Some text after."));
    }

    #[test]
    fn parse_tool_calls_text_before_and_after() {
        let response = r#"Before text.
<tool_call>
{"name": "shell", "arguments": {"command": "echo hi"}}
</tool_call>
After text."#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.contains("Before text."));
        assert!(text.contains("After text."));
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn parse_tool_calls_handles_openai_format() {
        // OpenAI-style response with tool_calls array
        let response = r#"{"content": "Let me check that for you.", "tool_calls": [{"type": "function", "function": {"name": "shell", "arguments": "{\"command\": \"ls -la\"}"}}]}"#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(text, "Let me check that for you.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "ls -la"
        );
    }

    #[test]
    fn parse_tool_calls_handles_openai_format_multiple_calls() {
        let response = r#"{"tool_calls": [{"type": "function", "function": {"name": "file_read", "arguments": "{\"path\": \"a.txt\"}"}}, {"type": "function", "function": {"name": "file_read", "arguments": "{\"path\": \"b.txt\"}"}}]}"#;

        let (_, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[1].name, "file_read");
    }

    #[test]
    fn parse_tool_calls_openai_format_without_content() {
        // Some providers don't include content field with tool_calls
        let response = r#"{"tool_calls": [{"type": "function", "function": {"name": "memory_recall", "arguments": "{}"}}]}"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty()); // No content field
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "memory_recall");
    }

    #[test]
    fn parse_tool_calls_handles_openai_message_wrapper_with_content() {
        let response = r#"{
            "message": {
                "role": "assistant",
                "content": "<think>plan</think>\nI will call a tool.",
                "tool_calls": [
                    {
                        "id": "chatcmpl-tool-a18c01b8849eb05d",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"command\": \"ls -la\"}"
                        }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }"#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "ls -la"
        );
        assert!(text.contains("I will call a tool."));
    }

    #[test]
    fn parse_tool_calls_handles_openai_choices_message_wrapper() {
        let response = r#"{
            "id": "chatcmpl-123",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Checking now.",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "shell",
                                    "arguments": "{\"command\":\"pwd\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        }"#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(text, "Checking now.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "pwd"
        );
        assert_eq!(calls[0].tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn parse_tool_calls_preserves_openai_tool_call_ids() {
        let response = r#"{"tool_calls":[{"id":"call_42","function":{"name":"shell","arguments":"{\"command\":\"pwd\"}"}}]}"#;
        let (_, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_call_id.as_deref(), Some("call_42"));
    }

    #[test]
    fn parse_tool_calls_handles_markdown_json_inside_tool_call_tag() {
        let response = r#"<tool_call>
```json
{"name": "file_write", "arguments": {"path": "test.py", "content": "print('ok')"}}
```
</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "file_write");
        assert_eq!(
            calls[0].arguments.get("path").unwrap().as_str().unwrap(),
            "test.py"
        );
    }

    #[test]
    fn parse_tool_calls_handles_noisy_tool_call_tag_body() {
        let response = r#"<tool_call>
I will now call the tool with this payload:
{"name": "shell", "arguments": {"command": "pwd"}}
</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "pwd"
        );
    }

    #[test]
    fn parse_tool_calls_handles_tool_call_inline_attributes_with_send_message_alias() {
        let response = r#"<tool_call>send_message channel="user_channel" message="Hello! How can I assist you today?"</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "message_send");
        assert_eq!(
            calls[0].arguments.get("channel").unwrap().as_str().unwrap(),
            "user_channel"
        );
        assert_eq!(
            calls[0].arguments.get("message").unwrap().as_str().unwrap(),
            "Hello! How can I assist you today?"
        );
    }

    #[test]
    fn parse_tool_calls_handles_tool_call_function_style_arguments() {
        let response = r#"<tool_call>message_send(channel="general", message="test")</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "message_send");
        assert_eq!(
            calls[0].arguments.get("channel").unwrap().as_str().unwrap(),
            "general"
        );
        assert_eq!(
            calls[0].arguments.get("message").unwrap().as_str().unwrap(),
            "test"
        );
    }

    #[test]
    fn parse_tool_calls_handles_xml_nested_tool_payload() {
        let response = r#"<tool_call>
<memory_recall>
<query>project roadmap</query>
</memory_recall>
</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "memory_recall");
        assert_eq!(
            calls[0].arguments.get("query").unwrap().as_str().unwrap(),
            "project roadmap"
        );
    }

    #[test]
    fn parse_tool_calls_ignores_xml_thinking_wrapper() {
        let response = r#"<tool_call>
<thinking>Need to inspect memory first</thinking>
<memory_recall>
<query>recent deploy notes</query>
</memory_recall>
</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "memory_recall");
        assert_eq!(
            calls[0].arguments.get("query").unwrap().as_str().unwrap(),
            "recent deploy notes"
        );
    }

    #[test]
    fn parse_tool_calls_handles_xml_with_json_arguments() {
        let response = r#"<tool_call>
<shell>{"command":"pwd"}</shell>
</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "pwd"
        );
    }

    #[test]
    fn parse_tool_calls_handles_markdown_tool_call_fence() {
        let response = r#"I'll check that.
```tool_call
{"name": "shell", "arguments": {"command": "pwd"}}
```
Done."#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "pwd"
        );
        assert!(text.contains("I'll check that."));
        assert!(text.contains("Done."));
        assert!(!text.contains("```tool_call"));
    }

    #[test]
    fn parse_tool_calls_handles_markdown_tool_call_hybrid_close_tag() {
        let response = r#"Preface
```tool-call
{"name": "shell", "arguments": {"command": "date"}}
</tool_call>
Tail"#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "date"
        );
        assert!(text.contains("Preface"));
        assert!(text.contains("Tail"));
        assert!(!text.contains("```tool-call"));
    }

    #[test]
    fn parse_tool_calls_handles_markdown_invoke_fence() {
        let response = r#"Checking.
```invoke
{"name": "shell", "arguments": {"command": "date"}}
```
Done."#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "date"
        );
        assert!(text.contains("Checking."));
        assert!(text.contains("Done."));
    }

    #[test]
    fn parse_tool_calls_handles_tool_name_fence_format() {
        // Issue #1420: xAI grok models use ```tool <name> format
        let response = r#"I'll write a test file.
```tool file_write
{"path": "/home/user/test.txt", "content": "Hello world"}
```
Done."#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "file_write");
        assert_eq!(
            calls[0].arguments.get("path").unwrap().as_str().unwrap(),
            "/home/user/test.txt"
        );
        assert!(text.contains("I'll write a test file."));
        assert!(text.contains("Done."));
    }

    #[test]
    fn parse_tool_calls_handles_tool_name_fence_shell() {
        // Issue #1420: Test shell command in ```tool shell format
        let response = r#"```tool shell
{"command": "ls -la"}
```"#;

        let (_text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "ls -la"
        );
    }

    #[test]
    fn parse_tool_calls_handles_multiple_tool_name_fences() {
        // Multiple tool calls in ```tool <name> format
        let response = r#"First, I'll write a file.
```tool file_write
{"path": "/tmp/a.txt", "content": "A"}
```
Then read it.
```tool file_read
{"path": "/tmp/a.txt"}
```
Done."#;

        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "file_write");
        assert_eq!(calls[1].name, "file_read");
        assert!(text.contains("First, I'll write a file."));
        assert!(text.contains("Then read it."));
        assert!(text.contains("Done."));
    }

    #[test]
    fn parse_tool_calls_handles_toolcall_tag_alias() {
        let response = r#"<toolcall>
{"name": "shell", "arguments": {"command": "date"}}
</toolcall>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "date"
        );
    }

    #[test]
    fn parse_tool_calls_handles_tool_dash_call_tag_alias() {
        let response = r#"<tool-call>
{"name": "shell", "arguments": {"command": "whoami"}}
</tool-call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "whoami"
        );
    }

    #[test]
    fn parse_tool_calls_handles_invoke_tag_alias() {
        let response = r#"<invoke>
{"name": "shell", "arguments": {"command": "uptime"}}
</invoke>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "uptime"
        );
    }

    #[test]
    fn parse_tool_calls_handles_minimax_invoke_parameter_format() {
        let response = r#"<minimax:tool_call>
<invoke name="shell">
<parameter name="command">sqlite3 /tmp/test.db ".tables"</parameter>
</invoke>
</minimax:tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            r#"sqlite3 /tmp/test.db ".tables""#
        );
    }

    #[test]
    fn parse_tool_calls_handles_minimax_invoke_with_surrounding_text() {
        let response = r#"Preface
<minimax:tool_call>
<invoke name='http_request'>
<parameter name='url'>https://example.com</parameter>
<parameter name='method'>GET</parameter>
</invoke>
</minimax:tool_call>
Tail"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.contains("Preface"));
        assert!(text.contains("Tail"));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "http_request");
        assert_eq!(
            calls[0].arguments.get("url").unwrap().as_str().unwrap(),
            "https://example.com"
        );
        assert_eq!(
            calls[0].arguments.get("method").unwrap().as_str().unwrap(),
            "GET"
        );
    }

    #[test]
    fn parse_tool_calls_handles_minimax_toolcall_alias_and_cross_close_tag() {
        let response = r#"<tool_call>
{"name":"shell","arguments":{"command":"date"}}
</minimax:toolcall>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "date"
        );
    }

    #[test]
    fn parse_tool_calls_handles_perl_style_tool_call_blocks() {
        let response = r#"TOOL_CALL
{tool => "shell", args => { --command "uname -a" }}}
/TOOL_CALL"#;

        let calls = parse_perl_style_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "uname -a"
        );
    }

    #[test]
    fn parse_tool_calls_recovers_unclosed_tool_call_with_json() {
        let response = r#"I will call the tool now.
<tool_call>
{"name": "shell", "arguments": {"command": "uptime -p"}}"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.contains("I will call the tool now."));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "uptime -p"
        );
    }

    #[test]
    fn parse_tool_calls_recovers_mismatched_close_tag() {
        let response = r#"<tool_call>
{"name": "shell", "arguments": {"command": "uptime"}}
</arg_value>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0].arguments.get("command").unwrap().as_str().unwrap(),
            "uptime"
        );
    }

    #[test]
    fn parse_tool_calls_recovers_cross_alias_closing_tags() {
        let response = r#"<toolcall>
{"name": "shell", "arguments": {"command": "date"}}
</tool_call>"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.is_empty());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
    }

    #[test]
    fn parse_tool_calls_rejects_raw_tool_json_without_tags() {
        // SECURITY: Raw JSON without explicit wrappers should NOT be parsed
        // This prevents prompt injection attacks where malicious content
        // could include JSON that mimics a tool call.
        let response = r#"Sure, creating the file now.
{"name": "file_write", "arguments": {"path": "hello.py", "content": "print('hello')"}}"#;

        let (text, calls) = parse_tool_calls(response);
        assert!(text.contains("Sure, creating the file now."));
        assert_eq!(
            calls.len(),
            0,
            "Raw JSON without wrappers should not be parsed"
        );
    }

    #[test]
    fn trim_history_preserves_system_prompt() {
        let mut history = vec![ChatMessage::system("system prompt")];
        for i in 0..DEFAULT_MAX_HISTORY_MESSAGES + 20 {
            history.push(ChatMessage::user(format!("msg {i}")));
        }
        let original_len = history.len();
        assert!(original_len > DEFAULT_MAX_HISTORY_MESSAGES + 1);

        trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);

        // System prompt preserved
        assert_eq!(history[0].role, "system");
        assert_eq!(history[0].content, "system prompt");
        // Trimmed to limit
        assert_eq!(history.len(), DEFAULT_MAX_HISTORY_MESSAGES + 1); // +1 for system
                                                                     // Most recent messages preserved
        let last = &history[history.len() - 1];
        assert_eq!(
            last.content,
            format!("msg {}", DEFAULT_MAX_HISTORY_MESSAGES + 19)
        );
    }

    #[test]
    fn trim_history_noop_when_within_limit() {
        let mut history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
        ];
        trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn build_compaction_transcript_formats_roles() {
        let messages = vec![
            ChatMessage::user("I like dark mode"),
            ChatMessage::assistant("Got it"),
        ];
        let transcript = build_compaction_transcript(&messages);
        assert!(transcript.contains("USER: I like dark mode"));
        assert!(transcript.contains("ASSISTANT: Got it"));
    }

    #[test]
    fn apply_compaction_summary_replaces_old_segment() {
        let mut history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("old 1"),
            ChatMessage::assistant("old 2"),
            ChatMessage::user("recent 1"),
            ChatMessage::assistant("recent 2"),
        ];

        apply_compaction_summary(&mut history, 1, 3, "- user prefers concise replies");

        // After splice [sys, summary, recent1, recent2] (4 entries) the
        // sanitize_after_trim pass detects an assistant in slot 1 and
        // inserts a synthetic "请继续" user message before it (the
        // Anthropic API requires the first non-system message to be
        // user-role). So final length is 5: [sys, synthetic-user,
        // summary, recent1, recent2].
        assert_eq!(history.len(), 5);
        assert_eq!(history[0].role, "system");
        assert_eq!(history[1].role, "user");
        assert!(history[2].content.contains("Compaction summary"));
        assert!(history[3].content.contains("recent 1"));
        assert!(history[4].content.contains("recent 2"));
    }

    #[tokio::test]
    async fn autosave_memory_keys_preserve_multiple_turns() {
        let tmp = TempDir::new().unwrap();
        let mem = SqliteMemory::new(tmp.path()).unwrap();

        let key1 = autosave_memory_key("user_msg");
        let key2 = autosave_memory_key("user_msg");

        mem.store(&key1, "I'm Paul", MemoryCategory::Conversation, None)
            .await
            .unwrap();
        mem.store(&key2, "I'm 45", MemoryCategory::Conversation, None)
            .await
            .unwrap();

        assert_eq!(mem.count().await.unwrap(), 2);

        let recalled = mem.recall("45", 5, None).await.unwrap();
        assert!(recalled.iter().any(|entry| entry.content.contains("45")));
    }

    #[tokio::test]
    async fn build_context_ignores_legacy_assistant_autosave_entries() {
        let tmp = TempDir::new().unwrap();
        let mem = SqliteMemory::new(tmp.path()).unwrap();
        mem.store(
            "assistant_resp_poisoned",
            "User suffered a fabricated event",
            MemoryCategory::Daily,
            None,
        )
        .await
        .unwrap();
        mem.store(
            "user_msg_real",
            "User asked for concise status updates",
            MemoryCategory::Conversation,
            None,
        )
        .await
        .unwrap();

        let context = build_context(&mem, "status updates", 0.0).await;
        assert!(context.contains("user_msg_real"));
        assert!(!context.contains("assistant_resp_poisoned"));
        assert!(!context.contains("fabricated event"));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Recovery Tests - Tool Call Parsing Edge Cases
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn parse_tool_calls_handles_empty_tool_result() {
        // Recovery: Empty tool_result tag should be handled gracefully
        let response = r#"I'll run that command.
<tool_result name="shell">

</tool_result>
Done."#;
        let (text, calls) = parse_tool_calls(response);
        assert!(text.contains("Done."));
        assert!(calls.is_empty());
    }

    #[test]
    fn parse_arguments_value_handles_null() {
        // Recovery: null arguments are returned as-is (Value::Null)
        let value = serde_json::json!(null);
        let result = parse_arguments_value(Some(&value));
        assert!(result.is_null());
    }

    #[test]
    fn parse_tool_calls_handles_empty_tool_calls_array() {
        // Recovery: Empty tool_calls array returns original response (no tool parsing)
        let response = r#"{"content": "Hello", "tool_calls": []}"#;
        let (text, calls) = parse_tool_calls(response);
        // When tool_calls is empty, the entire JSON is returned as text
        assert!(text.contains("Hello"));
        assert!(calls.is_empty());
    }

    #[test]
    fn detect_tool_call_parse_issue_flags_malformed_payloads() {
        let response =
            "<tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"pwd\"}</tool_call>";
        let issue = detect_tool_call_parse_issue(response, &[]);
        assert!(
            issue.is_some(),
            "malformed tool payload should be flagged for diagnostics"
        );
    }

    #[test]
    fn detect_tool_call_parse_issue_ignores_normal_text() {
        let issue = detect_tool_call_parse_issue("Thanks, done.", &[]);
        assert!(issue.is_none());
    }

    #[test]
    fn parse_tool_calls_handles_whitespace_only_name() {
        // Recovery: Whitespace-only tool name should return None
        let value = serde_json::json!({"function": {"name": "   ", "arguments": {}}});
        let result = parse_tool_call_value(&value);
        assert!(result.is_none());
    }

    #[test]
    fn parse_tool_calls_handles_empty_string_arguments() {
        // Recovery: Empty string arguments should be handled
        let value = serde_json::json!({"name": "test", "arguments": ""});
        let result = parse_tool_call_value(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "test");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Recovery Tests - History Management
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn trim_history_with_no_system_prompt() {
        // Recovery: History without system prompt should trim correctly
        let mut history = vec![];
        for i in 0..DEFAULT_MAX_HISTORY_MESSAGES + 20 {
            history.push(ChatMessage::user(format!("msg {i}")));
        }
        trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
        assert_eq!(history.len(), DEFAULT_MAX_HISTORY_MESSAGES);
    }

    #[test]
    fn trim_history_preserves_role_ordering() {
        // Recovery: After trimming, role ordering should remain consistent
        let mut history = vec![ChatMessage::system("system")];
        for i in 0..DEFAULT_MAX_HISTORY_MESSAGES + 10 {
            history.push(ChatMessage::user(format!("user {i}")));
            history.push(ChatMessage::assistant(format!("assistant {i}")));
        }
        trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
        assert_eq!(history[0].role, "system");
        assert_eq!(history[history.len() - 1].role, "assistant");
    }

    #[test]
    fn trim_history_with_only_system_prompt() {
        // Recovery: Only system prompt should not be trimmed
        let mut history = vec![ChatMessage::system("system prompt")];
        trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
        assert_eq!(history.len(), 1);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Recovery Tests - Arguments Parsing
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn parse_arguments_value_handles_invalid_json_string() {
        // Recovery: Invalid JSON string should return empty object
        let value = serde_json::Value::String("not valid json".to_string());
        let result = parse_arguments_value(Some(&value));
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_arguments_value_handles_none() {
        // Recovery: None arguments should return empty object
        let result = parse_arguments_value(None);
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Recovery Tests - JSON Extraction
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn extract_json_values_handles_empty_string() {
        // Recovery: Empty input should return empty vec
        let result = extract_json_values("");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_json_values_handles_whitespace_only() {
        // Recovery: Whitespace only should return empty vec
        let result = extract_json_values("   \n\t  ");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_json_values_handles_multiple_objects() {
        // Recovery: Multiple JSON objects should all be extracted
        let input = r#"{"a": 1}{"b": 2}{"c": 3}"#;
        let result = extract_json_values(input);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn extract_json_values_handles_arrays() {
        // Recovery: JSON arrays should be extracted
        let input = r#"[1, 2, 3]{"key": "value"}"#;
        let result = extract_json_values(input);
        assert_eq!(result.len(), 2);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Recovery Tests - Constants Validation
    // ═══════════════════════════════════════════════════════════════════════

    const _: () = {
        assert!(DEFAULT_MAX_TOOL_ITERATIONS > 0);
        // Note: DEFAULT_MAX_TOOL_ITERATIONS was deliberately bumped to
        // usize::MAX (no built-in cap; runtime config controls bound).
        // The previous `<= 100` upper bound assertion is therefore no
        // longer applicable; drop it instead of tightening the constant.
        assert!(DEFAULT_MAX_HISTORY_MESSAGES > 0);
        assert!(DEFAULT_MAX_HISTORY_MESSAGES <= 1000);
    };

    #[test]
    fn constants_bounds_are_compile_time_checked() {
        // Bounds are enforced by the const assertions above.
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Recovery Tests - Tool Call Value Parsing
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn parse_tool_call_value_handles_missing_name_field() {
        // Recovery: Missing name field should return None
        let value = serde_json::json!({"function": {"arguments": {}}});
        let result = parse_tool_call_value(&value);
        assert!(result.is_none());
    }

    #[test]
    fn parse_tool_call_value_handles_top_level_name() {
        // Recovery: Tool call with name at top level (non-OpenAI format)
        let value = serde_json::json!({"name": "test_tool", "arguments": {}});
        let result = parse_tool_call_value(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "test_tool");
    }

    #[test]
    fn parse_tool_call_value_accepts_top_level_parameters_alias() {
        let value = serde_json::json!({
            "name": "schedule",
            "parameters": {"action": "create", "message": "test"}
        });
        let result = parse_tool_call_value(&value).expect("tool call should parse");
        assert_eq!(result.name, "schedule");
        assert_eq!(
            result.arguments.get("action").and_then(|v| v.as_str()),
            Some("create")
        );
    }

    #[test]
    fn parse_tool_call_value_accepts_function_parameters_alias() {
        let value = serde_json::json!({
            "function": {
                "name": "shell",
                "parameters": {"command": "date"}
            }
        });
        let result = parse_tool_call_value(&value).expect("tool call should parse");
        assert_eq!(result.name, "shell");
        assert_eq!(
            result.arguments.get("command").and_then(|v| v.as_str()),
            Some("date")
        );
    }

    #[test]
    fn parse_tool_call_value_recovers_shell_command_from_raw_string_arguments() {
        let value = serde_json::json!({
            "name": "shell",
            "arguments": "uname -a"
        });
        let result = parse_tool_call_value(&value).expect("tool call should parse");
        assert_eq!(result.name, "shell");
        assert_eq!(
            result.arguments.get("command").and_then(|v| v.as_str()),
            Some("uname -a")
        );
    }

    #[test]
    fn parse_tool_call_value_recovers_shell_command_from_cmd_alias() {
        let value = serde_json::json!({
            "function": {
                "name": "shell",
                "arguments": {"cmd": "pwd"}
            }
        });
        let result = parse_tool_call_value(&value).expect("tool call should parse");
        assert_eq!(result.name, "shell");
        assert_eq!(
            result.arguments.get("command").and_then(|v| v.as_str()),
            Some("pwd")
        );
    }

    #[test]
    fn parse_tool_call_value_preserves_tool_call_id_aliases() {
        let value = serde_json::json!({
            "call_id": "legacy_1",
            "function": {
                "name": "shell",
                "arguments": {"command": "date"}
            }
        });
        let result = parse_tool_call_value(&value).expect("tool call should parse");
        assert_eq!(result.tool_call_id.as_deref(), Some("legacy_1"));
    }

    #[test]
    fn parse_tool_calls_from_json_value_handles_empty_array() {
        // Recovery: Empty tool_calls array should return empty vec
        let value = serde_json::json!({"tool_calls": []});
        let result = parse_tool_calls_from_json_value(&value);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_tool_calls_from_json_value_handles_missing_tool_calls() {
        // Recovery: Missing tool_calls field should fall through
        let value = serde_json::json!({"name": "test", "arguments": {}});
        let result = parse_tool_calls_from_json_value(&value);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn parse_tool_calls_from_json_value_handles_top_level_array() {
        // Recovery: Top-level array of tool calls
        let value = serde_json::json!([
            {"name": "tool_a", "arguments": {}},
            {"name": "tool_b", "arguments": {}}
        ]);
        let result = parse_tool_calls_from_json_value(&value);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn parse_structured_tool_calls_recovers_shell_command_from_string_payload() {
        let calls = vec![ToolCall {
            id: "call_1".to_string(),
            name: "shell".to_string(),
            arguments: "ls -la".to_string(),
        }];
        let parsed = parse_structured_tool_calls(&calls);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "shell");
        assert_eq!(
            parsed[0].arguments.get("command").and_then(|v| v.as_str()),
            Some("ls -la")
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // GLM-Style Tool Call Parsing
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn parse_glm_style_browser_open_url() {
        let response = "browser_open/url>https://example.com";
        let calls = parse_glm_style_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "shell");
        assert!(calls[0].1["command"].as_str().unwrap().contains("curl"));
        assert!(calls[0].1["command"]
            .as_str()
            .unwrap()
            .contains("example.com"));
    }

    #[test]
    fn parse_glm_style_shell_command() {
        let response = "shell/command>ls -la";
        let calls = parse_glm_style_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "shell");
        assert_eq!(calls[0].1["command"], "ls -la");
    }

    #[test]
    fn parse_glm_style_http_request() {
        let response = "http_request/url>https://api.example.com/data";
        let calls = parse_glm_style_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "http_request");
        assert_eq!(calls[0].1["url"], "https://api.example.com/data");
        assert_eq!(calls[0].1["method"], "GET");
    }

    #[test]
    fn parse_glm_style_plain_url() {
        let response = "https://example.com/api";
        let calls = parse_glm_style_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "shell");
        assert!(calls[0].1["command"].as_str().unwrap().contains("curl"));
    }

    #[test]
    fn parse_glm_style_json_args() {
        let response = r#"shell/{"command": "echo hello"}"#;
        let calls = parse_glm_style_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "shell");
        assert_eq!(calls[0].1["command"], "echo hello");
    }

    #[test]
    fn parse_glm_style_multiple_calls() {
        let response = r#"shell/command>ls
browser_open/url>https://example.com"#;
        let calls = parse_glm_style_tool_calls(response);
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn parse_glm_style_tool_call_integration() {
        // Integration test: GLM format should be parsed in parse_tool_calls
        let response = "Checking...\nbrowser_open/url>https://example.com\nDone";
        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert!(text.contains("Checking"));
        assert!(text.contains("Done"));
    }

    #[test]
    fn parse_glm_style_rejects_non_http_url_param() {
        let response = "browser_open/url>javascript:alert(1)";
        let calls = parse_glm_style_tool_calls(response);
        assert!(calls.is_empty());
    }

    #[test]
    fn parse_tool_calls_handles_unclosed_tool_call_tag() {
        let response = "<tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"pwd\"}}\nDone";
        let (text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "pwd");
        assert_eq!(text, "Done");
    }

    // ─────────────────────────────────────────────────────────────────────
    // TG4 (inline): parse_tool_calls robustness — malformed/edge-case inputs
    // Prevents: Pattern 4 issues #746, #418, #777, #848
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn parse_tool_calls_empty_input_returns_empty() {
        let (text, calls) = parse_tool_calls("");
        assert!(calls.is_empty(), "empty input should produce no tool calls");
        assert!(text.is_empty(), "empty input should produce no text");
    }

    #[test]
    fn parse_tool_calls_whitespace_only_returns_empty_calls() {
        let (text, calls) = parse_tool_calls("   \n\t  ");
        assert!(calls.is_empty());
        assert!(text.is_empty() || text.trim().is_empty());
    }

    #[test]
    fn parse_tool_calls_nested_xml_tags_handled() {
        // Double-wrapped tool call should still parse the inner call
        let response = r#"<tool_call><tool_call>{"name":"echo","arguments":{"msg":"hi"}}</tool_call></tool_call>"#;
        let (_text, calls) = parse_tool_calls(response);
        // Should find at least one tool call
        assert!(
            !calls.is_empty(),
            "nested XML tags should still yield at least one tool call"
        );
    }

    #[test]
    fn parse_tool_calls_truncated_json_no_panic() {
        // Incomplete JSON inside tool_call tags
        let response = r#"<tool_call>{"name":"shell","arguments":{"command":"ls"</tool_call>"#;
        let (_text, _calls) = parse_tool_calls(response);
        // Should not panic — graceful handling of truncated JSON
    }

    #[test]
    fn parse_tool_calls_empty_json_object_in_tag() {
        let response = "<tool_call>{}</tool_call>";
        let (_text, calls) = parse_tool_calls(response);
        // Empty JSON object has no name field — should not produce valid tool call
        assert!(
            calls.is_empty(),
            "empty JSON object should not produce a tool call"
        );
    }

    #[test]
    fn parse_tool_calls_closing_tag_only_returns_text() {
        let response = "Some text </tool_call> more text";
        let (text, calls) = parse_tool_calls(response);
        assert!(
            calls.is_empty(),
            "closing tag only should not produce calls"
        );
        assert!(
            !text.is_empty(),
            "text around orphaned closing tag should be preserved"
        );
    }

    #[test]
    fn parse_tool_calls_very_large_arguments_no_panic() {
        let large_arg = "x".repeat(100_000);
        let response = format!(
            r#"<tool_call>{{"name":"echo","arguments":{{"message":"{}"}}}}</tool_call>"#,
            large_arg
        );
        let (_text, calls) = parse_tool_calls(&response);
        assert_eq!(calls.len(), 1, "large arguments should still parse");
        assert_eq!(calls[0].name, "echo");
    }

    #[test]
    fn parse_tool_calls_special_characters_in_arguments() {
        let response = r#"<tool_call>{"name":"echo","arguments":{"message":"hello \"world\" <>&'\n\t"}}</tool_call>"#;
        let (_text, calls) = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
    }

    #[test]
    fn parse_tool_calls_text_with_embedded_json_not_extracted() {
        // Raw JSON without any tags should NOT be extracted as a tool call
        let response = r#"Here is some data: {"name":"echo","arguments":{"message":"hi"}} end."#;
        let (_text, calls) = parse_tool_calls(response);
        assert!(
            calls.is_empty(),
            "raw JSON in text without tags should not be extracted"
        );
    }

    #[test]
    fn parse_tool_calls_multiple_formats_mixed() {
        // Mix of text and properly tagged tool call
        let response = r#"I'll help you with that.

<tool_call>
{"name":"shell","arguments":{"command":"echo hello"}}
</tool_call>

Let me check the result."#;
        let (text, calls) = parse_tool_calls(response);
        assert_eq!(
            calls.len(),
            1,
            "should extract one tool call from mixed content"
        );
        assert_eq!(calls[0].name, "shell");
        assert!(
            text.contains("help you"),
            "text before tool call should be preserved"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // TG4 (inline): trim_history edge cases
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn trim_history_empty_history() {
        let mut history: Vec<crate::providers::ChatMessage> = vec![];
        trim_history(&mut history, 10);
        assert!(history.is_empty());
    }

    #[test]
    fn trim_history_system_only() {
        let mut history = vec![crate::providers::ChatMessage::system("system prompt")];
        trim_history(&mut history, 10);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "system");
    }

    #[test]
    fn trim_history_exactly_at_limit() {
        let mut history = vec![
            crate::providers::ChatMessage::system("system"),
            crate::providers::ChatMessage::user("msg 1"),
            crate::providers::ChatMessage::assistant("reply 1"),
        ];
        trim_history(&mut history, 2); // 2 non-system messages = exactly at limit
        assert_eq!(history.len(), 3, "should not trim when exactly at limit");
    }

    #[test]
    fn trim_history_removes_oldest_non_system() {
        let mut history = vec![
            crate::providers::ChatMessage::system("system"),
            crate::providers::ChatMessage::user("old msg"),
            crate::providers::ChatMessage::assistant("old reply"),
            crate::providers::ChatMessage::user("new msg"),
            crate::providers::ChatMessage::assistant("new reply"),
        ];
        trim_history(&mut history, 2);
        assert_eq!(history.len(), 3); // system + 2 kept
        assert_eq!(history[0].role, "system");
        assert_eq!(history[1].content, "new msg");
    }

    /// When `build_system_prompt_with_mode` is called with `native_tools = true`,
    /// the output must contain ZERO XML protocol artifacts. In the native path
    /// `build_tool_instructions` is never called, so the system prompt alone
    /// must be clean of XML tool-call protocol.
    #[test]
    fn native_tools_system_prompt_contains_zero_xml() {
        use crate::channels::build_system_prompt_with_mode;

        let tool_summaries: Vec<(&str, &str)> = vec![
            ("shell", "Execute shell commands"),
            ("file_read", "Read files"),
        ];

        let system_prompt = build_system_prompt_with_mode(
            std::path::Path::new("/tmp"),
            "test-model",
            &tool_summaries,
            &[],  // no skills
            None, // no identity config
            None, // no bootstrap_max_chars
            true, // native_tools
            crate::config::SkillsPromptInjectionMode::Full,
        );

        // Must contain zero XML protocol artifacts
        assert!(
            !system_prompt.contains("<tool_call>"),
            "Native prompt must not contain <tool_call>"
        );
        assert!(
            !system_prompt.contains("</tool_call>"),
            "Native prompt must not contain </tool_call>"
        );
        assert!(
            !system_prompt.contains("<tool_result>"),
            "Native prompt must not contain <tool_result>"
        );
        assert!(
            !system_prompt.contains("</tool_result>"),
            "Native prompt must not contain </tool_result>"
        );
        assert!(
            !system_prompt.contains("## Tool Use Protocol"),
            "Native prompt must not contain XML protocol header"
        );

        // Positive: native prompt should still list tools and contain task instructions
        assert!(
            system_prompt.contains("shell"),
            "Native prompt must list tool names"
        );
        assert!(
            system_prompt.contains("## Your Task"),
            "Native prompt should contain task instructions"
        );
    }

    // ── Cross-Alias & GLM Shortened Body Tests ──────────────────────────

    #[test]
    fn parse_tool_calls_cross_alias_close_tag_with_json() {
        // <tool_call> opened but closed with </invoke> — JSON body
        let input = r#"<tool_call>{"name": "shell", "arguments": {"command": "ls"}}</invoke>"#;
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "ls");
        assert!(text.is_empty());
    }

    #[test]
    fn parse_tool_calls_cross_alias_close_tag_with_glm_shortened() {
        // <tool_call>shell>uname -a</invoke> — GLM shortened inside cross-alias tags
        let input = "<tool_call>shell>uname -a</invoke>";
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "uname -a");
        assert!(text.is_empty());
    }

    #[test]
    fn parse_tool_calls_glm_shortened_body_in_matched_tags() {
        // <tool_call>shell>pwd</tool_call> — GLM shortened in matched tags
        let input = "<tool_call>shell>pwd</tool_call>";
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "pwd");
        assert!(text.is_empty());
    }

    #[test]
    fn parse_tool_calls_glm_yaml_style_in_tags() {
        // <tool_call>shell>\ncommand: date\napproved: true</invoke>
        let input = "<tool_call>shell>\ncommand: date\napproved: true</invoke>";
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "date");
        assert_eq!(calls[0].arguments["approved"], true);
        assert!(text.is_empty());
    }

    #[test]
    fn parse_tool_calls_attribute_style_in_tags() {
        // <tool_call>shell command="date" /></tool_call>
        let input = r#"<tool_call>shell command="date" /></tool_call>"#;
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "date");
        assert!(text.is_empty());
    }

    #[test]
    fn parse_tool_calls_file_read_shortened_in_cross_alias() {
        // <tool_call>file_read path=".env" /></invoke>
        let input = r#"<tool_call>file_read path=".env" /></invoke>"#;
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[0].arguments["path"], ".env");
        assert!(text.is_empty());
    }

    #[test]
    fn parse_tool_calls_unclosed_glm_shortened_no_close_tag() {
        // <tool_call>shell>ls -la (no close tag at all)
        let input = "<tool_call>shell>ls -la";
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "ls -la");
        assert!(text.is_empty());
    }

    #[test]
    fn parse_tool_calls_text_before_cross_alias() {
        // Text before and after cross-alias tool call
        let input = "Let me check that.\n<tool_call>shell>uname -a</invoke>\nDone.";
        let (text, calls) = parse_tool_calls(input);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "uname -a");
        assert!(text.contains("Let me check that."));
        assert!(text.contains("Done."));
    }

    #[test]
    fn parse_glm_shortened_body_url_to_curl() {
        // URL values for shell should be wrapped in curl
        let call = parse_glm_shortened_body("shell>https://example.com/api").unwrap();
        assert_eq!(call.name, "shell");
        let cmd = call.arguments["command"].as_str().unwrap();
        assert!(cmd.contains("curl"));
        assert!(cmd.contains("example.com"));
    }

    #[test]
    fn parse_glm_shortened_body_browser_open_maps_to_shell_command() {
        // browser_open aliases to shell, and shortened calls must still emit
        // shell's canonical "command" argument.
        let call = parse_glm_shortened_body("browser_open>https://example.com").unwrap();
        assert_eq!(call.name, "shell");
        let cmd = call.arguments["command"].as_str().unwrap();
        assert!(cmd.contains("curl"));
        assert!(cmd.contains("example.com"));
    }

    #[test]
    fn parse_glm_shortened_body_memory_recall() {
        // memory_recall>some query — default param is "query"
        let call = parse_glm_shortened_body("memory_recall>recent meetings").unwrap();
        assert_eq!(call.name, "memory_recall");
        assert_eq!(call.arguments["query"], "recent meetings");
    }

    #[test]
    fn parse_glm_shortened_body_function_style_alias_maps_to_message_send() {
        let call =
            parse_glm_shortened_body(r#"sendmessage(channel="alerts", message="hi")"#).unwrap();
        assert_eq!(call.name, "message_send");
        assert_eq!(call.arguments["channel"], "alerts");
        assert_eq!(call.arguments["message"], "hi");
    }

    #[test]
    fn map_tool_name_alias_direct_coverage() {
        assert_eq!(map_tool_name_alias("bash"), "shell");
        assert_eq!(map_tool_name_alias("filelist"), "file_list");
        assert_eq!(map_tool_name_alias("memorystore"), "memory_store");
        assert_eq!(map_tool_name_alias("memoryforget"), "memory_forget");
        assert_eq!(map_tool_name_alias("http"), "http_request");
        assert_eq!(
            map_tool_name_alias("totally_unknown_tool"),
            "totally_unknown_tool"
        );
    }

    #[test]
    fn default_param_for_tool_coverage() {
        assert_eq!(default_param_for_tool("shell"), "command");
        assert_eq!(default_param_for_tool("bash"), "command");
        assert_eq!(default_param_for_tool("file_read"), "path");
        assert_eq!(default_param_for_tool("memory_recall"), "query");
        assert_eq!(default_param_for_tool("memory_store"), "content");
        assert_eq!(default_param_for_tool("http_request"), "url");
        assert_eq!(default_param_for_tool("browser_open"), "url");
        assert_eq!(default_param_for_tool("unknown_tool"), "input");
    }

    #[test]
    fn parse_glm_shortened_body_rejects_empty() {
        assert!(parse_glm_shortened_body("").is_none());
        assert!(parse_glm_shortened_body("   ").is_none());
    }

    #[test]
    fn parse_glm_shortened_body_rejects_invalid_tool_name() {
        // Tool names with special characters should be rejected
        assert!(parse_glm_shortened_body("not-a-tool>value").is_none());
        assert!(parse_glm_shortened_body("tool name>value").is_none());
    }

    // ═══════════════════════════════════════════════════════════════════════
    // reasoning_content pass-through tests for history builders
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn build_native_assistant_history_includes_reasoning_content() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
        }];
        let result = build_native_assistant_history("answer", &calls, Some("thinking step"));
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["content"].as_str(), Some("answer"));
        assert_eq!(parsed["reasoning_content"].as_str(), Some("thinking step"));
        assert!(parsed["tool_calls"].is_array());
    }

    #[test]
    fn build_native_assistant_history_omits_reasoning_content_when_none() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
        }];
        let result = build_native_assistant_history("answer", &calls, None);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["content"].as_str(), Some("answer"));
        assert!(parsed.get("reasoning_content").is_none());
    }

    #[test]
    fn build_native_assistant_history_from_parsed_calls_includes_reasoning_content() {
        let calls = vec![ParsedToolCall {
            name: "shell".into(),
            arguments: serde_json::json!({"command": "pwd"}),
            tool_call_id: Some("call_2".into()),
        }];
        let result = build_native_assistant_history_from_parsed_calls(
            "answer",
            &calls,
            Some("deep thought"),
        );
        assert!(result.is_some());
        let parsed: serde_json::Value = serde_json::from_str(result.as_deref().unwrap()).unwrap();
        assert_eq!(parsed["content"].as_str(), Some("answer"));
        assert_eq!(parsed["reasoning_content"].as_str(), Some("deep thought"));
        assert!(parsed["tool_calls"].is_array());
    }

    #[test]
    fn build_native_assistant_history_from_parsed_calls_omits_reasoning_content_when_none() {
        let calls = vec![ParsedToolCall {
            name: "shell".into(),
            arguments: serde_json::json!({"command": "pwd"}),
            tool_call_id: Some("call_2".into()),
        }];
        let result = build_native_assistant_history_from_parsed_calls("answer", &calls, None);
        assert!(result.is_some());
        let parsed: serde_json::Value = serde_json::from_str(result.as_deref().unwrap()).unwrap();
        assert_eq!(parsed["content"].as_str(), Some("answer"));
        assert!(parsed.get("reasoning_content").is_none());
    }

    #[tokio::test]
    async fn run_tool_call_loop_writes_checkpoint_when_writer_is_scoped() {
        use crate::agent::checkpoint::{FsCheckpointWriter, Snapshot};
        use std::sync::Arc as StdArc;
        use tempfile::TempDir;

        // 1st response: tool call → iteration runs to completion → snapshot.
        // 2nd response: plain text → loop exits before snapshot point.
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hi"}}
</tool_call>"#,
            "done",
        ]);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(DelayTool::new(
            "shell",
            10,
            Arc::clone(&active),
            Arc::clone(&max_active),
        ))];

        // Auto-approve so the supervised "shell" tool runs without prompting.
        let approval_cfg = crate::config::AutonomyConfig {
            level: crate::security::AutonomyLevel::Full,
            auto_approve: vec!["*".into()],
            always_ask: vec![],
            ..crate::config::AutonomyConfig::default()
        };
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);

        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run shell"),
        ];
        let observer = NoopObserver;

        let tmp = TempDir::new().unwrap();
        let writer: StdArc<dyn crate::agent::checkpoint::CheckpointWriter> =
            StdArc::new(FsCheckpointWriter::new(tmp.path()));

        let result = run_tool_call_loop_with_non_cli_approval_context(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            Some(&approval_mgr),
            "telegram",
            None,
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
            Some(writer),
            None, // no resume lineage for this test
        )
        .await
        .expect("tool loop should complete");

        assert_eq!(result, "done");

        // Inventory: exactly one snapshot — iteration 0's tool result push.
        // Iteration 1 returned text-only and exited before the snapshot
        // point, so no second file.
        let turn_dirs: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .collect();
        assert_eq!(turn_dirs.len(), 1, "exactly one turn directory expected");
        let snapshot_files: Vec<_> = std::fs::read_dir(turn_dirs[0].path())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(snapshot_files.len(), 1, "exactly one snapshot file expected");
        let snap_path = snapshot_files[0].path();
        assert_eq!(snap_path.file_name().unwrap(), "000000.json");

        let body = std::fs::read_to_string(&snap_path).unwrap();
        let snapshot: Snapshot = serde_json::from_str(&body).unwrap();
        assert_eq!(snapshot.iteration, 0);
        assert_eq!(snapshot.parent_iteration, None);
        // History at snapshot time includes the iteration's pushes; the
        // exact count varies by tool dispatcher mode, so assert lower-bound.
        assert!(
            snapshot.history.len() >= 4,
            "snapshot history should include the iteration's pushes, got {}",
            snapshot.history.len()
        );
        assert_eq!(snapshot.history[0].role, "system");
        assert_eq!(snapshot.history[1].role, "user");
    }

    #[tokio::test]
    async fn run_tool_call_loop_does_not_write_checkpoints_without_writer_in_scope() {
        // Same scenario as above but no writer scoped — verifies the
        // default path emits zero side effects.
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hi"}}
</tool_call>"#,
            "done",
        ]);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(DelayTool::new(
            "shell",
            10,
            Arc::clone(&active),
            Arc::clone(&max_active),
        ))];
        let approval_cfg = crate::config::AutonomyConfig {
            level: crate::security::AutonomyLevel::Full,
            auto_approve: vec!["*".into()],
            always_ask: vec![],
            ..crate::config::AutonomyConfig::default()
        };
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);
        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run shell"),
        ];
        let observer = NoopObserver;

        let result = run_tool_call_loop_with_non_cli_approval_context(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            Some(&approval_mgr),
            "telegram",
            None,
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
            None, // no checkpoint writer
            None, // no resume lineage
        )
        .await
        .expect("tool loop should complete");
        assert_eq!(result, "done");
    }

    #[tokio::test]
    async fn run_tool_call_loop_stamps_resume_lineage_on_iteration_zero_snapshot() {
        use crate::agent::checkpoint::{FsCheckpointWriter, Snapshot};
        use std::sync::Arc as StdArc;
        use tempfile::TempDir;

        // 1st response: tool call → iteration 0 snapshot.
        // 2nd response: text-only → loop exits.
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hi"}}
</tool_call>"#,
            "done",
        ]);
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(DelayTool::new(
            "shell",
            10,
            Arc::clone(&active),
            Arc::clone(&max_active),
        ))];
        let approval_cfg = crate::config::AutonomyConfig {
            level: crate::security::AutonomyLevel::Full,
            auto_approve: vec!["*".into()],
            always_ask: vec![],
            ..crate::config::AutonomyConfig::default()
        };
        let approval_mgr = ApprovalManager::from_config(&approval_cfg);
        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("run shell"),
        ];
        let observer = NoopObserver;

        let tmp = TempDir::new().unwrap();
        let writer: StdArc<dyn crate::agent::checkpoint::CheckpointWriter> =
            StdArc::new(FsCheckpointWriter::new(tmp.path()));

        let lineage = ResumeLineage {
            parent_turn_id: "parent-abc".into(),
            branched_from_iteration: 5,
        };

        let result = run_tool_call_loop_with_non_cli_approval_context(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            Some(&approval_mgr),
            "telegram",
            None,
            &crate::config::MultimodalConfig::default(),
            4,
            None,
            None,
            None,
            &[],
            Some(writer),
            Some(lineage),
        )
        .await
        .expect("tool loop should complete");
        assert_eq!(result, "done");

        // Inspect the snapshot — should carry the lineage on iteration 0.
        let turn_dirs: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .collect();
        assert_eq!(turn_dirs.len(), 1);
        let snap_file = turn_dirs[0].path().join("000000.json");
        let body = std::fs::read_to_string(&snap_file).unwrap();
        let snap: Snapshot = serde_json::from_str(&body).unwrap();
        assert_eq!(snap.iteration, 0);
        assert_eq!(snap.parent_turn_id.as_deref(), Some("parent-abc"));
        assert_eq!(snap.branched_from_iteration, Some(5));
    }
}
