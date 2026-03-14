use crate::memory::capsules::CapsuleStore;
use crate::memory::embeddings::EmbeddingProvider;
use crate::providers::{ChatMessage, Provider};
use crate::util::truncate_with_ellipsis;
use anyhow::Result;
use std::sync::Arc;

/// Keep this many most-recent non-system messages after compaction.
const COMPACTION_KEEP_RECENT_MESSAGES: usize = 20;

/// Safety cap for compaction source transcript passed to the summarizer.
const COMPACTION_MAX_SOURCE_CHARS: usize = 32_000;

/// Max characters retained in stored compaction summary.
const COMPACTION_MAX_SUMMARY_CHARS: usize = 6_000;

/// Max characters for a single tool result in the transcript.
const TOOL_RESULT_MAX_CHARS: usize = 500;

/// Trigger token-based compaction when input tokens exceed this fraction of max_context_tokens.
const COMPACTION_TOKEN_THRESHOLD_RATIO: f64 = 0.70;

/// Trigger mid-loop emergency trim when estimated tokens exceed this fraction of max_context_tokens.
/// Higher than COMPACTION_TOKEN_THRESHOLD_RATIO because this is a fast trim (no LLM call).
const MID_LOOP_TRIM_TOKEN_THRESHOLD_RATIO: f64 = 0.80;

/// Default max context tokens for mid-loop trim estimation.
/// Used when the caller doesn't provide a specific value.
const MID_LOOP_DEFAULT_MAX_CONTEXT_TOKENS: usize = 200_000;

/// Number of recent messages to keep during mid-loop trim.
const MID_LOOP_KEEP_RECENT: usize = 30;

/// After trimming/draining messages, remove ALL orphaned `tool` role messages
/// whose matching assistant `tool_use` was removed by compaction or trimming.
/// This prevents "tool_call_id is not found" API errors.
///
/// A `tool` message is considered orphaned if the consecutive run of `tool` messages
/// it belongs to is not immediately preceded by an `assistant` message containing
/// tool_use content (i.e. not a compaction summary or plain text assistant message).
fn sanitize_after_trim(history: &mut Vec<ChatMessage>) {
    let start = if history.first().map_or(false, |m| m.role == "system") {
        1
    } else {
        0
    };

    // Scan entire history for orphaned tool messages, not just the beginning.
    // A run of consecutive `tool` messages is valid only if immediately preceded
    // by an assistant message that contains tool_use (has "tool_use" in content).
    let mut i = start;
    while i < history.len() {
        if history[i].role == "tool" {
            // Find the start of this consecutive run of tool messages
            let run_start = i;
            while i < history.len() && history[i].role == "tool" {
                i += 1;
            }
            // Check the message before the run
            let is_orphaned = if run_start == 0 {
                true
            } else {
                let before = &history[run_start - 1];
                // Valid predecessor: an assistant message that contains tool call data.
                // Native mode stores JSON with "tool_calls" key; prompt mode uses <tool_call> XML.
                // Compaction summaries and plain assistant messages contain neither.
                !(before.role == "assistant"
                    && (before.content.contains("\"tool_calls\"")
                        || before.content.contains("<tool_call")))
            };

            if is_orphaned {
                let count = i - run_start;
                tracing::debug!(
                    "sanitize_after_trim: removing {} orphaned tool message(s) at index {}",
                    count,
                    run_start
                );
                history.drain(run_start..i);
                i = run_start; // re-check from same position
                continue;
            }
        } else {
            i += 1;
        }
    }

    let start = if history.first().map_or(false, |m| m.role == "system") {
        1
    } else {
        0
    };

    // Anthropic API requires first non-system message to be "user" role.
    // After compaction, an assistant summary may end up first — fix by inserting
    // a synthetic user message.
    if start < history.len() && history[start].role == "assistant" {
        tracing::debug!(
            "sanitize_after_trim: inserting synthetic user message before assistant at index {}",
            start
        );
        history.insert(start, ChatMessage::user("请继续".to_string()));
    }
}

