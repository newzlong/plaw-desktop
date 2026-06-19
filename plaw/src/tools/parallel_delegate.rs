//! Parallel delegation tool: run multiple independent sub-agent tasks concurrently.
//!
//! Unlike `delegate` which dispatches to named pre-configured agents, this tool
//! spawns ephemeral sub-agent loops that share the parent's provider/model but
//! each get an independent conversation history and a filtered tool allowlist.
//! All tasks run concurrently via `tokio::JoinSet` and results are returned
//! as a single aggregated response.

use super::traits::{Tool, ToolResult};
use crate::agent::loop_::run_tool_call_loop;
use crate::observability::traits::Observer;
use crate::providers::{self, ChatMessage, Provider};
use crate::security::policy::ToolOperation;
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// Maximum number of parallel tasks per invocation.
const MAX_PARALLEL_TASKS: usize = 10;

/// Default timeout per sub-task (seconds).
const SUBTASK_TIMEOUT_SECS: u64 = 300;

/// Default max tool iterations per sub-task.
const SUBTASK_MAX_ITERATIONS: usize = 15;

/// Tool that runs multiple independent sub-agent loops in parallel.
/// Each sub-task gets its own conversation history and can only use
/// read-only tools by default (configurable via allowed_tools).
pub struct ParallelDelegateTool {
    security: Arc<SecurityPolicy>,
    /// Provider spec string (e.g. "anthropic-custom:https://api.kimi.com/coding")
    default_provider: String,
    /// Model name (e.g. "kimi-k2.5")
    default_model: String,
    /// API key for creating sub-agent providers
    api_key: Option<String>,
    /// Provider runtime options
    provider_runtime_options: providers::ProviderRuntimeOptions,
    /// Parent tool registry (sub-tasks filter from this)
    parent_tools: Arc<Vec<Arc<dyn Tool>>>,
    /// Multimodal config for sub-agent loops
    multimodal_config: crate::config::MultimodalConfig,
}

/// Default read-only tools for sub-tasks when no allowed_tools specified.
const DEFAULT_READONLY_TOOLS: &[&str] = &[
    "file_read",
    "glob_search",
    "content_search",
    "web_fetch",
    "web_search_tool",
    "memory_recall",
    "pdf_read",
    "image_info",
];

impl ParallelDelegateTool {
    pub fn new(
        security: Arc<SecurityPolicy>,
        default_provider: String,
        default_model: String,
        api_key: Option<String>,
        provider_runtime_options: providers::ProviderRuntimeOptions,
        parent_tools: Arc<Vec<Arc<dyn Tool>>>,
        multimodal_config: crate::config::MultimodalConfig,
    ) -> Self {
        Self {
            security,
            default_provider,
            default_model,
            api_key,
            provider_runtime_options,
            parent_tools,
            multimodal_config,
        }
    }
}

/// Thin wrapper to use Arc<dyn Tool> as Box<dyn Tool>.
struct ToolArcRef {
    inner: Arc<dyn Tool>,
}

