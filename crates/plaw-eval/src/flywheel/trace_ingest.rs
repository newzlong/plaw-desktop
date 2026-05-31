//! Direct trace → eval case ingestion.
//!
//! Sibling to [`super::promoter`] / [`super::reviewer`]: the flywheel
//! pipeline routes traces through a human-review queue before promotion;
//! `trace_ingest` skips the queue and writes a case directly. Used by
//! the `plaw-eval add-from-trace <turn_id> --suite <name>` CLI for fast
//! "I just had an interesting plaw session, regression-test that exact
//! tool sequence" workflows.
//!
//! Reads plaw's `state/runtime-trace.jsonl` (the producer-side format
//! emitted by [`crate::observability::runtime_trace`] in the plaw lib
//! crate — same JSONL the OpenTelemetry GenAI attrs PR #64 enriched
//! with `gen_ai.*` keys, but ingest only needs the structured fields:
//! `event_type`, `turn_id`, and `payload`).
//!
//! Closes audit item #9 ([[oss-agent-framework-audit-2026-05-30]]) —
//! the prod→eval closed-loop that distinguishes serious users from
//! demo users.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;

use crate::suite::{load_suite, Case, CaseExpected, CaseInput};

/// Result of a successful `add-from-trace` ingest.
#[derive(Debug, Clone)]
pub struct TraceIngestResult {
    /// New case id assigned by [`super::promoter::synthesise_case_id`].
    pub new_case_id: String,
    /// Suite TOML path the case was appended to.
    pub target_suite_path: String,
    /// Ordered tool names extracted from the trace's `tool_call_start`
    /// events. May be empty for chat-only turns.
    pub tool_sequence: Vec<String>,
    /// Number of agent-loop iterations observed in the trace.
    pub iterations: usize,
    /// Number of trace events matched against the input id.
    pub events_matched: usize,
    /// Bytes appended to the suite file.
    pub appended_bytes: usize,
}

/// Materialize a [`Case`] from a plaw runtime-trace JSONL file and
/// append it to the target suite.
///
/// Matching: events are filtered by `turn_id == id` first, then
/// `trace_id == id` as fallback (a trace_id is the cross-loop
/// correlation, a turn_id is per-loop-invocation; users typically
/// know the turn_id from `plaw checkpoint list` or the plaw UI).
///
/// `task_hint`: optional free-text used as the case's `[cases.input]
/// task` field. When `None`, a TODO placeholder is written so the
/// operator must hand-edit before running — keeps the cli from
/// silently generating eval cases the operator hasn't reviewed.
pub fn ingest_trace_from_jsonl(
    trace_or_turn_id: &str,
    target_suite_path: impl AsRef<Path>,
    trace_jsonl_path: impl AsRef<Path>,
    task_hint: Option<&str>,
) -> Result<TraceIngestResult> {
    let target_suite_path = target_suite_path.as_ref();
    let trace_jsonl_path = trace_jsonl_path.as_ref();

    if trace_or_turn_id.trim().is_empty() {
        anyhow::bail!("trace_or_turn_id must be non-empty");
    }

    let body = fs::read_to_string(trace_jsonl_path).with_context(|| {
        format!(
            "reading runtime trace at {}",
            trace_jsonl_path.display()
        )
    })?;
    let extracted = extract_trace(&body, trace_or_turn_id)?;
    if extracted.events_matched == 0 {
        anyhow::bail!(
            "no events found for id '{trace_or_turn_id}' in {}. \
             Check the id is correct (look in plaw-data/state/checkpoints/ \
             or the runtime trace) — both `turn_id` and `trace_id` are accepted.",
            trace_jsonl_path.display()
        );
    }

    let suite = load_suite(target_suite_path).with_context(|| {
        format!(
            "loading target suite at {}",
            target_suite_path.display()
        )
    })?;
    let new_case_id = super::promoter::synthesise_case_id(
        &suite,
        &format!("from-trace-{}", trace_or_turn_id_short(trace_or_turn_id)),
    );

    let task_text = task_hint
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "TODO: describe the user task that produced this trace. \
                 Observed: {} tool call(s) across {} iteration(s). \
                 Source id: {trace_or_turn_id}",
                extracted.tool_sequence.len(),
                extracted.iterations
            )
        });

    let case = Case {
        id: new_case_id.clone(),
        input: CaseInput::Agent {
            task: task_text,
            max_steps: extracted.iterations.max(5),
        },
        expected: if extracted.tool_sequence.is_empty() {
            None
        } else {
            Some(CaseExpected {
                tool_sequence: extracted.tool_sequence.clone(),
                ..CaseExpected::default()
            })
        },
        tags: vec!["from_trace".to_string()],
        cluster_id: None,
        source: "from_trace".to_string(),
        promoted_at: Some(Utc::now().to_rfc3339()),
        metrics: None,
    };

    let appended_bytes = super::promoter::append_case_to_suite(target_suite_path, &case)?;

    Ok(TraceIngestResult {
        new_case_id,
        target_suite_path: target_suite_path.display().to_string(),
        tool_sequence: extracted.tool_sequence,
        iterations: extracted.iterations,
        events_matched: extracted.events_matched,
        appended_bytes,
    })
}

