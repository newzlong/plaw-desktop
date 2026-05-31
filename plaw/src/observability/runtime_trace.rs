use crate::config::ObservabilityConfig;
use crate::observability::trace_context::TraceContext;
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use uuid::Uuid;

const DEFAULT_TRACE_REL_PATH: &str = "state/runtime-trace.jsonl";

/// Runtime trace storage policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTraceStorageMode {
    None,
    Rolling,
    Full,
}

impl RuntimeTraceStorageMode {
    fn from_raw(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rolling" => Self::Rolling,
            "full" => Self::Full,
            _ => Self::None,
        }
    }
}

/// Structured runtime trace event for tool-call and model-reply diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeTraceEvent {
    pub id: String,
    pub timestamp: String,
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Root identifier for a logical trace (cron fire, sub-agent spawn, etc.).
    /// Shared across every event in the same trace; absent for events emitted
    /// outside any [`crate::observability::trace_context::CURRENT_TRACE`] scope.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub trace_id: Option<String>,
    /// Identifier of the current span. Distinct from [`Self::id`] (which is
    /// per-event); a single span typically emits multiple events.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub span_id: Option<String>,
    /// Parent span's identifier, if any. Empty at the root of a trace.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_span_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
    /// OpenTelemetry GenAI semantic-convention attributes derived from the
    /// structured fields and payload at emission time. Lets external
    /// consumers (Langfuse, Phoenix, Logfire, Datadog) ingest plaw trace
    /// files without per-tool field-mapping plugins. See
    /// <https://opentelemetry.io/docs/specs/semconv/gen-ai/> for the
    /// canonical attribute names.
    ///
    /// `BTreeMap` for stable JSON key ordering — diff-friendly snapshots
    /// and reproducible test fixtures. Omitted from serialization when
    /// empty, matching pre-OTel JSONL output bit-for-bit on events with
    /// no derivable GenAI attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
}

struct RuntimeTraceLogger {
    mode: RuntimeTraceStorageMode,
    max_entries: usize,
    path: PathBuf,
    write_lock: std::sync::Mutex<()>,
}

impl RuntimeTraceLogger {
    fn new(mode: RuntimeTraceStorageMode, max_entries: usize, path: PathBuf) -> Self {
        Self {
            mode,
            max_entries: max_entries.max(1),
            path,
            write_lock: std::sync::Mutex::new(()),
        }
    }

    fn append(&self, event: &RuntimeTraceEvent) -> Result<()> {
        if self.mode == RuntimeTraceStorageMode::None {
            return Ok(());
        }

        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let line = serde_json::to_string(event)?;
        let mut options = OpenOptions::new();
        options.create(true).append(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = options.open(&self.path)?;
        writeln!(file, "{line}")?;
        file.sync_data()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }

        if self.mode == RuntimeTraceStorageMode::Rolling {
            self.trim_to_last_entries()?;
        }

        Ok(())
    }

    fn trim_to_last_entries(&self) -> Result<()> {
        let raw = fs::read_to_string(&self.path).unwrap_or_default();
        let lines: Vec<&str> = raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();

        if lines.len() <= self.max_entries {
            return Ok(());
        }

        let keep_from = lines.len().saturating_sub(self.max_entries);
        let kept = &lines[keep_from..];
        let mut rewritten = kept.join("\n");
        rewritten.push('\n');

        let tmp = self.path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::write(&tmp, rewritten)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }

        fs::rename(tmp, &self.path)?;
        Ok(())
    }
}

static TRACE_LOGGER: LazyLock<RwLock<Option<Arc<RuntimeTraceLogger>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Resolve runtime trace storage mode from config.
pub fn storage_mode_from_config(config: &ObservabilityConfig) -> RuntimeTraceStorageMode {
    let mode = RuntimeTraceStorageMode::from_raw(&config.runtime_trace_mode);
    if mode == RuntimeTraceStorageMode::None
        && !config.runtime_trace_mode.trim().is_empty()
        && !config.runtime_trace_mode.eq_ignore_ascii_case("none")
    {
        tracing::warn!(
            mode = %config.runtime_trace_mode,
            "Unknown observability.runtime_trace_mode; falling back to none"
        );
    }
    mode
}

/// Resolve runtime trace path from config.
pub fn resolve_trace_path(config: &ObservabilityConfig, workspace_dir: &Path) -> PathBuf {
    let raw = config.runtime_trace_path.trim();
    let fallback = workspace_dir.join(DEFAULT_TRACE_REL_PATH);
    if raw.is_empty() {
        return fallback;
    }

    let configured = PathBuf::from(raw);
    if configured.is_absolute() {
        configured
    } else {
        workspace_dir.join(configured)
    }
}

