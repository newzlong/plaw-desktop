//! Deterministic multi-stage workflow runner.
//!
//! Implements DeerFlow v1's plan-then-execute pattern as a pure-Rust
//! orchestrator over plaw's existing [`crate::tools::DelegateTool`]:
//! a [`PipelineConfig`] declares an ordered list of stages, each
//! stage routes to a named agent in the `[agents.*]` registry with a
//! prompt template, and stages share a string-keyed blackboard that
//! later stages reference via `{prior.<output_key>}` placeholders.
//!
//! Design choices vs. DeerFlow v1 (per the
//! `deerflow-pattern-discovery` workflow, 2026-05-30):
//!
//! - **No hardcoded role enum** — agent names are freeform strings
//!   matching the `[agents.*]` registry keys. Plaw's
//!   model-agnostic + provider-agnostic invariants extend to
//!   "role-agnostic" here: planners, researchers, coders, reporters,
//!   critics, translators, etc. are all just config entries.
//!
//! - **No graph library** — stages run in a flat `for` loop. The
//!   blackboard `HashMap<String, String>` is the only "state".
//!   Conditional / DAG-shaped pipelines are a Phase 2 concern.
//!
//! - **DelegateTool, not a new runtime** — pipelines invoke the same
//!   delegate tool the main LLM calls, sharing the full registry +
//!   security policy + coordination bus + multimodal config. This
//!   keeps the trust boundary identical between LLM-driven delegate
//!   calls and pipeline-driven ones.

use crate::config::{PipelineConfig, PipelineErrorPolicy, PipelineStage};
use crate::tools::Tool;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// Result of a single pipeline invocation.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Name of the pipeline that ran (for tracing / logging).
    pub pipeline_name: String,
    /// All stage outputs, keyed by each stage's `output_key`. Final
    /// stage's value is also surfaced as `final_output`; intermediate
    /// stages are exposed for callers that want to inspect (or for
    /// future UI to render the activity log).
    pub blackboard: HashMap<String, String>,
    /// Final stage's output. Convenience accessor — equivalent to
    /// `blackboard.get(&last_stage.output_key)`.
    pub final_output: String,
    /// Number of stages that ran. With `on_error: abort`, this may be
    /// less than `pipeline.stages.len()` when a stage failed.
    pub stages_run: usize,
}

/// Substitute `{user_message}` and `{prior.<key>}` placeholders in a
/// template against the current blackboard.
///
/// Unknown placeholders are left untouched (rendered literally) — this
/// is intentional so misnamed `{prior.foo}` references surface in the
/// rendered prompt rather than silently disappearing, which makes
/// debugging miswritten pipelines easier.
pub(crate) fn render_template(
    template: &str,
    user_message: &str,
    blackboard: &HashMap<String, String>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            // Unclosed '{' — emit literally and stop scanning.
            out.push_str(&rest[open..]);
            return out;
        };
        let key = &after_open[..close];
        let after_close = &after_open[close + 1..];
        let replacement: Option<&str> = if key == "user_message" {
            Some(user_message)
        } else if let Some(blackboard_key) = key.strip_prefix("prior.") {
            blackboard.get(blackboard_key).map(String::as_str)
        } else {
            None
        };
        match replacement {
            Some(value) => out.push_str(value),
            None => {
                // Unknown placeholder — keep verbatim including braces.
                out.push('{');
                out.push_str(key);
                out.push('}');
            }
        }
        rest = after_close;
    }
    out.push_str(rest);
    out
}