/// Estimate the total token count of conversation history using a rough char/4 heuristic.
/// Returns 0 for empty history.
pub(crate) fn estimate_history_tokens(history: &[ChatMessage]) -> usize {
    history
        .iter()
        .map(|m| m.content.len().div_ceil(4))
        .sum()
}

/// Check whether compaction should trigger based on token usage.
pub(crate) fn should_compact_by_tokens(
    history: &[ChatMessage],
    last_input_tokens: Option<u64>,
    max_context_tokens: usize,
) -> bool {
    if max_context_tokens == 0 {
        return false;
    }

    let threshold = (max_context_tokens as f64 * COMPACTION_TOKEN_THRESHOLD_RATIO) as u64;
    let current_tokens =
        last_input_tokens.unwrap_or_else(|| estimate_history_tokens(history) as u64);

    current_tokens >= threshold
}

/// Mid-loop emergency trim: lightweight context reduction without LLM summarization.
///
/// Called inside `run_tool_call_loop` after each tool result batch. Uses char-based
/// token estimation to detect when context is approaching the limit, then trims
/// older messages (preserving system prompt + recent messages) so the loop can
/// continue without hitting API context-length errors.
///
/// Returns `true` if a trim was performed.
pub(crate) fn mid_loop_trim_if_needed(history: &mut Vec<ChatMessage>) -> bool {
    let estimated = estimate_history_tokens(history) as u64;
    let threshold =
        (MID_LOOP_DEFAULT_MAX_CONTEXT_TOKENS as f64 * MID_LOOP_TRIM_TOKEN_THRESHOLD_RATIO) as u64;

    if estimated < threshold {
        return false;
    }

    let has_system = history.first().map_or(false, |m| m.role == "system");
    let non_system_count = if has_system {
        history.len().saturating_sub(1)
    } else {
        history.len()
    };

    if non_system_count <= MID_LOOP_KEEP_RECENT {
        return false;
    }

    let start = if has_system { 1 } else { 0 };
    let to_remove = non_system_count - MID_LOOP_KEEP_RECENT;
    history.drain(start..start + to_remove);
    sanitize_after_trim(history);

    tracing::info!(
        estimated_tokens = estimated,
        threshold = threshold,
        removed_messages = to_remove,
        remaining = history.len(),
        "mid-loop trim: context approaching limit, trimmed old messages"
    );
    true
}

/// Trim conversation history to prevent unbounded growth.
/// Preserves the system prompt (first message if role=system) and the most recent messages.
pub(crate) fn trim_history(history: &mut Vec<ChatMessage>, max_history: usize) {
    let has_system = history.first().map_or(false, |m| m.role == "system");
    let non_system_count = if has_system {
        history.len() - 1
    } else {
        history.len()
    };

    if non_system_count <= max_history {
        return;
    }

    let start = if has_system { 1 } else { 0 };
    let to_remove = non_system_count - max_history;
    history.drain(start..start + to_remove);
    sanitize_after_trim(history);
}

/// Check if a message content looks like a tool result (verbose output that can be truncated).
fn is_tool_output(content: &str) -> bool {
    // Tool results from Plaw typically start with markers or contain structured output
    content.starts_with("Tool ")
        || content.starts_with("```")
        || content.starts_with("{\"")
        || content.contains("[tool_result]")
        || content.contains("[Tool output]")
}