/// Initialize (or disable) runtime trace logging.
pub fn init_from_config(config: &ObservabilityConfig, workspace_dir: &Path) {
    let mode = storage_mode_from_config(config);
    let logger = if mode == RuntimeTraceStorageMode::None {
        None
    } else {
        Some(Arc::new(RuntimeTraceLogger::new(
            mode,
            config.runtime_trace_max_entries.max(1),
            resolve_trace_path(config, workspace_dir),
        )))
    };

    let mut guard = TRACE_LOGGER.write().unwrap_or_else(|e| e.into_inner());
    *guard = logger;
}

/// Record a runtime trace event.
pub fn record_event(
    event_type: &str,
    channel: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
    turn_id: Option<&str>,
    success: Option<bool>,
    message: Option<&str>,
    payload: Value,
) {
    let logger = TRACE_LOGGER
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some(logger) = logger else {
        return;
    };

    // Stamp the ambient trace context if one is active on this task.
    // Absent outside any scope — those events get no trace fields, matching
    // pre-trace-context JSONL output bit-for-bit.
    let ctx = TraceContext::current();
    let attributes = derive_otel_attributes(event_type, channel, provider, model, &payload);
    let event = RuntimeTraceEvent {
        id: Uuid::new_v4().to_string(),
        timestamp: Utc::now().to_rfc3339(),
        event_type: event_type.to_string(),
        channel: channel.map(str::to_string),
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        turn_id: turn_id.map(str::to_string),
        success,
        message: message.map(str::to_string),
        trace_id: ctx.as_ref().map(|c| c.trace_id.clone()),
        span_id: ctx.as_ref().map(|c| c.span_id.clone()),
        parent_span_id: ctx.as_ref().and_then(|c| c.parent_span_id.clone()),
        payload,
        attributes,
    };

    if let Err(err) = logger.append(&event) {
        tracing::warn!("Failed to write runtime trace event: {err}");
    }
}

/// Derive OpenTelemetry GenAI semantic-convention attributes from a
/// runtime trace event's structured fields + payload.
///
/// Reference: <https://opentelemetry.io/docs/specs/semconv/gen-ai/>.
/// Emits a subset focused on what Langfuse / Phoenix / Logfire / Datadog
/// actually consume:
///
/// - `gen_ai.system` ← `provider`
/// - `gen_ai.request.model` ← `model`
/// - `gen_ai.operation.name` ← derived from `event_type`
/// - `gen_ai.usage.input_tokens` ← `payload.input_tokens` (llm events)
/// - `gen_ai.usage.output_tokens` ← `payload.output_tokens` (llm events)
/// - `gen_ai.tool.name` ← `payload.tool_name` (tool events)
/// - `gen_ai.tool.call.id` ← `payload.tool_call_id` (tool events)
/// - `plaw.channel` ← `channel` (plaw-namespaced, not in spec)
///
/// Returns an empty map for non-GenAI events (cron, channel, etc.) so
/// `attributes` is skip-serialized — pre-OTel events stay byte-identical
/// in the JSONL output.
fn derive_otel_attributes(
    event_type: &str,
    channel: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
    payload: &Value,
) -> BTreeMap<String, Value> {
    let mut attrs: BTreeMap<String, Value> = BTreeMap::new();

    let is_genai_event = matches!(
        event_type,
        "llm_request"
            | "llm_response"
            | "tool_call_start"
            | "tool_call"
            | "tool_call_result"
            | "tool_call_parse_issue"
            | "tool_loop_exhausted"
    );
    if !is_genai_event {
        return attrs;
    }

    if let Some(p) = provider {
        attrs.insert("gen_ai.system".into(), json!(p));
    }
    if let Some(m) = model {
        attrs.insert("gen_ai.request.model".into(), json!(m));
    }
    if let Some(c) = channel {
        attrs.insert("plaw.channel".into(), json!(c));
    }

    match event_type {
        "llm_request" | "llm_response" => {
            attrs.insert("gen_ai.operation.name".into(), json!("chat"));
            if let Some(it) = payload.get("input_tokens").filter(|v| !v.is_null()) {
                attrs.insert("gen_ai.usage.input_tokens".into(), it.clone());
            }
            if let Some(ot) = payload.get("output_tokens").filter(|v| !v.is_null()) {
                attrs.insert("gen_ai.usage.output_tokens".into(), ot.clone());
            }
            // Plaw-specific extension: surfaces the agent loop's iteration
            // counter so downstream consumers can correlate multi-iteration
            // turns without re-parsing the payload.
            if let Some(iter) = payload.get("iteration").filter(|v| !v.is_null()) {
                attrs.insert("plaw.iteration".into(), iter.clone());
            }
        }
        "tool_call_start" | "tool_call" | "tool_call_result" | "tool_call_parse_issue" => {
            attrs.insert("gen_ai.operation.name".into(), json!("execute_tool"));
            if let Some(name) = payload.get("tool_name").filter(|v| !v.is_null()) {
                attrs.insert("gen_ai.tool.name".into(), name.clone());
            }
            if let Some(id) = payload.get("tool_call_id").filter(|v| !v.is_null()) {
                attrs.insert("gen_ai.tool.call.id".into(), id.clone());
            }
        }
        "tool_loop_exhausted" => {
            // No tool-specific attrs; gen_ai.system + request.model carry
            // enough context for filtering.
        }
        _ => {}
    }

    attrs
}