/// Run a pipeline end-to-end.
///
/// `delegate_tool` is invoked once per stage with the rendered prompt
/// (+ optional context). The first stage failure under
/// `PipelineErrorPolicy::Abort` short-circuits; under `Continue` the
/// failure is recorded as the stage's blackboard entry (prefixed with
/// "ERROR: ") and the pipeline proceeds.
pub async fn run_pipeline(
    pipeline_name: &str,
    pipeline: &PipelineConfig,
    user_message: &str,
    delegate_tool: &dyn Tool,
) -> Result<PipelineResult> {
    if pipeline.stages.is_empty() {
        anyhow::bail!("pipeline '{pipeline_name}' has no stages");
    }
    validate_pipeline(pipeline_name, pipeline)?;

    let mut blackboard: HashMap<String, String> = HashMap::new();
    let mut stages_run = 0usize;
    let mut last_output: String = String::new();

    for (idx, stage) in pipeline.stages.iter().enumerate() {
        let rendered_prompt = render_template(&stage.prompt, user_message, &blackboard);
        let rendered_context = stage
            .context
            .as_deref()
            .map(|t| render_template(t, user_message, &blackboard));

        let mut args = serde_json::Map::new();
        args.insert("agent".to_string(), serde_json::json!(stage.agent));
        args.insert("prompt".to_string(), serde_json::json!(rendered_prompt));
        if let Some(ctx) = rendered_context {
            args.insert("context".to_string(), serde_json::json!(ctx));
        }
        let tool_args = serde_json::Value::Object(args);

        tracing::info!(
            pipeline = %pipeline_name,
            stage_idx = idx,
            agent = %stage.agent,
            output_key = %stage.output_key,
            "pipeline stage starting"
        );

        let result = delegate_tool.execute_validated(tool_args).await;
        stages_run += 1;

        match result {
            Ok(ref r) if r.success => {
                blackboard.insert(stage.output_key.clone(), r.output.clone());
                last_output = r.output.clone();
                tracing::info!(
                    pipeline = %pipeline_name,
                    stage_idx = idx,
                    output_chars = r.output.chars().count(),
                    "pipeline stage completed"
                );
            }
            Ok(r) => {
                let err = r.error.unwrap_or_else(|| "(no error message)".to_string());
                tracing::warn!(
                    pipeline = %pipeline_name,
                    stage_idx = idx,
                    error = %err,
                    "pipeline stage failed (delegate returned failure)"
                );
                match pipeline.on_error {
                    PipelineErrorPolicy::Abort => {
                        return Err(anyhow!(
                            "pipeline '{pipeline_name}' stage {idx} (agent '{}') \
                             failed: {err}",
                            stage.agent
                        ));
                    }
                    PipelineErrorPolicy::Continue => {
                        let msg = format!("ERROR: {err}");
                        blackboard.insert(stage.output_key.clone(), msg.clone());
                        last_output = msg;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    pipeline = %pipeline_name,
                    stage_idx = idx,
                    error = %e,
                    "pipeline stage raised error"
                );
                match pipeline.on_error {
                    PipelineErrorPolicy::Abort => {
                        return Err(anyhow!(
                            "pipeline '{pipeline_name}' stage {idx} (agent '{}') \
                             raised: {e}",
                            stage.agent
                        ));
                    }
                    PipelineErrorPolicy::Continue => {
                        let msg = format!("ERROR: {e}");
                        blackboard.insert(stage.output_key.clone(), msg.clone());
                        last_output = msg;
                    }
                }
            }
        }
    }

    Ok(PipelineResult {
        pipeline_name: pipeline_name.to_string(),
        blackboard,
        final_output: last_output,
        stages_run,
    })
}

/// Static validation: stages must have non-empty agent + prompt +
/// output_key, and output_keys must be unique within the pipeline.
/// Misnamed `{prior.<key>}` placeholders are NOT validated statically
/// — they render literally so the debugging output shows the typo.
fn validate_pipeline(name: &str, pipeline: &PipelineConfig) -> Result<()> {
    let mut seen_keys: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (idx, stage) in pipeline.stages.iter().enumerate() {
        if stage.agent.trim().is_empty() {
            anyhow::bail!("pipeline '{name}' stage {idx}: empty `agent` field");
        }
        if stage.prompt.trim().is_empty() {
            anyhow::bail!("pipeline '{name}' stage {idx}: empty `prompt` field");
        }
        if stage.output_key.trim().is_empty() {
            anyhow::bail!("pipeline '{name}' stage {idx}: empty `output_key` field");
        }
        if !seen_keys.insert(stage.output_key.as_str()) {
            anyhow::bail!(
                "pipeline '{name}' stage {idx}: duplicate `output_key` '{}' \
                 (earlier stage already uses it)",
                stage.output_key
            );
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn stage_count(pipeline: &PipelineConfig) -> usize {
    pipeline.stages.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::traits::{ToolResult, ToolSpec};
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn stage(agent: &str, prompt: &str, output_key: &str) -> PipelineStage {
        PipelineStage {
            agent: agent.to_string(),
            prompt: prompt.to_string(),
            output_key: output_key.to_string(),
            context: None,
        }
    }

    /// Mock delegate tool that records every `execute` call and returns
    /// a scripted response per call index. Lets tests assert the exact
    /// prompts that flow through the pipeline.
    struct ScriptedDelegate {
        responses: Mutex<Vec<Result<ToolResult, anyhow::Error>>>,
        calls: Mutex<Vec<serde_json::Value>>,
    }

    impl ScriptedDelegate {
        fn from_outputs(outputs: Vec<&str>) -> Self {
            let responses = outputs
                .into_iter()
                .map(|s| {
                    Ok(ToolResult {
                        success: true,
                        output: s.to_string(),
                        error: None,
                    })
                })
                .collect();
            Self {
                responses: Mutex::new(responses),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<serde_json::Value> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Tool for ScriptedDelegate {
        fn name(&self) -> &str {
            "delegate"
        }
        fn description(&self) -> &str {
            "scripted delegate for pipeline tests"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {"type": "string"},
                    "prompt": {"type": "string"},
                    "context": {"type": "string"},
                },
                "required": ["agent", "prompt"]
            })
        }
        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            self.calls.lock().unwrap().push(args);
            let mut q = self.responses.lock().unwrap();
            if q.is_empty() {
                anyhow::bail!("ScriptedDelegate: ran out of scripted responses");
            }
            q.remove(0)
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name().to_string(),
                description: self.description().to_string(),
                parameters: self.parameters_schema(),
            }
        }
    }

    // ── render_template ──────────────────────────────────────────

    #[test]
    fn render_template_substitutes_user_message() {
        let bb: HashMap<String, String> = HashMap::new();
        let out = render_template("Hello, {user_message}!", "world", &bb);
        assert_eq!(out, "Hello, world!");
    }

    #[test]
    fn render_template_substitutes_prior_keys() {
        let mut bb = HashMap::new();
        bb.insert("plan".to_string(), "step 1, step 2".to_string());
        let out = render_template("Execute: {prior.plan}", "ignored", &bb);
        assert_eq!(out, "Execute: step 1, step 2");
    }

    #[test]
    fn render_template_leaves_unknown_placeholders_intact() {
        // Misnamed placeholders should render verbatim — debugging aid.
        let bb: HashMap<String, String> = HashMap::new();
        let out = render_template("X={prior.missing} Y={typo}", "u", &bb);
        assert_eq!(out, "X={prior.missing} Y={typo}");
    }

    #[test]
    fn render_template_handles_unclosed_brace_gracefully() {
        let bb: HashMap<String, String> = HashMap::new();
        let out = render_template("abc {unclosed", "u", &bb);
        assert_eq!(out, "abc {unclosed");
    }

    #[test]
    fn render_template_supports_multiple_placeholders_in_one_template() {
        let mut bb = HashMap::new();
        bb.insert("a".to_string(), "1".to_string());
        bb.insert("b".to_string(), "2".to_string());
        let out = render_template(
            "u={user_message} a={prior.a} b={prior.b}",
            "u_val",
            &bb,
        );
        assert_eq!(out, "u=u_val a=1 b=2");
    }

    // ── validate_pipeline ────────────────────────────────────────

    #[test]
    fn validate_pipeline_rejects_empty_agent() {
        let p = PipelineConfig {
            stages: vec![stage("", "p", "k")],
            on_error: PipelineErrorPolicy::Abort,
        };
        let err = validate_pipeline("test", &p).unwrap_err();
        assert!(err.to_string().contains("empty `agent`"));
    }

    #[test]
    fn validate_pipeline_rejects_empty_prompt() {
        let p = PipelineConfig {
            stages: vec![stage("a", "", "k")],
            on_error: PipelineErrorPolicy::Abort,
        };
        let err = validate_pipeline("test", &p).unwrap_err();
        assert!(err.to_string().contains("empty `prompt`"));
    }

    #[test]
    fn validate_pipeline_rejects_duplicate_output_key() {
        let p = PipelineConfig {
            stages: vec![stage("a", "p1", "shared"), stage("b", "p2", "shared")],
            on_error: PipelineErrorPolicy::Abort,
        };
        let err = validate_pipeline("test", &p).unwrap_err();
        assert!(err.to_string().contains("duplicate `output_key`"));
    }

    #[test]
    fn validate_pipeline_accepts_well_formed() {
        let p = PipelineConfig {
            stages: vec![stage("planner", "p1", "plan"), stage("coder", "p2", "code")],
            on_error: PipelineErrorPolicy::Abort,
        };
        assert!(validate_pipeline("test", &p).is_ok());
    }

    // ── run_pipeline end-to-end ──────────────────────────────────

    #[tokio::test]
    async fn run_pipeline_executes_stages_in_order_and_threads_blackboard() {
        let delegate = ScriptedDelegate::from_outputs(vec![
            "PLAN: do A then B",
            "RESEARCH: facts about A",
            "CODE: <code based on plan + research>",
        ]);
        let pipeline = PipelineConfig {
            stages: vec![
                stage("planner", "Break down: {user_message}", "plan"),
                stage("researcher", "Gather facts for: {prior.plan}", "research"),
                stage(
                    "coder",
                    "Implement.\nPlan: {prior.plan}\nResearch: {prior.research}",
                    "code",
                ),
            ],
            on_error: PipelineErrorPolicy::Abort,
        };

        let result = run_pipeline("test", &pipeline, "build a widget", &delegate)
            .await
            .unwrap();

        assert_eq!(result.stages_run, 3);
        assert_eq!(result.final_output, "CODE: <code based on plan + research>");
        assert_eq!(result.blackboard.len(), 3);
        assert_eq!(result.blackboard["plan"], "PLAN: do A then B");
        assert_eq!(result.blackboard["research"], "RESEARCH: facts about A");

        // Inspect what the delegate was actually called with.
        let calls = delegate.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0]["agent"], "planner");
        assert_eq!(calls[0]["prompt"], "Break down: build a widget");
        assert_eq!(calls[1]["agent"], "researcher");
        assert_eq!(calls[1]["prompt"], "Gather facts for: PLAN: do A then B");
        assert_eq!(calls[2]["agent"], "coder");
        assert!(calls[2]["prompt"]
            .as_str()
            .unwrap()
            .contains("Plan: PLAN: do A then B"));
        assert!(calls[2]["prompt"]
            .as_str()
            .unwrap()
            .contains("Research: RESEARCH: facts about A"));
    }

    #[tokio::test]
    async fn run_pipeline_aborts_on_first_failure_with_abort_policy() {
        let delegate = ScriptedDelegate {
            responses: Mutex::new(vec![
                Ok(ToolResult {
                    success: true,
                    output: "ok-1".into(),
                    error: None,
                }),
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("planner exploded".into()),
                }),
                // 3rd response shouldn't be consumed.
                Ok(ToolResult {
                    success: true,
                    output: "should-not-run".into(),
                    error: None,
                }),
            ]),
            calls: Mutex::new(Vec::new()),
        };
        let pipeline = PipelineConfig {
            stages: vec![
                stage("a", "p1", "k1"),
                stage("b", "p2", "k2"),
                stage("c", "p3", "k3"),
            ],
            on_error: PipelineErrorPolicy::Abort,
        };
        let result = run_pipeline("t", &pipeline, "u", &delegate).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("planner exploded"));
        assert!(err.contains("stage 1"));
        // Confirm only 2 stages were attempted.
        assert_eq!(delegate.calls().len(), 2);
    }

    #[tokio::test]
    async fn run_pipeline_continues_past_failure_with_continue_policy() {
        let delegate = ScriptedDelegate {
            responses: Mutex::new(vec![
                Ok(ToolResult {
                    success: true,
                    output: "ok-1".into(),
                    error: None,
                }),
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("transient".into()),
                }),
                Ok(ToolResult {
                    success: true,
                    output: "ok-3".into(),
                    error: None,
                }),
            ]),
            calls: Mutex::new(Vec::new()),
        };
        let pipeline = PipelineConfig {
            stages: vec![
                stage("a", "p1", "k1"),
                stage("b", "p2", "k2"),
                stage("c", "incorporate {prior.k2}", "k3"),
            ],
            on_error: PipelineErrorPolicy::Continue,
        };
        let result = run_pipeline("t", &pipeline, "u", &delegate)
            .await
            .unwrap();
        assert_eq!(result.stages_run, 3);
        assert_eq!(result.blackboard["k1"], "ok-1");
        assert_eq!(result.blackboard["k2"], "ERROR: transient");
        assert_eq!(result.blackboard["k3"], "ok-3");
        // Stage 3 should have seen the error string substituted.
        let calls = delegate.calls();
        assert_eq!(calls[2]["prompt"], "incorporate ERROR: transient");
    }

    #[tokio::test]
    async fn run_pipeline_rejects_empty_pipeline() {
        let delegate = ScriptedDelegate::from_outputs(vec![]);
        let pipeline = PipelineConfig {
            stages: vec![],
            on_error: PipelineErrorPolicy::Abort,
        };
        let err = run_pipeline("empty", &pipeline, "u", &delegate)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("has no stages"));
    }

    #[tokio::test]
    async fn run_pipeline_surfaces_validation_errors() {
        let delegate = ScriptedDelegate::from_outputs(vec![]);
        let pipeline = PipelineConfig {
            stages: vec![stage("a", "p1", "shared"), stage("b", "p2", "shared")],
            on_error: PipelineErrorPolicy::Abort,
        };
        let err = run_pipeline("bad", &pipeline, "u", &delegate)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("duplicate `output_key`"));
        // Nothing should have been called.
        assert!(delegate.calls().is_empty());
    }

    #[tokio::test]
    async fn run_pipeline_passes_rendered_context_when_set() {
        let delegate = ScriptedDelegate::from_outputs(vec!["x"]);
        let pipeline = PipelineConfig {
            stages: vec![PipelineStage {
                agent: "a".into(),
                prompt: "p".into(),
                output_key: "k".into(),
                context: Some("ctx-for-{user_message}".into()),
            }],
            on_error: PipelineErrorPolicy::Abort,
        };
        run_pipeline("t", &pipeline, "hello", &delegate)
            .await
            .unwrap();
        let calls = delegate.calls();
        assert_eq!(calls[0]["context"], "ctx-for-hello");
    }
}