/// Build compaction transcript with smart content weighting.
/// - User messages and AI decision text: preserved in full (up to budget)
/// - Tool call results and verbose outputs: truncated aggressively
/// - Previous compaction summaries: included with [Prior Summary] marker
pub(crate) fn build_compaction_transcript(messages: &[ChatMessage]) -> String {
    let mut transcript = String::new();
    let mut char_budget = COMPACTION_MAX_SOURCE_CHARS;

    for msg in messages {
        if char_budget == 0 {
            break;
        }

        let role = msg.role.to_uppercase();
        let content = msg.content.trim();

        // Detect prior compaction summary — include with marker
        if content.starts_with("[Compaction summary]") {
            let section = format!("[Prior Summary]\n{}\n\n", content);
            let section_len = section.chars().count();
            if section_len <= char_budget {
                transcript.push_str(&section);
                char_budget -= section_len;
            } else {
                let truncated = truncate_with_ellipsis(&section, char_budget);
                transcript.push_str(&truncated);
                char_budget = 0;
            }
            continue;
        }

        // Tool results: aggressive truncation
        if is_tool_output(content) || (role == "ASSISTANT" && content.len() > 2000 && content.contains('\n')) {
            let truncated_content = if content.chars().count() > TOOL_RESULT_MAX_CHARS {
                truncate_with_ellipsis(content, TOOL_RESULT_MAX_CHARS)
            } else {
                content.to_string()
            };
            let line = format!("{role}: {truncated_content}\n");
            let line_len = line.chars().count();
            if line_len <= char_budget {
                transcript.push_str(&line);
                char_budget -= line_len;
            }
            continue;
        }

        // User messages and AI decisions: full content
        let line = format!("{role}: {content}\n");
        let line_len = line.chars().count();
        if line_len <= char_budget {
            transcript.push_str(&line);
            char_budget -= line_len;
        } else {
            let truncated = truncate_with_ellipsis(&line, char_budget);
            transcript.push_str(&truncated);
            char_budget = 0;
        }
    }

    transcript
}

/// Check if a compaction summary indicates pending/unresolved tasks.
pub(crate) fn summary_has_pending_tasks(summary: &str) -> bool {
    let lower = summary.to_lowercase();
    // Check for the structured section header or common pending indicators
    lower.contains("## pending")
        || lower.contains("## unresolved")
        || lower.contains("未完成")
        || lower.contains("待完成")
        || lower.contains("还需要")
        || lower.contains("not yet")
        || lower.contains("todo")
        || lower.contains("remaining task")
}

pub(crate) fn apply_compaction_summary(
    history: &mut Vec<ChatMessage>,
    start: usize,
    compact_end: usize,
    summary: &str,
) {
    let summary_msg =
        ChatMessage::assistant(format!("[Compaction summary]\n{}", summary.trim()));
    history.splice(start..compact_end, std::iter::once(summary_msg));
    sanitize_after_trim(history);
}

/// Structured summarizer system prompt.
/// Produces a summary with clearly labeled sections that the AI can parse in future turns.
const SUMMARIZER_SYSTEM: &str = r#"You are a conversation compaction engine for an AI coding assistant. Your job is to compress older conversation history into a structured summary that preserves all context needed for the AI to continue working effectively.

Output the summary in the following structured format (use exactly these section headers):

## Current Task
What the user is currently trying to accomplish. Include specific goals and constraints.

## Completed Work
What has already been done. Include:
- Files created or modified (with paths)
- Key code changes and their purpose
- Commands run and their outcomes

## Pending/Unresolved
Tasks mentioned but not yet completed, open questions, or blockers.

## Key Decisions
Important choices made during the conversation (tech choices, architecture decisions, trade-offs).

## User Preferences
Any stated preferences about workflow, style, language, or approach.

## Relevant Code Context
Important file paths, function names, variable names, or code patterns that the AI needs to remember.

## Keywords
5-10 key terms/phrases that identify this conversation segment. Include tool names, technology names, file paths, error types, and feature names. Output as a comma-separated list on a single line.

Rules:
- Keep each section concise (2-5 bullet points max)
- Omit empty sections entirely (except Keywords — always include Keywords)
- Preserve exact file paths, function names, and technical terms
- Focus on WHAT and WHY, not HOW (skip verbose implementation details)
- If prior compaction summaries exist, merge them — do not nest summaries
- Tool execution logs should be reduced to their key findings/outcomes only
- Use the same language as the conversation (if Chinese, write in Chinese)
- Keywords section must always be present, even when other sections are omitted"#;