/// Load recent runtime trace events from storage.
pub fn load_events(
    path: &Path,
    limit: usize,
    event_filter: Option<&str>,
    contains: Option<&str>,
) -> Result<Vec<RuntimeTraceEvent>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(path)?;
    let mut events = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<RuntimeTraceEvent>(trimmed) {
            Ok(event) => events.push(event),
            Err(err) => tracing::warn!("Skipping malformed runtime trace line: {err}"),
        }
    }

    if let Some(filter) = event_filter.map(str::trim).filter(|f| !f.is_empty()) {
        let normalized = filter.to_ascii_lowercase();
        events.retain(|event| event.event_type.to_ascii_lowercase() == normalized);
    }

    if let Some(needle) = contains.map(str::trim).filter(|s| !s.is_empty()) {
        let needle = needle.to_ascii_lowercase();
        events.retain(|event| {
            let mut haystack = format!(
                "{} {} {}",
                event.event_type,
                event.message.as_deref().unwrap_or_default(),
                event.payload
            );
            if let Some(channel) = &event.channel {
                haystack.push_str(channel);
            }
            if let Some(provider) = &event.provider {
                haystack.push_str(provider);
            }
            if let Some(model) = &event.model {
                haystack.push_str(model);
            }
            haystack.to_ascii_lowercase().contains(&needle)
        });
    }

    if events.len() > limit {
        let keep_from = events.len() - limit;
        events = events.split_off(keep_from);
    }

    events.reverse();
    Ok(events)
}