impl ToolArcRef {
    fn new(inner: Arc<dyn Tool>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Tool for ToolArcRef {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        self.inner.execute(args).await
    }
}

/// Noop observer for sub-agent loops.
struct NoopObserver;

impl Observer for NoopObserver {
    fn record_event(&self, _event: &crate::observability::traits::ObserverEvent) {}
    fn record_metric(&self, _metric: &crate::observability::traits::ObserverMetric) {}
    fn name(&self) -> &str {
        "noop"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl Tool for ParallelDelegateTool {
    fn name(&self) -> &str {
        "parallel_delegate"
    }

    fn description(&self) -> &str {
        "Run multiple independent sub-tasks in parallel. Each task gets its own conversation \
         context and runs a tool-call loop concurrently. Use when you need to research, analyze, \
         or explore multiple independent questions simultaneously. Sub-tasks default to read-only \
         tools (file_read, glob_search, content_search, web_fetch, memory_recall)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "tasks": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_PARALLEL_TASKS,
                    "description": "Array of sub-tasks to run in parallel",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Short identifier for this task (e.g. 'batch-1', 'search-api')"
                            },
                            "prompt": {
                                "type": "string",
                                "minLength": 1,
                                "description": "The task/prompt for this sub-agent"
                            },
                            "system_prompt": {
                                "type": "string",
                                "description": "Optional system prompt override for this sub-task"
                            },
                            "allowed_tools": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional tool allowlist. Defaults to read-only tools."
                            },
                            "max_iterations": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 30,
                                "description": "Max tool-call iterations for this sub-task (default: 15)"
                            }
                        },
                        "required": ["prompt"]
                    }
                }
            },
            "required": ["tasks"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        // Security check
        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "parallel_delegate")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let tasks = args
            .get("tasks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Missing 'tasks' array parameter"))?;

        if tasks.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("'tasks' array must not be empty".into()),
            });
        }

        if tasks.len() > MAX_PARALLEL_TASKS {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Too many tasks ({}, max {})",
                    tasks.len(),
                    MAX_PARALLEL_TASKS
                )),
            });
        }

        // Create provider (shared across all sub-tasks via Arc)
        let provider: Arc<dyn Provider> = {
            let cred = self.api_key.as_deref();
            match providers::create_provider_with_options(
                &self.default_provider,
                cred,
                &self.provider_runtime_options,
            ) {
                Ok(p) => Arc::from(p),
                Err(e) => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to create provider: {e}")),
                    });
                }
            }
        };

        // Parse tasks and spawn concurrent futures
        let mut join_set = tokio::task::JoinSet::new();

        for (idx, task_value) in tasks.iter().enumerate() {
            let task_id = task_value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let task_id = if task_id.is_empty() {
                format!("task-{}", idx + 1)
            } else {
                task_id
            };

            let prompt = match task_value.get("prompt").and_then(|v| v.as_str()) {
                Some(p) if !p.trim().is_empty() => p.trim().to_string(),
                _ => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Task '{}' has empty or missing prompt", task_id)),
                    });
                }
            };

            let system_prompt = task_value
                .get("system_prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let max_iterations = task_value
                .get("max_iterations")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(SUBTASK_MAX_ITERATIONS);

            // Build allowed tools set
            let allowed: HashSet<String> = if let Some(tools_arr) = task_value
                .get("allowed_tools")
                .and_then(|v| v.as_array())
            {
                tools_arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            } else {
                DEFAULT_READONLY_TOOLS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            };

            // Filter parent tools by allowlist, exclude delegation tools
            let sub_tools: Vec<Box<dyn Tool>> = self
                .parent_tools
                .iter()
                .filter(|tool| allowed.contains(tool.name()))
                .filter(|tool| {
                    tool.name() != "delegate"
                        && tool.name() != "parallel_delegate"
                        && tool.name() != "subagent_spawn"
                })
                .map(|tool| Box::new(ToolArcRef::new(tool.clone())) as Box<dyn Tool>)
                .collect();

            let provider = provider.clone();
            let model = self.default_model.clone();
            let multimodal_config = self.multimodal_config.clone();

            // Capture parent's trace context BEFORE the spawn — once inside
            // the spawned task, the task-local is fresh (None by default).
            // If parent had a context, derive a child span; otherwise the
            // sub-task starts its own root trace.
            let parent_ctx_for_child = crate::observability::trace_context::TraceContext::current();

            join_set.spawn(async move {
                let child_ctx = parent_ctx_for_child
                    .as_ref()
                    .map(crate::observability::trace_context::TraceContext::child)
                    .unwrap_or_else(crate::observability::trace_context::TraceContext::root);

                crate::observability::trace_context::CURRENT_TRACE
                    .scope(Some(child_ctx), async move {
                // Bound concurrent agentic loops process-wide (shared with
                // subagent_spawn) so per-call fan-out caps can't compose into a
                // provider-request storm. Held until this sub-task returns.
                let _permit = crate::tools::agentic_semaphore().acquire_owned().await.ok();
                let mut history = Vec::new();
                if let Some(sys) = &system_prompt {
                    history.push(ChatMessage::system(sys.clone()));
                }
                history.push(ChatMessage::user(prompt));

                let noop = NoopObserver;

                let result = tokio::time::timeout(
                    Duration::from_secs(SUBTASK_TIMEOUT_SECS),
                    run_tool_call_loop(
                        &*provider,
                        &mut history,
                        &sub_tools,
                        &noop,
                        "parallel_delegate",
                        &model,
                        0.3, // lower temperature for sub-tasks
                        true,
                        None,
                        "parallel_delegate",
                        &multimodal_config,
                        max_iterations,
                        None,
                        None,
                        None,
                        &[],
                    ),
                )
                .await;

                match result {
                    Ok(Ok(response)) => (task_id, true, response),
                    Ok(Err(e)) => (task_id, false, format!("Error: {e}")),
                    Err(_) => (
                        task_id,
                        false,
                        format!("Timed out after {SUBTASK_TIMEOUT_SECS}s"),
                    ),
                }
                    })
                    .await
            });
        }

        // Collect results
        let mut results: Vec<(String, bool, String)> = Vec::new();
        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok(task_result) => results.push(task_result),
                Err(e) => results.push(("unknown".into(), false, format!("Join error: {e}"))),
            }
        }

        // Sort by task_id for deterministic output
        results.sort_by(|a, b| a.0.cmp(&b.0));

        // Format output
        let total = results.len();
        let succeeded = results.iter().filter(|(_, ok, _)| *ok).count();
        let mut output = format!(
            "[parallel_delegate: {succeeded}/{total} tasks completed]\n\n"
        );

        for (task_id, success, response) in &results {
            let status = if *success { "OK" } else { "FAILED" };
            output.push_str(&format!("--- [{task_id}] ({status}) ---\n"));
            // Truncate very long responses to keep context manageable
            if response.len() > 4000 {
                // Find a valid UTF-8 char boundary at or before 4000
                let mut end = 4000;
                while end > 0 && !response.is_char_boundary(end) {
                    end -= 1;
                }
                output.push_str(&response[..end]);
                output.push_str("\n...(truncated)\n");
            } else {
                output.push_str(response);
            }
            output.push_str("\n\n");
        }

        Ok(ToolResult {
            success: succeeded > 0,
            output,
            error: if succeeded == 0 {
                Some("All parallel tasks failed".into())
            } else {
                None
            },
        })
    }
}