/// Auto-compact conversation history.
///
/// Triggers when EITHER condition is met:
/// 1. Message count exceeds `max_history` (existing behavior)
/// 2. Token usage exceeds 70% of `max_context_tokens` (token-aware trigger)
pub(crate) async fn auto_compact_history(
    history: &mut Vec<ChatMessage>,
    provider: &dyn Provider,
    model: &str,
    max_history: usize,
    last_input_tokens: Option<u64>,
    max_context_tokens: usize,
    capsule_store: Option<&Arc<CapsuleStore>>,
    session_id: Option<&str>,
    embedding_provider: Option<&Arc<dyn EmbeddingProvider>>,
) -> Result<bool> {
    let has_system = history.first().map_or(false, |m| m.role == "system");
    let non_system_count = if has_system {
        history.len().saturating_sub(1)
    } else {
        history.len()
    };

    let trigger_by_count = non_system_count > max_history;
    let trigger_by_tokens =
        should_compact_by_tokens(history, last_input_tokens, max_context_tokens);

    if !trigger_by_count && !trigger_by_tokens {
        return Ok(false);
    }

    let start = if has_system { 1 } else { 0 };
    let keep_recent = COMPACTION_KEEP_RECENT_MESSAGES.min(non_system_count);
    let compact_count = non_system_count.saturating_sub(keep_recent);
    if compact_count == 0 {
        return Ok(false);
    }

    let compact_end = start + compact_count;
    let to_compact: Vec<ChatMessage> = history[start..compact_end].to_vec();
    let transcript = build_compaction_transcript(&to_compact);

    let summarizer_user = format!(
        "Compress the following conversation history into a structured summary.\n\n---\n{}\n---",
        transcript
    );

    let summary_raw = provider
        .chat_with_system(Some(SUMMARIZER_SYSTEM), &summarizer_user, model, 0.2)
        .await
        .unwrap_or_else(|_| {
            // Fallback: deterministic local truncation when summarization fails.
            truncate_with_ellipsis(&transcript, COMPACTION_MAX_SUMMARY_CHARS)
        });

    let summary = truncate_with_ellipsis(&summary_raw, COMPACTION_MAX_SUMMARY_CHARS);

    // ── Capsule archival: preserve pre-compact messages ──────────
    if let (Some(store), Some(sid)) = (capsule_store, session_id) {
        let keywords = extract_keywords_from_summary(&summary);
        // Serialize messages for archival (role: content pairs)
        let serialized = serialize_messages_for_capsule(&to_compact);
        let token_estimate = estimate_tokens_simple(&serialized);

        // Embed the summary for semantic search (best-effort, non-blocking failure)
        let embedding = if let Some(emb) = embedding_provider {
            match emb.embed_one(&summary).await {
                Ok(vec) => Some(vec),
                Err(e) => {
                    tracing::warn!("[capsule] embedding failed (falling back to keyword-only): {e}");
                    None
                }
            }
        } else {
            None
        };

        if let Err(e) = store.create_from_compact(
            sid,
            keywords,
            &summary,
            &serialized,
            token_estimate,
            to_compact.len() as u64,
            embedding,
        ) {
            eprintln!("[capsule] Failed to archive capsule: {e}");
        }
    }

    apply_compaction_summary(history, start, compact_end, &summary);

    Ok(true)
}

/// Extract keywords from the structured summary's `## Keywords` section.
fn extract_keywords_from_summary(summary: &str) -> Vec<String> {
    let mut in_keywords = false;
    for line in summary.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## Keywords") || trimmed.starts_with("## 关键词") {
            in_keywords = true;
            continue;
        }
        if in_keywords {
            if trimmed.starts_with("##") {
                break; // next section
            }
            if !trimmed.is_empty() {
                // Parse comma-separated keywords
                return trimmed
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    // Fallback: no Keywords section found — extract from section headers
    Vec::new()
}

/// Serialize chat messages into a human-readable archive format.
fn serialize_messages_for_capsule(messages: &[ChatMessage]) -> String {
    let mut buf = String::new();
    for msg in messages {
        buf.push_str(&format!("[{}]\n{}\n\n", msg.role, msg.content));
    }
    buf
}

/// Simple token estimate: ~4 chars per token (rough approximation).
fn estimate_tokens_simple(text: &str) -> u64 {
    (text.len() as u64) / 4
}
