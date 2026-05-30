use super::parsing::ParsedToolCall;
use super::{scrub_credentials, ToolLoopCancelled, DRAFT_PROGRESS_SENTINEL};
use crate::approval::ApprovalManager;
use crate::observability::{Observer, ObserverEvent};
use crate::tools::Tool;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn find_tool<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> Option<&'a dyn Tool> {
    tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
}

/// Generate a human-readable progress hint from tool name + arguments.
fn describe_tool_action(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    let action = args.get("action").and_then(|v| v.as_str());
    let url = args.get("url").and_then(|v| v.as_str());
    let selector = args.get("selector").and_then(|v| v.as_str());
    let command = args.get("command").and_then(|v| v.as_str());
    let path = args.get("path").and_then(|v| v.as_str());

    match tool_name {
        "browser" => {
            let act = action?;
            Some(match act {
                "open" => format!("Navigating to {}", url.unwrap_or("...")),
                "snapshot" => "Taking page snapshot".to_string(),
                "click" => format!("Clicking {}", selector.unwrap_or("element")),
                "fill" => format!("Filling {}", selector.unwrap_or("field")),
                "type" => format!("Typing into {}", selector.unwrap_or("field")),
                "get_text" => format!("Extracting text from {}", selector.unwrap_or("element")),
                "get_title" => "Getting page title".to_string(),
                "get_url" => "Getting current URL".to_string(),
                "screenshot" => "Taking screenshot".to_string(),
                "wait" => "Waiting for condition".to_string(),
                "press" => format!("Pressing key: {}", args.get("key").and_then(|v| v.as_str()).unwrap_or("?")),
                "hover" => format!("Hovering over {}", selector.unwrap_or("element")),
                "scroll" => format!("Scrolling {}", args.get("direction").and_then(|v| v.as_str()).unwrap_or("down")),
                "is_visible" => format!("Checking visibility: {}", selector.unwrap_or("element")),
                "close" => "Closing browser".to_string(),
                "find" => format!("Finding element: {}={}", args.get("by").and_then(|v| v.as_str()).unwrap_or("?"), args.get("value").and_then(|v| v.as_str()).unwrap_or("?")),
                other => format!("Browser action: {other}"),
            })
        }
        "shell" => command.map(|cmd| {
            let short = if cmd.chars().count() > 60 {
                cmd.chars().take(60).collect::<String>()
            } else {
                cmd.to_string()
            };
            format!("Running: {short}")
        }),
        "file_read" | "read_file" => path.map(|p| format!("Reading {p}")),
        "file_write" | "write_file" => path.map(|p| format!("Writing {p}")),
        "file_edit" | "edit_file" => path.map(|p| format!("Editing {p}")),
        "web_fetch" => url.map(|u| format!("Fetching {u}")),
        "web_search" => args.get("query").and_then(|v| v.as_str()).map(|q| format!("Searching: {q}")),
        "http_request" => url.map(|u| {
            let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
            format!("{method} {u}")
        }),
        _ => None,
    }
}

/// Send a progress event through the on_delta channel.
async fn send_progress(on_delta: Option<&mpsc::Sender<String>>, tool_name: &str, msg: &str) {
    if let Some(tx) = on_delta {
        let event = format!(
            "{DRAFT_PROGRESS_SENTINEL}\x00TOOL_PROGRESS\x00{tool_name}|{msg}"
        );
        let _ = tx.send(event).await;
    }
}

