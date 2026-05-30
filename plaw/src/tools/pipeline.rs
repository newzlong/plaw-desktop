//! `run_pipeline` tool — LLM-callable entry into the pre-configured
//! multi-stage workflows declared under `[pipelines.*]`.
//!
//! Each invocation looks up the named pipeline in the config registry
//! and runs it via [`crate::agent::pipeline::run_pipeline`], which
//! dispatches each stage through the shared [`crate::tools::DelegateTool`]
//! instance. The pipeline result's final-stage output is returned;
//! the full blackboard is appended for transparency.

use crate::agent::pipeline::run_pipeline;
use crate::config::PipelineConfig;
use crate::tools::traits::{
    SideEffectClass, Tool, ToolResult, ToolResultValue, TypedToolResult,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Tool exposing pipeline invocation to the agent loop's LLM.
///
/// Constructed once at tool-registry setup time when at least one
/// `[pipelines.*]` entry is configured. Holds an `Arc<dyn Tool>`
/// pointing at the same [`crate::tools::DelegateTool`] instance the
/// main LLM uses directly, so pipeline-driven and LLM-driven delegate
/// calls share the trust boundary, coordination bus, and security policy.
pub struct PipelineTool {
    pipelines: Arc<HashMap<String, PipelineConfig>>,
    delegate_tool: Arc<dyn Tool>,
}

impl PipelineTool {
    pub fn new(
        pipelines: Arc<HashMap<String, PipelineConfig>>,
        delegate_tool: Arc<dyn Tool>,
    ) -> Self {
        Self {
            pipelines,
            delegate_tool,
        }
    }

    /// Sorted list of available pipeline names — surfaced in the
    /// tool description so the LLM knows what's invokable.
    fn pipeline_names_sorted(&self) -> Vec<String> {
        let mut names: Vec<String> = self.pipelines.keys().cloned().collect();
        names.sort();
        names
    }
}

#[async_trait]
impl Tool for PipelineTool {
    fn name(&self) -> &str {
        "run_pipeline"
    }

    fn description(&self) -> &str {
        // The static description doesn't enumerate pipeline names
        // (those vary per config); the LLM can call this tool
        // optimistically — invalid names surface as a clear error.
        "Run a pre-configured deterministic multi-stage workflow. Each \
         pipeline declared under `[pipelines.<name>]` in config.toml \
         routes through multiple sub-agents (planner → researcher → coder \
         → reporter, or any user-defined sequence). Use when the user \
         asks for a complex task that benefits from explicit planning \
         and structured handoff between specialized agents. Returns the \
         final stage's output."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let available = self.pipeline_names_sorted();
        let pipeline_param = if available.is_empty() {
            // No pipelines configured — still expose the schema so
            // the LLM gets a sensible error if it tries.
            serde_json::json!({
                "type": "string",
                "description": "Name of the pipeline to run. \
                    No pipelines are currently configured under \
                    `[pipelines.*]` in config.toml."
            })
        } else {
            serde_json::json!({
                "type": "string",
                "description": format!(
                    "Name of the pipeline to run. Available: {}.",
                    available.join(", ")
                ),
                "enum": available,
            })
        };
        serde_json::json!({
            "type": "object",
            "properties": {
                "pipeline_name": pipeline_param,
                "user_message": {
                    "type": "string",
                    "description": "Initial user-facing task description \
                        passed to the pipeline's first stage and available \
                        in every stage's prompt via {user_message}."
                }
            },
            "required": ["pipeline_name", "user_message"]
        })
    }

    fn side_effects(&self) -> SideEffectClass {
        // Pipelines fan out to sub-agents which may themselves write,
        // spawn, or call network. Conservative classification.
        SideEffectClass::Spawn
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let Some(pipeline_name) = args.get("pipeline_name").and_then(|v| v.as_str()) else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("missing required `pipeline_name` parameter".into()),
            });
        };
        let Some(user_message) = args.get("user_message").and_then(|v| v.as_str()) else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("missing required `user_message` parameter".into()),
            });
        };

        let Some(pipeline) = self.pipelines.get(pipeline_name) else {
            let mut names = self.pipeline_names_sorted();
            let msg = if names.is_empty() {
                format!(
                    "pipeline '{pipeline_name}' not found and no pipelines are \
                     configured. Add an entry under `[pipelines.<name>]` in \
                     config.toml."
                )
            } else {
                names.truncate(10);
                format!(
                    "pipeline '{pipeline_name}' not found. Available: {}",
                    names.join(", ")
                )
            };
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(msg),
            });
        };

        match run_pipeline(
            pipeline_name,
            pipeline,
            user_message,
            self.delegate_tool.as_ref(),
        )
        .await
        {
            Ok(result) => {
                let formatted = format!(
                    "[pipeline:{} stages_run={}]\n{}",
                    result.pipeline_name, result.stages_run, result.final_output
                );
                Ok(ToolResult {
                    success: true,
                    output: formatted,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("pipeline '{pipeline_name}' failed: {e}")),
            }),
        }
    }

    /// Expose the per-stage blackboard via the structured typed value
    /// channel (alongside the human-readable `output`). Future UI
    /// consumers can render an activity log without re-parsing the
    /// formatted string.
    async fn execute_typed(
        &self,
        args: serde_json::Value,
    ) -> anyhow::Result<TypedToolResult> {
        let pipeline_name = args
            .get("pipeline_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let user_message = args
            .get("user_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if pipeline_name.is_empty() || user_message.is_empty() {
            // Fall through to the plain execute() to get its parameter-error
            // surface — typed wrapper shouldn't duplicate that logic.
            let result = self.execute(args).await?;
            return Ok(TypedToolResult::untyped(result));
        }

        let Some(pipeline) = self.pipelines.get(&pipeline_name) else {
            let result = self.execute(args).await?;
            return Ok(TypedToolResult::untyped(result));
        };

        match run_pipeline(
            &pipeline_name,
            pipeline,
            &user_message,
            self.delegate_tool.as_ref(),
        )
        .await
        {
            Ok(result) => {
                let formatted = format!(
                    "[pipeline:{} stages_run={}]\n{}",
                    result.pipeline_name, result.stages_run, result.final_output
                );
                let value = ToolResultValue::Json {
                    data: serde_json::json!({
                        "pipeline_name": result.pipeline_name,
                        "stages_run": result.stages_run,
                        "final_output": result.final_output,
                        "blackboard": result.blackboard,
                    }),
                };
                Ok(TypedToolResult {
                    result: ToolResult {
                        success: true,
                        output: formatted,
                        error: None,
                    },
                    value: Some(value),
                })
            }
            Err(e) => {
                let result = ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("pipeline '{pipeline_name}' failed: {e}")),
                };
                Ok(TypedToolResult::untyped(result))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PipelineConfig, PipelineErrorPolicy, PipelineStage};
    use crate::tools::traits::ToolResult;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct EchoDelegate {
        calls: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl Tool for EchoDelegate {
        fn name(&self) -> &str {
            "delegate"
        }
        fn description(&self) -> &str {
            "echo delegate for pipeline tool tests"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {"type": "string"},
                    "prompt": {"type": "string"},
                },
                "required": ["agent", "prompt"]
            })
        }
        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            self.calls.lock().unwrap().push(args.clone());
            Ok(ToolResult {
                success: true,
                output: format!(
                    "echo:{}:{}",
                    args.get("agent").and_then(|v| v.as_str()).unwrap_or(""),
                    args.get("prompt").and_then(|v| v.as_str()).unwrap_or("")
                ),
                error: None,
            })
        }
    }

    fn sample_pipelines() -> HashMap<String, PipelineConfig> {
        let mut m = HashMap::new();
        m.insert(
            "plan_then_execute".to_string(),
            PipelineConfig {
                stages: vec![
                    PipelineStage {
                        agent: "planner".into(),
                        prompt: "Plan: {user_message}".into(),
                        output_key: "plan".into(),
                        context: None,
                    },
                    PipelineStage {
                        agent: "coder".into(),
                        prompt: "Execute: {prior.plan}".into(),
                        output_key: "code".into(),
                        context: None,
                    },
                ],
                on_error: PipelineErrorPolicy::Abort,
            },
        );
        m
    }

    fn build_tool() -> (PipelineTool, Arc<EchoDelegate>) {
        let delegate = Arc::new(EchoDelegate {
            calls: Mutex::new(Vec::new()),
        });
        let delegate_as_tool: Arc<dyn Tool> = delegate.clone();
        let tool = PipelineTool::new(Arc::new(sample_pipelines()), delegate_as_tool);
        (tool, delegate)
    }

    #[test]
    fn tool_metadata_matches_expected_shape() {
        let (tool, _) = build_tool();
        assert_eq!(tool.name(), "run_pipeline");
        assert!(tool.description().contains("multi-stage workflow"));
        assert_eq!(tool.side_effects(), SideEffectClass::Spawn);
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["pipeline_name"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "plan_then_execute"));
    }

    #[tokio::test]
    async fn execute_runs_named_pipeline_end_to_end() {
        let (tool, delegate) = build_tool();
        let result = tool
            .execute(serde_json::json!({
                "pipeline_name": "plan_then_execute",
                "user_message": "build a widget"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.starts_with("[pipeline:plan_then_execute stages_run=2]"));
        assert!(result.output.contains("echo:coder:Execute:"));

        let calls = delegate.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["agent"], "planner");
        assert_eq!(calls[1]["prompt"], "Execute: echo:planner:Plan: build a widget");
    }

    #[tokio::test]
    async fn execute_errors_when_pipeline_name_is_unknown() {
        let (tool, _) = build_tool();
        let result = tool
            .execute(serde_json::json!({
                "pipeline_name": "ghost",
                "user_message": "x"
            }))
            .await
            .unwrap();
        // Tool contract: parameter errors come back as success=false,
        // not as a propagated anyhow error (the LLM gets the message
        // and can retry with a corrected name).
        assert!(!result.success);
        let err = result.error.unwrap_or_default();
        assert!(err.contains("pipeline 'ghost' not found"));
        assert!(err.contains("plan_then_execute"));
    }

    #[tokio::test]
    async fn execute_errors_when_required_args_missing() {
        let (tool, _) = build_tool();
        let result = tool
            .execute(serde_json::json!({"pipeline_name": "plan_then_execute"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("user_message"));
    }

    #[tokio::test]
    async fn execute_typed_surfaces_blackboard_via_json_variant() {
        let (tool, _) = build_tool();
        let typed = tool
            .execute_typed(serde_json::json!({
                "pipeline_name": "plan_then_execute",
                "user_message": "go"
            }))
            .await
            .unwrap();
        assert!(typed.result.success);
        let Some(ToolResultValue::Json { data }) = typed.value else {
            panic!("expected Json variant, got {:?}", typed.value);
        };
        assert_eq!(data["pipeline_name"], "plan_then_execute");
        assert_eq!(data["stages_run"], 2);
        assert!(data["blackboard"]["plan"]
            .as_str()
            .unwrap()
            .starts_with("echo:planner:"));
        assert!(data["blackboard"]["code"]
            .as_str()
            .unwrap()
            .starts_with("echo:coder:"));
    }

    #[tokio::test]
    async fn empty_pipelines_map_still_exposes_schema() {
        let delegate: Arc<dyn Tool> = Arc::new(EchoDelegate {
            calls: Mutex::new(Vec::new()),
        });
        let tool = PipelineTool::new(Arc::new(HashMap::new()), delegate);
        let schema = tool.parameters_schema();
        // No enum constraint when registry is empty.
        assert!(schema["properties"]["pipeline_name"].get("enum").is_none());
        // Tool can still be called — should fail gracefully.
        let result = tool
            .execute(serde_json::json!({
                "pipeline_name": "anything",
                "user_message": "x"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .unwrap()
            .contains("no pipelines are configured"));
    }
}