/// Find a runtime trace event by id.
pub fn find_event_by_id(path: &Path, id: &str) -> Result<Option<RuntimeTraceEvent>> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path)?;
    for line in raw.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<RuntimeTraceEvent>(trimmed) {
            if event.id == id {
                return Ok(Some(event));
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_observability_config() -> ObservabilityConfig {
        ObservabilityConfig {
            backend: "none".to_string(),
            otel_endpoint: None,
            otel_service_name: None,
            runtime_trace_mode: "rolling".to_string(),
            runtime_trace_path: "state/runtime-trace.jsonl".to_string(),
            runtime_trace_max_entries: 3,
        }
    }

    #[test]
    fn resolve_trace_path_relative_joins_workspace() {
        let cfg = test_observability_config();
        let workspace = tempfile::tempdir().unwrap();
        let path = resolve_trace_path(&cfg, workspace.path());
        assert_eq!(path, workspace.path().join("state/runtime-trace.jsonl"));
    }

    #[test]
    fn storage_mode_parses_known_values() {
        let mut cfg = test_observability_config();
        cfg.runtime_trace_mode = "none".into();
        assert_eq!(
            storage_mode_from_config(&cfg),
            RuntimeTraceStorageMode::None
        );

        cfg.runtime_trace_mode = "rolling".into();
        assert_eq!(
            storage_mode_from_config(&cfg),
            RuntimeTraceStorageMode::Rolling
        );

        cfg.runtime_trace_mode = "full".into();
        assert_eq!(
            storage_mode_from_config(&cfg),
            RuntimeTraceStorageMode::Full
        );
    }

    #[test]
    fn rolling_mode_keeps_latest_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let logger = RuntimeTraceLogger::new(RuntimeTraceStorageMode::Rolling, 2, path.clone());

        for i in 0..5 {
            let event = RuntimeTraceEvent {
                id: format!("id-{i}"),
                timestamp: Utc::now().to_rfc3339(),
                event_type: "test".into(),
                channel: None,
                provider: None,
                model: None,
                turn_id: None,
                success: None,
                attributes: BTreeMap::new(),
                message: Some(format!("event-{i}")),
                trace_id: None,
                span_id: None,
                parent_span_id: None,
                payload: serde_json::json!({ "i": i }),
            };
            logger.append(&event).unwrap();
        }

        let events = load_events(&path, 10, None, None).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message.as_deref(), Some("event-4"));
        assert_eq!(events[1].message.as_deref(), Some("event-3"));
    }

    #[test]
    fn find_event_by_id_returns_match() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let logger = RuntimeTraceLogger::new(RuntimeTraceStorageMode::Full, 100, path.clone());

        let target_id = "target-event";
        let event = RuntimeTraceEvent {
            id: target_id.into(),
            timestamp: Utc::now().to_rfc3339(),
            event_type: "tool_call_result".into(),
            channel: Some("telegram".into()),
            provider: Some("openrouter".into()),
            model: Some("x".into()),
            turn_id: Some("turn-1".into()),
            success: Some(false),
            message: Some("boom".into()),
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            payload: serde_json::json!({ "error": "boom" }),
            attributes: BTreeMap::new(),
        };
        logger.append(&event).unwrap();

        let found = find_event_by_id(&path, target_id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, target_id);
    }

    // ── OTel GenAI attribute derivation ─────────────────────────────

    #[test]
    fn derive_otel_attrs_returns_empty_for_non_genai_events() {
        let attrs = derive_otel_attributes(
            "cron_job_completed",
            Some("cron"),
            None,
            None,
            &json!({}),
        );
        assert!(attrs.is_empty());
    }

    #[test]
    fn derive_otel_attrs_for_llm_request_includes_system_model_operation() {
        let attrs = derive_otel_attributes(
            "llm_request",
            Some("webchat"),
            Some("anthropic"),
            Some("claude-sonnet-4-6"),
            &json!({"iteration": 3, "messages_count": 12}),
        );
        assert_eq!(attrs["gen_ai.system"], json!("anthropic"));
        assert_eq!(attrs["gen_ai.request.model"], json!("claude-sonnet-4-6"));
        assert_eq!(attrs["gen_ai.operation.name"], json!("chat"));
        assert_eq!(attrs["plaw.channel"], json!("webchat"));
        assert_eq!(attrs["plaw.iteration"], json!(3));
    }

    #[test]
    fn derive_otel_attrs_for_llm_response_includes_token_usage() {
        let attrs = derive_otel_attributes(
            "llm_response",
            Some("webchat"),
            Some("deepseek"),
            Some("deepseek-v4-pro"),
            &json!({
                "iteration": 1,
                "duration_ms": 1234,
                "input_tokens": 256,
                "output_tokens": 32,
                "raw_response": "..."
            }),
        );
        assert_eq!(attrs["gen_ai.usage.input_tokens"], json!(256));
        assert_eq!(attrs["gen_ai.usage.output_tokens"], json!(32));
        assert_eq!(attrs["gen_ai.operation.name"], json!("chat"));
    }

    #[test]
    fn derive_otel_attrs_omits_token_usage_when_payload_lacks_them() {
        // Some providers don't report tokens; the OTel attr should be
        // absent rather than null so consumers can distinguish
        // "not reported" from "zero".
        let attrs = derive_otel_attributes(
            "llm_response",
            Some("cli"),
            Some("ollama"),
            Some("qwen2.5:14b"),
            &json!({"iteration": 1, "duration_ms": 500}),
        );
        assert!(!attrs.contains_key("gen_ai.usage.input_tokens"));
        assert!(!attrs.contains_key("gen_ai.usage.output_tokens"));
        // Still emits the system / model attrs.
        assert_eq!(attrs["gen_ai.system"], json!("ollama"));
    }

    #[test]
    fn derive_otel_attrs_omits_token_usage_when_value_is_null() {
        let attrs = derive_otel_attributes(
            "llm_response",
            Some("cli"),
            Some("ollama"),
            Some("qwen2.5:14b"),
            &json!({"input_tokens": null, "output_tokens": null}),
        );
        assert!(!attrs.contains_key("gen_ai.usage.input_tokens"));
        assert!(!attrs.contains_key("gen_ai.usage.output_tokens"));
    }

    #[test]
    fn derive_otel_attrs_for_tool_events_includes_tool_attributes() {
        let attrs = derive_otel_attributes(
            "tool_call_start",
            Some("webchat"),
            Some("anthropic"),
            Some("claude-sonnet-4-6"),
            &json!({"tool_name": "shell", "tool_call_id": "call_xyz"}),
        );
        assert_eq!(attrs["gen_ai.operation.name"], json!("execute_tool"));
        assert_eq!(attrs["gen_ai.tool.name"], json!("shell"));
        assert_eq!(attrs["gen_ai.tool.call.id"], json!("call_xyz"));
    }

    #[test]
    fn derive_otel_attrs_for_tool_events_omits_missing_tool_fields() {
        let attrs = derive_otel_attributes(
            "tool_call_parse_issue",
            Some("webchat"),
            Some("openai"),
            Some("gpt-4o"),
            &json!({"iteration": 2}),
        );
        // Operation name still set; tool-specific fields absent.
        assert_eq!(attrs["gen_ai.operation.name"], json!("execute_tool"));
        assert!(!attrs.contains_key("gen_ai.tool.name"));
        assert!(!attrs.contains_key("gen_ai.tool.call.id"));
    }

    #[test]
    fn derive_otel_attrs_handles_missing_provider_and_model() {
        // Some events (channel-side, pre-provider-resolution) emit without
        // provider/model. Attributes should reflect that without panicking.
        let attrs = derive_otel_attributes(
            "llm_request",
            Some("telegram"),
            None,
            None,
            &json!({}),
        );
        assert_eq!(attrs["gen_ai.operation.name"], json!("chat"));
        assert!(!attrs.contains_key("gen_ai.system"));
        assert!(!attrs.contains_key("gen_ai.request.model"));
    }

    #[test]
    fn event_serialization_omits_empty_attributes() {
        // Pre-OTel events (cron, channel listener, etc.) had no `attributes`
        // field. Empty maps must be skipped on serialization so the JSONL
        // output stays byte-identical for non-GenAI events.
        let event = RuntimeTraceEvent {
            id: "fixed-id".into(),
            timestamp: "2026-01-01T00:00:00+00:00".into(),
            event_type: "cron_job_completed".into(),
            channel: Some("cron".into()),
            provider: None,
            model: None,
            turn_id: None,
            success: Some(true),
            message: None,
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            payload: json!({}),
            attributes: BTreeMap::new(),
        };
        let s = serde_json::to_string(&event).unwrap();
        assert!(!s.contains("\"attributes\""), "empty attributes should be omitted, got: {s}");
    }

    #[test]
    fn event_serialization_includes_populated_attributes() {
        let mut attrs = BTreeMap::new();
        attrs.insert("gen_ai.system".into(), json!("anthropic"));
        attrs.insert("gen_ai.request.model".into(), json!("claude-sonnet-4-6"));
        let event = RuntimeTraceEvent {
            id: "x".into(),
            timestamp: "2026-01-01T00:00:00+00:00".into(),
            event_type: "llm_request".into(),
            channel: None,
            provider: Some("anthropic".into()),
            model: Some("claude-sonnet-4-6".into()),
            turn_id: None,
            success: None,
            message: None,
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            payload: json!({}),
            attributes: attrs,
        };
        let s = serde_json::to_string(&event).unwrap();
        assert!(s.contains("\"attributes\""));
        assert!(s.contains("\"gen_ai.system\":\"anthropic\""));
        assert!(s.contains("\"gen_ai.request.model\":\"claude-sonnet-4-6\""));
    }

    #[test]
    fn event_attributes_field_round_trips_through_serde() {
        let mut attrs = BTreeMap::new();
        attrs.insert("gen_ai.system".into(), json!("openai"));
        attrs.insert("gen_ai.usage.input_tokens".into(), json!(100));
        let event = RuntimeTraceEvent {
            id: "y".into(),
            timestamp: "2026-01-01T00:00:00+00:00".into(),
            event_type: "llm_response".into(),
            channel: None,
            provider: Some("openai".into()),
            model: Some("gpt-4o".into()),
            turn_id: None,
            success: Some(true),
            message: None,
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            payload: json!({}),
            attributes: attrs,
        };
        let s = serde_json::to_string(&event).unwrap();
        let parsed: RuntimeTraceEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.attributes.len(), 2);
        assert_eq!(parsed.attributes["gen_ai.system"], json!("openai"));
        assert_eq!(parsed.attributes["gen_ai.usage.input_tokens"], json!(100));
    }

    #[test]
    fn legacy_event_without_attributes_field_deserializes_via_default() {
        // Pre-OTel JSONL files don't have the `attributes` field. The
        // reader must default it to an empty map, not fail to parse.
        let body = r#"{
            "id": "z",
            "timestamp": "2026-01-01T00:00:00+00:00",
            "event_type": "llm_request",
            "channel": "cli",
            "provider": "anthropic",
            "model": "claude-sonnet-4-6",
            "payload": {}
        }"#;
        let parsed: RuntimeTraceEvent = serde_json::from_str(body).unwrap();
        assert!(parsed.attributes.is_empty());
    }
}
