use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Sender for real-time progress messages during tool execution.
pub type ProgressTx = mpsc::Sender<String>;

/// Result of a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Optional structured representation of a tool's output, carried alongside the
/// canonical [`ToolResult`] text form. Tools that produce well-typed payloads
/// (JSON, base64-encoded binary, etc.) can override [`Tool::execute_typed`] to
/// expose this variant; consumers that want typed access call `execute_typed`
/// instead of `execute`.
///
/// Variants intentionally cover only the payload kinds in use today.
/// Streaming is *not* a variant: real-time progress is delivered via
/// [`ProgressTx`] in [`Tool::execute_with_progress`]; the final value still
/// fits one of these shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolResultValue {
    /// Free-text or markdown payload — the default human-readable form.
    Text { content: String },
    /// Structured JSON payload — for tools like `git_operations`, `http_request`,
    /// and `cron_list` that already construct typed data internally.
    Json { data: serde_json::Value },
    /// Binary payload, base64-encoded with a MIME type — for tools like
    /// `screenshot` and `image_info` that return image bytes.
    Bytes { mime: String, base64: String },
}

/// [`ToolResult`] paired with an optional [`ToolResultValue`]. Returned from
/// [`Tool::execute_typed`]. The default implementation of `execute_typed`
/// returns `value: None`; tools that want to expose a typed value override.
#[derive(Debug, Clone)]
pub struct TypedToolResult {
    pub result: ToolResult,
    pub value: Option<ToolResultValue>,
}

impl TypedToolResult {
    /// Construct a typed result from a plain [`ToolResult`] with no structured value.
    pub fn untyped(result: ToolResult) -> Self {
        Self {
            result,
            value: None,
        }
    }
}

/// Description of a tool for the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Replay safety of a tool. Read by retry / audit consumers to decide
/// whether re-running with the same args is safe.
///
/// Default for any tool that doesn't override [`Tool::idempotency`] is
/// [`Idempotency::Unknown`] — audit code should treat unknown as
/// non-idempotent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Idempotency {
    /// Replaying with the same args produces the same observable result
    /// (e.g. `file_read`, `glob_search`, GET-only `http_request`).
    Idempotent,
    /// Replaying is idempotent **with respect to the given key** — e.g.
    /// `file_write` to the same path twice ends with the same file
    /// content, even if filesystem timestamps differ. The string names
    /// the key (typically a parameter path like `"path"`).
    IdempotentByKey { key: String },
    /// Replaying may produce a different result or duplicate side effects
    /// (e.g. `shell`, `cron_add`, anything that mutates external state
    /// without a natural dedup key).
    NonIdempotent,
    /// Tool has not declared its replay semantics. Audit code should treat
    /// as [`NonIdempotent`].
    Unknown,
}

/// Coarse-grained side-effect category. Read by audit / sandbox / cost
/// consumers; the LLM does not see this.
///
/// Default is [`SideEffectClass::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    /// No state mutation; safe to call concurrently and freely.
    ReadOnly,
    /// Mutates local filesystem or in-process memory.
    LocalWrite,
    /// Spawns or controls a local subprocess (shell, git, patch).
    LocalExecute,
    /// Performs a write to a remote network endpoint (POST/PUT/DELETE,
    /// push notifications, third-party APIs that mutate).
    NetworkWrite,
    /// Spawns a sub-agent or scheduled job — fans out to additional tool
    /// execution chains.
    Spawn,
    /// Tool has not declared. Audit code should treat as the strictest
    /// non-read class.
    Unknown,
}