/// Parsed trace cross-section: just the bits needed to assemble a case.
/// Public so callers (and tests) can poke at the extraction logic
/// without writing a file.
#[derive(Debug, Clone, Default)]
pub struct ExtractedTrace {
    pub tool_sequence: Vec<String>,
    pub iterations: usize,
    pub events_matched: usize,
    pub failed_tool_calls: usize,
}

/// Pure extractor: turns a JSONL blob + id into [`ExtractedTrace`].
/// Separated from [`ingest_trace_from_jsonl`] so tests don't need a
/// real file or a real suite TOML.
///
/// Schema accepted: per-line `serde_json::Value` with at least the
/// fields produced by [`crate::observability::runtime_trace::RuntimeTraceEvent`]:
/// `event_type` (str), `turn_id` (str?), `trace_id` (str?), `success`
/// (bool?), `payload` (object).
///
/// Recognized event types (extra types are ignored — forward-compatible
/// with future runtime_trace additions):
/// - `tool_call_start` → appends `payload.tool_name` to `tool_sequence`
/// - `tool_call` → counts iterations
/// - `tool_call_parse_issue` → counts as failed
/// - `llm_response` → counts iterations
pub fn extract_trace(jsonl_body: &str, id: &str) -> Result<ExtractedTrace> {
    let mut tool_sequence: Vec<String> = Vec::new();
    let mut iteration_seen: HashSet<u64> = HashSet::new();
    let mut events_matched = 0usize;
    let mut failed_tool_calls = 0usize;

    for (line_no, line) in jsonl_body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    line = line_no + 1,
                    error = %e,
                    "skipping malformed runtime trace line during ingest"
                );
                continue;
            }
        };

        let matches = event
            .get("turn_id")
            .and_then(Value::as_str)
            .map(|t| t == id)
            .unwrap_or(false)
            || event
                .get("trace_id")
                .and_then(Value::as_str)
                .map(|t| t == id)
                .unwrap_or(false);
        if !matches {
            continue;
        }
        events_matched += 1;

        let event_type = event.get("event_type").and_then(Value::as_str).unwrap_or("");
        let payload = event.get("payload");

        match event_type {
            "tool_call_start" => {
                if let Some(name) = payload
                    .and_then(|p| p.get("tool_name"))
                    .and_then(Value::as_str)
                {
                    tool_sequence.push(name.to_string());
                }
            }
            "tool_call_parse_issue" => {
                failed_tool_calls += 1;
            }
            "llm_response" | "tool_call" => {
                if let Some(iter) =
                    payload.and_then(|p| p.get("iteration")).and_then(Value::as_u64)
                {
                    iteration_seen.insert(iter);
                }
            }
            _ => {}
        }
    }

    Ok(ExtractedTrace {
        tool_sequence,
        iterations: iteration_seen.len(),
        events_matched,
        failed_tool_calls,
    })
}