async fn execute_one_tool(
    call_name: &str,
    call_arguments: serde_json::Value,
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
    on_delta: Option<&mpsc::Sender<String>>,
) -> Result<ToolExecutionOutcome> {
    observer.record_event(&ObserverEvent::ToolCallStart {
        tool: call_name.to_string(),
    });
    let start = Instant::now();

    let Some(tool) = find_tool(tools_registry, call_name) else {
        let reason = format!("Unknown tool: {call_name}");
        let duration = start.elapsed();
        observer.record_event(&ObserverEvent::ToolCall {
            tool: call_name.to_string(),
            duration,
            success: false,
        });
        return Ok(ToolExecutionOutcome {
            output: reason.clone(),
            success: false,
            error_reason: Some(scrub_credentials(&reason)),
            duration,
        });
    };

    // Send pre-execution progress hint
    let progress_hint = describe_tool_action(call_name, &call_arguments);
    let mut progress_log = Vec::new();
    if let Some(ref hint) = progress_hint {
        send_progress(on_delta, call_name, hint).await;
        progress_log.push(hint.clone());
    }

    // Execute the tool. `execute_validated` short-circuits with a structured
    // validation error before dispatching to the tool body if args don't
    // match `parameters_schema()`.
    let tool_future = tool.execute_validated(call_arguments);
    let tool_result = if let Some(token) = cancellation_token {
        tokio::select! {
            () = token.cancelled() => return Err(ToolLoopCancelled.into()),
            result = tool_future => result,
        }
    } else {
        tool_future.await
    };

    match tool_result {
        Ok(r) => {
            let duration = start.elapsed();
            observer.record_event(&ObserverEvent::ToolCall {
                tool: call_name.to_string(),
                duration,
                success: r.success,
            });
            if r.success {
                let output = prepend_progress_log(&progress_log, &r.output, duration);
                Ok(ToolExecutionOutcome {
                    output: scrub_credentials(&output),
                    success: true,
                    error_reason: None,
                    duration,
                })
            } else {
                let reason = r.error.unwrap_or(r.output);
                let output = prepend_progress_log(&progress_log, &format!("Error: {reason}"), duration);
                Ok(ToolExecutionOutcome {
                    output,
                    success: false,
                    error_reason: Some(scrub_credentials(&reason)),
                    duration,
                })
            }
        }
        Err(e) => {
            let duration = start.elapsed();
            observer.record_event(&ObserverEvent::ToolCall {
                tool: call_name.to_string(),
                duration,
                success: false,
            });
            let reason = format!("Error executing {call_name}: {e}");
            let output = prepend_progress_log(&progress_log, &reason, duration);
            Ok(ToolExecutionOutcome {
                output: scrub_credentials(&output),
                success: false,
                error_reason: Some(scrub_credentials(&reason)),
                duration,
            })
        }
    }
}

/// Prepend execution steps to tool output so the AI knows what happened.
fn prepend_progress_log(steps: &[String], output: &str, duration: Duration) -> String {
    if steps.is_empty() {
        return output.to_string();
    }
    let mut log = format!("[Execution steps ({:.1}s)]\n", duration.as_secs_f64());
    for step in steps {
        log.push_str(&format!("- {step}\n"));
    }
    log.push_str("[Result]\n");
    log.push_str(output);
    log
}

pub(super) struct ToolExecutionOutcome {
    pub(super) output: String,
    pub(super) success: bool,
    pub(super) error_reason: Option<String>,
    pub(super) duration: Duration,
}

pub(super) fn should_execute_tools_in_parallel(
    tool_calls: &[ParsedToolCall],
    tools_registry: &[Box<dyn Tool>],
    approval: Option<&ApprovalManager>,
) -> bool {
    if tool_calls.len() <= 1 {
        return false;
    }

    if let Some(mgr) = approval {
        if tool_calls
            .iter()
            .any(|call| mgr.needs_approval(&call.name, &call.arguments))
        {
            return false;
        }
    }

    // Parallel-safety gate: only run in parallel if EVERY tool declares
    // `SideEffectClass::ReadOnly`. Anything that mutates local FS/process,
    // talks to a remote endpoint, spawns sub-agents, or hasn't declared
    // (`Unknown`) is treated as potentially racy with concurrent calls and
    // forced sequential — e.g. two `shell` calls touching the same path
    // would otherwise interleave non-deterministically, even when the user
    // has waived approval. Tool authors opt into parallelism by overriding
    // `Tool::side_effects()` to `ReadOnly` (see `tools/traits.rs`).
    //
    // Unknown tool name (not in the live registry) is treated conservatively
    // as non-ReadOnly — the dispatcher will reject it shortly anyway, but
    // we shouldn't widen the parallel window based on a name we can't
    // verify.
    use crate::tools::traits::SideEffectClass;
    tool_calls.iter().all(|call| {
        tools_registry
            .iter()
            .find(|t| t.name() == call.name)
            .map(|t| t.side_effects() == SideEffectClass::ReadOnly)
            .unwrap_or(false)
    })
}

pub(super) async fn execute_tools_parallel(
    tool_calls: &[ParsedToolCall],
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
    on_delta: Option<&mpsc::Sender<String>>,
) -> Result<Vec<ToolExecutionOutcome>> {
    let futures: Vec<_> = tool_calls
        .iter()
        .map(|call| {
            execute_one_tool(
                &call.name,
                call.arguments.clone(),
                tools_registry,
                observer,
                cancellation_token,
                on_delta,
            )
        })
        .collect();

    let results = futures_util::future::join_all(futures).await;
    results.into_iter().collect()
}

pub(super) async fn execute_tools_sequential(
    tool_calls: &[ParsedToolCall],
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
    on_delta: Option<&mpsc::Sender<String>>,
) -> Result<Vec<ToolExecutionOutcome>> {
    let mut outcomes = Vec::with_capacity(tool_calls.len());

    for call in tool_calls {
        outcomes.push(
            execute_one_tool(
                &call.name,
                call.arguments.clone(),
                tools_registry,
                observer,
                cancellation_token,
                on_delta,
            )
            .await?,
        );
    }

    Ok(outcomes)
}