/// Core tool trait — implement for any capability
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used in LLM function calling)
    fn name(&self) -> &str;

    /// Human-readable description
    fn description(&self) -> &str;

    /// JSON schema for parameters
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with given arguments
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult>;

    /// Execute with a progress channel for real-time status updates.
    /// Override this for long-running tools (browser, shell) to send progress.
    /// Default: calls `execute()` with no progress.
    async fn execute_with_progress(
        &self,
        args: serde_json::Value,
        _progress: ProgressTx,
    ) -> anyhow::Result<ToolResult> {
        self.execute(args).await
    }

    /// Execute and return the result paired with an optional structured value.
    ///
    /// Default delegates to [`Self::execute`] and returns `value: None` —
    /// callers should treat [`ToolResult::output`] as text.
    ///
    /// Tools whose outputs are already typed (JSON via `serde_json::Value`,
    /// base64-encoded binary, etc.) should override this so consumers can
    /// route on the payload kind without re-parsing the `output` string.
    ///
    /// The canonical text form in `result.output` must remain populated even
    /// when `value` is `Some(...)`; the LLM-facing path and existing log /
    /// scrub / inject scanning pipelines all read `output`.
    async fn execute_typed(&self, args: serde_json::Value) -> anyhow::Result<TypedToolResult> {
        let result = self.execute(args).await?;
        Ok(TypedToolResult::untyped(result))
    }

    /// Validate `args` against `parameters_schema()` and short-circuit with a
    /// structured `ToolResult` error before [`Self::execute`] runs if they
    /// don't conform. On validation success, delegates to [`Self::execute`].
    ///
    /// This is the call site the dispatcher should use — it gives the LLM a
    /// machine-readable error so it can retry with corrected args instead of
    /// guessing what `"Missing 'command' parameter"` meant. Tool authors do
    /// not implement this; the default body covers all cases.
    async fn execute_validated(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if let Err(failures) =
            crate::tools::validation::validate_against_schema(&args, &self.parameters_schema())
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(crate::tools::validation::render_validation_error(&failures)),
            });
        }
        self.execute(args).await
    }

    /// Get the full spec for LLM registration
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }

    /// Tool's interface version. Bumped when `parameters_schema()` changes
    /// shape in a way callers must adapt to. Default `"1.0"` for tools
    /// that haven't versioned their schema. Not currently consumed —
    /// reserved for future audit / replay / cache-invalidation logic.
    fn version(&self) -> &str {
        "1.0"
    }

    /// Replay safety. See [`Idempotency`] variants. Default
    /// [`Idempotency::Unknown`]; tools that mutate state should
    /// override to [`Idempotency::NonIdempotent`], pure-read tools
    /// to [`Idempotency::Idempotent`].
    ///
    /// **Not yet consumed.** Future audit/retry layers will read this
    /// to decide whether a failed-but-may-have-applied tool is safe to
    /// re-execute. Declaring it now lets tool authors record intent
    /// rather than having to reverse-engineer it later.
    fn idempotency(&self) -> Idempotency {
        Idempotency::Unknown
    }

    /// Side-effect class. See [`SideEffectClass`] variants. Default
    /// [`SideEffectClass::Unknown`]; tools should override to one of
    /// the concrete variants.
    ///
    /// **Not yet consumed.** Future sandbox-selection, dry-run, and
    /// audit code will branch on this — e.g. only `ReadOnly` tools may
    /// run during a research-phase dry-run.
    fn side_effects(&self) -> SideEffectClass {
        SideEffectClass::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy_tool"
        }

        fn description(&self) -> &str {
            "A deterministic test tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                }
            })
        }

        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            Ok(ToolResult {
                success: true,
                output: args
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                error: None,
            })
        }
    }

    #[test]
    fn spec_uses_tool_metadata_and_schema() {
        let tool = DummyTool;
        let spec = tool.spec();

        assert_eq!(spec.name, "dummy_tool");
        assert_eq!(spec.description, "A deterministic test tool");
        assert_eq!(spec.parameters["type"], "object");
        assert_eq!(spec.parameters["properties"]["value"]["type"], "string");
    }

    #[tokio::test]
    async fn execute_returns_expected_output() {
        let tool = DummyTool;
        let result = tool
            .execute(serde_json::json!({ "value": "hello-tool" }))
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output, "hello-tool");
        assert!(result.error.is_none());
    }

    #[test]
    fn default_idempotency_is_unknown() {
        let tool = DummyTool;
        assert_eq!(tool.idempotency(), Idempotency::Unknown);
    }

    #[test]
    fn default_side_effects_is_unknown() {
        let tool = DummyTool;
        assert_eq!(tool.side_effects(), SideEffectClass::Unknown);
    }

    #[test]
    fn default_version_is_one_dot_zero() {
        let tool = DummyTool;
        assert_eq!(tool.version(), "1.0");
    }

    #[test]
    fn idempotency_serializes_with_tag() {
        let by_key = Idempotency::IdempotentByKey { key: "path".into() };
        let json = serde_json::to_value(&by_key).unwrap();
        assert_eq!(json["kind"], "idempotent_by_key");
        assert_eq!(json["key"], "path");

        let plain = Idempotency::Idempotent;
        let json = serde_json::to_value(&plain).unwrap();
        assert_eq!(json["kind"], "idempotent");
    }

    #[test]
    fn side_effect_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(SideEffectClass::ReadOnly).unwrap(),
            serde_json::json!("read_only")
        );
        assert_eq!(
            serde_json::to_value(SideEffectClass::LocalExecute).unwrap(),
            serde_json::json!("local_execute")
        );
        assert_eq!(
            serde_json::to_value(SideEffectClass::NetworkWrite).unwrap(),
            serde_json::json!("network_write")
        );
    }

    #[test]
    fn tool_result_serialization_roundtrip() {
        let result = ToolResult {
            success: false,
            output: String::new(),
            error: Some("boom".into()),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();

        assert!(!parsed.success);
        assert_eq!(parsed.error.as_deref(), Some("boom"));
    }

    #[test]
    fn tool_result_value_text_roundtrip() {
        let v = ToolResultValue::Text {
            content: "hello".into(),
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["kind"], "text");
        assert_eq!(json["content"], "hello");

        let parsed: ToolResultValue = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, v);
    }

    #[test]
    fn tool_result_value_json_roundtrip() {
        let v = ToolResultValue::Json {
            data: serde_json::json!({"branch": "main", "clean": true}),
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["kind"], "json");
        assert_eq!(json["data"]["branch"], "main");
        assert_eq!(json["data"]["clean"], true);

        let parsed: ToolResultValue = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, v);
    }

    #[test]
    fn tool_result_value_bytes_roundtrip() {
        let v = ToolResultValue::Bytes {
            mime: "image/png".into(),
            base64: "iVBORw0KGgo".into(),
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["kind"], "bytes");
        assert_eq!(json["mime"], "image/png");
        assert_eq!(json["base64"], "iVBORw0KGgo");

        let parsed: ToolResultValue = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, v);
    }

    #[tokio::test]
    async fn default_execute_typed_returns_no_value() {
        let tool = DummyTool;
        let typed = tool
            .execute_typed(serde_json::json!({ "value": "x" }))
            .await
            .unwrap();
        assert!(typed.result.success);
        assert_eq!(typed.result.output, "x");
        assert!(typed.value.is_none());
    }

    #[test]
    fn typed_tool_result_untyped_constructor() {
        let r = ToolResult {
            success: true,
            output: "ok".into(),
            error: None,
        };
        let typed = TypedToolResult::untyped(r);
        assert!(typed.value.is_none());
        assert_eq!(typed.result.output, "ok");
    }
}