/// Short prefix of an id for synthesizing readable case ids.
/// Strips leading uuid-version segment if recognizable; otherwise
/// takes the first 8 chars.
fn trace_or_turn_id_short(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let head = cleaned.split('-').next().unwrap_or(&cleaned);
    let n = head.chars().take(8).collect::<String>();
    if n.is_empty() {
        "trace".to_string()
    } else {
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_event(
        event_type: &str,
        turn_id: &str,
        payload: serde_json::Value,
    ) -> String {
        serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "timestamp": "2026-06-01T00:00:00+00:00",
            "event_type": event_type,
            "turn_id": turn_id,
            "payload": payload,
        })
        .to_string()
    }

    fn make_event_with_trace_id(
        event_type: &str,
        trace_id: &str,
        payload: serde_json::Value,
    ) -> String {
        serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "timestamp": "2026-06-01T00:00:00+00:00",
            "event_type": event_type,
            "trace_id": trace_id,
            "payload": payload,
        })
        .to_string()
    }

    fn sample_minimal_suite(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("cases.toml");
        // load_suite() rejects empty `cases`, so seed one placeholder
        // entry — ingest_trace_from_jsonl reloads after appending so
        // this case ends up alongside the synthesized one.
        let toml = r#"
name = "smoke"
version = "1.0.0"
description = "test"

[default_judge]
model = "k2p5"
provider = "kimi"
mode = { kind = "score", scale = 5 }

[[cases]]
id = "seed-case"
[cases.input]
kind = "agent"
task = "Seed task — never executed in trace_ingest tests"
max_steps = 1
"#;
        fs::write(&path, toml).unwrap();
        path
    }

    // ── extract_trace ───────────────────────────────────────────────

    #[test]
    fn extract_collects_tool_sequence_in_order() {
        let body = format!(
            "{}\n{}\n{}\n",
            make_event(
                "tool_call_start",
                "turn-abc",
                serde_json::json!({"tool_name": "file_read", "iteration": 0})
            ),
            make_event(
                "tool_call_start",
                "turn-abc",
                serde_json::json!({"tool_name": "web_search_tool", "iteration": 0})
            ),
            make_event(
                "tool_call_start",
                "turn-abc",
                serde_json::json!({"tool_name": "file_write", "iteration": 1})
            ),
        );
        let ex = extract_trace(&body, "turn-abc").unwrap();
        assert_eq!(ex.tool_sequence, vec!["file_read", "web_search_tool", "file_write"]);
        assert_eq!(ex.events_matched, 3);
    }

    #[test]
    fn extract_counts_iterations_from_llm_response_payload() {
        let body = format!(
            "{}\n{}\n{}\n",
            make_event("llm_response", "turn-x", serde_json::json!({"iteration": 0})),
            make_event("llm_response", "turn-x", serde_json::json!({"iteration": 1})),
            make_event("llm_response", "turn-x", serde_json::json!({"iteration": 2})),
        );
        let ex = extract_trace(&body, "turn-x").unwrap();
        assert_eq!(ex.iterations, 3);
    }

    #[test]
    fn extract_filters_out_events_for_other_turns() {
        let body = format!(
            "{}\n{}\n",
            make_event(
                "tool_call_start",
                "turn-a",
                serde_json::json!({"tool_name": "file_read"})
            ),
            make_event(
                "tool_call_start",
                "turn-b",
                serde_json::json!({"tool_name": "shell"})
            ),
        );
        let ex = extract_trace(&body, "turn-a").unwrap();
        assert_eq!(ex.tool_sequence, vec!["file_read"]);
        assert_eq!(ex.events_matched, 1);
    }

    #[test]
    fn extract_accepts_trace_id_when_turn_id_absent() {
        // Sub-agent / cron-fired trace: events carry trace_id but no
        // turn_id at the root. Ingest must match either.
        let body = format!(
            "{}\n",
            make_event_with_trace_id(
                "tool_call_start",
                "trace-cron-1",
                serde_json::json!({"tool_name": "git_operations"})
            ),
        );
        let ex = extract_trace(&body, "trace-cron-1").unwrap();
        assert_eq!(ex.tool_sequence, vec!["git_operations"]);
        assert_eq!(ex.events_matched, 1);
    }

    #[test]
    fn extract_returns_zero_matches_for_unknown_id() {
        let body = format!(
            "{}\n",
            make_event(
                "tool_call_start",
                "turn-x",
                serde_json::json!({"tool_name": "shell"})
            )
        );
        let ex = extract_trace(&body, "ghost-turn").unwrap();
        assert_eq!(ex.events_matched, 0);
        assert!(ex.tool_sequence.is_empty());
    }

    #[test]
    fn extract_counts_parse_issues_as_failed_tool_calls() {
        let body = format!(
            "{}\n{}\n",
            make_event(
                "tool_call_start",
                "turn-x",
                serde_json::json!({"tool_name": "shell"})
            ),
            make_event(
                "tool_call_parse_issue",
                "turn-x",
                serde_json::json!({"iteration": 0})
            ),
        );
        let ex = extract_trace(&body, "turn-x").unwrap();
        assert_eq!(ex.failed_tool_calls, 1);
    }

    #[test]
    fn extract_skips_malformed_json_lines_without_aborting() {
        let body = format!(
            "{{ not-json\n{}\n{{\"truncated\":\n",
            make_event(
                "tool_call_start",
                "turn-x",
                serde_json::json!({"tool_name": "shell"})
            ),
        );
        let ex = extract_trace(&body, "turn-x").unwrap();
        assert_eq!(ex.tool_sequence, vec!["shell"]);
    }

    #[test]
    fn extract_rejects_empty_id() {
        // Tested at the wrapper layer too, but the extractor itself
        // is permissive — empty id matches nothing, returns 0.
        let body = format!(
            "{}\n",
            make_event(
                "tool_call_start",
                "turn-x",
                serde_json::json!({"tool_name": "shell"})
            )
        );
        let ex = extract_trace(&body, "").unwrap();
        assert_eq!(ex.events_matched, 0);
    }

    // ── ingest_trace_from_jsonl (file write path) ────────────────────

    #[test]
    fn ingest_appends_case_to_suite_with_tool_sequence() {
        let tmp = TempDir::new().unwrap();
        let suite_path = sample_minimal_suite(tmp.path());
        let jsonl_path = tmp.path().join("runtime-trace.jsonl");
        let body = format!(
            "{}\n{}\n{}\n",
            make_event("llm_response", "turn-test", serde_json::json!({"iteration": 0})),
            make_event(
                "tool_call_start",
                "turn-test",
                serde_json::json!({"tool_name": "file_read"})
            ),
            make_event(
                "tool_call_start",
                "turn-test",
                serde_json::json!({"tool_name": "shell"})
            ),
        );
        fs::write(&jsonl_path, body).unwrap();

        let result = ingest_trace_from_jsonl(
            "turn-test",
            &suite_path,
            &jsonl_path,
            Some("Run linter then ls"),
        )
        .unwrap();

        assert_eq!(result.tool_sequence, vec!["file_read", "shell"]);
        assert_eq!(result.iterations, 1);
        assert_eq!(result.events_matched, 3);
        // trace_or_turn_id_short splits on '-' and takes the head, so
        // "turn-test" → "turn"; ids end up like "from-trace-turn-flywheel-001".
        assert!(
            result.new_case_id.starts_with("from-trace-turn-flywheel-"),
            "unexpected id: {}",
            result.new_case_id
        );
        assert!(result.appended_bytes > 0);

        // Re-load the suite and verify the new case exists.
        let suite = load_suite(&suite_path).unwrap();
        let new = suite
            .cases
            .iter()
            .find(|c| c.id == result.new_case_id)
            .expect("new case present in re-loaded suite");
        assert_eq!(new.source, "from_trace");
        assert!(new.tags.contains(&"from_trace".to_string()));
        match &new.input {
            CaseInput::Agent { task, .. } => assert_eq!(task, "Run linter then ls"),
            other => panic!("expected Agent input, got {other:?}"),
        }
        let expected = new.expected.as_ref().unwrap();
        assert_eq!(expected.tool_sequence, vec!["file_read", "shell"]);
    }

    #[test]
    fn ingest_writes_todo_task_when_no_hint_provided() {
        let tmp = TempDir::new().unwrap();
        let suite_path = sample_minimal_suite(tmp.path());
        let jsonl_path = tmp.path().join("rt.jsonl");
        let body = format!(
            "{}\n",
            make_event(
                "tool_call_start",
                "turn-y",
                serde_json::json!({"tool_name": "shell"})
            )
        );
        fs::write(&jsonl_path, body).unwrap();

        let result = ingest_trace_from_jsonl("turn-y", &suite_path, &jsonl_path, None).unwrap();
        let suite = load_suite(&suite_path).unwrap();
        let new = suite.cases.iter().find(|c| c.id == result.new_case_id).unwrap();
        match &new.input {
            CaseInput::Agent { task, .. } => {
                assert!(task.starts_with("TODO:"), "expected TODO placeholder, got: {task}");
                assert!(task.contains("turn-y"));
            }
            other => panic!("expected Agent input, got {other:?}"),
        }
    }

    #[test]
    fn ingest_skips_expected_when_no_tools_observed() {
        // Chat-only turn (no tool calls). Case should still get created
        // but with no tool_sequence (chat-only eval).
        let tmp = TempDir::new().unwrap();
        let suite_path = sample_minimal_suite(tmp.path());
        let jsonl_path = tmp.path().join("rt.jsonl");
        let body = format!(
            "{}\n",
            make_event("llm_response", "turn-chat", serde_json::json!({"iteration": 0}))
        );
        fs::write(&jsonl_path, body).unwrap();

        let result = ingest_trace_from_jsonl("turn-chat", &suite_path, &jsonl_path, None).unwrap();
        let suite = load_suite(&suite_path).unwrap();
        let new = suite.cases.iter().find(|c| c.id == result.new_case_id).unwrap();
        assert!(new.expected.is_none() || new.expected.as_ref().unwrap().tool_sequence.is_empty());
    }

    #[test]
    fn ingest_errors_on_empty_id() {
        let tmp = TempDir::new().unwrap();
        let suite_path = sample_minimal_suite(tmp.path());
        let jsonl_path = tmp.path().join("rt.jsonl");
        fs::write(&jsonl_path, "").unwrap();
        let err = ingest_trace_from_jsonl("", &suite_path, &jsonl_path, None).unwrap_err();
        assert!(err.to_string().contains("must be non-empty"));
    }

    #[test]
    fn ingest_errors_on_unknown_id_with_clear_message() {
        let tmp = TempDir::new().unwrap();
        let suite_path = sample_minimal_suite(tmp.path());
        let jsonl_path = tmp.path().join("rt.jsonl");
        let body = format!(
            "{}\n",
            make_event(
                "tool_call_start",
                "turn-real",
                serde_json::json!({"tool_name": "shell"})
            )
        );
        fs::write(&jsonl_path, body).unwrap();
        let err = ingest_trace_from_jsonl("turn-ghost", &suite_path, &jsonl_path, None)
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no events found for id 'turn-ghost'"));
        assert!(msg.contains("turn_id"));
    }

    #[test]
    fn ingest_errors_on_missing_jsonl_file() {
        let tmp = TempDir::new().unwrap();
        let suite_path = sample_minimal_suite(tmp.path());
        let err = ingest_trace_from_jsonl(
            "turn-x",
            &suite_path,
            &tmp.path().join("nonexistent.jsonl"),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("reading runtime trace"));
    }

    #[test]
    fn ingest_synthesizes_unique_case_ids_on_repeat_calls() {
        let tmp = TempDir::new().unwrap();
        let suite_path = sample_minimal_suite(tmp.path());
        let jsonl_path = tmp.path().join("rt.jsonl");
        let body = format!(
            "{}\n",
            make_event(
                "tool_call_start",
                "turn-z",
                serde_json::json!({"tool_name": "shell"})
            )
        );
        fs::write(&jsonl_path, body).unwrap();

        let r1 = ingest_trace_from_jsonl("turn-z", &suite_path, &jsonl_path, Some("first"))
            .unwrap();
        let r2 = ingest_trace_from_jsonl("turn-z", &suite_path, &jsonl_path, Some("second"))
            .unwrap();
        assert_ne!(r1.new_case_id, r2.new_case_id, "repeat ingest must synthesize fresh ids");
    }
}
