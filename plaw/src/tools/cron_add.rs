use super::traits::{Tool, ToolResult};
use crate::config::Config;
use crate::cron::{self, DeliveryConfig, JobType, Schedule, SessionTarget};
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct CronAddTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
}

impl CronAddTool {
    pub fn new(config: Arc<Config>, security: Arc<SecurityPolicy>) -> Self {
        Self { config, security }
    }

    fn enforce_mutation_allowed(&self, action: &str) -> Option<ToolResult> {
        if !self.security.can_act() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Security policy: read-only mode, cannot perform '{action}'"
                )),
            });
        }

        if self.security.is_rate_limited() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".to_string()),
            });
        }

        if !self.security.record_action() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".to_string()),
            });
        }

        None
    }
}

#[async_trait]
impl Tool for CronAddTool {
    fn name(&self) -> &str {
        "cron_add"
    }

    fn description(&self) -> &str {
        "Create a scheduled cron job with cron/at/every schedules. Four job types:\n\
         - notification: Pure reminder/alert. The prompt text is shown directly to the user. No AI call. \
           Use this when the user says 'remind me...', 'don't forget...', 'alert me at...'.\n\
         - agent: AI executes a task with tools. Use context_summary to capture the current conversation context \
           so the agent has background when it runs. Use when the user says 'help me...', 'summarize...', 'check...', 'fetch...'.\n\
         - shell: Runs a shell command.\n\
         - pipeline: Runs a pre-configured [pipelines.*] multi-stage workflow. Requires pipeline_name; the prompt \
           becomes the pipeline's initial {user_message}. Use when the user references a named workflow/pipeline.\n\
         When in doubt between notification and agent, prefer agent (a wrong notification misses the task entirely).\n\
         For agent jobs, ALWAYS include context_summary with a summary of the relevant conversation context."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "schedule": {
                    "type": "object",
                    "description": "Schedule object: {kind:'cron',expr,tz?} | {kind:'at',at} | {kind:'every',every_ms}. Cron times default to user's local timezone if tz is omitted. Use standard 5-field cron: 'min hour day month weekday'."
                },
                "job_type": {
                    "type": "string",
                    "enum": ["shell", "agent", "notification", "pipeline"],
                    "description": "notification=pure text reminder (no AI call), agent=AI executes with tools, shell=runs shell command, pipeline=runs a pre-configured [pipelines.*] multi-stage workflow"
                },
                "pipeline_name": {
                    "type": "string",
                    "description": "For pipeline jobs: the name of the [pipelines.<name>] workflow to run. The 'prompt' field becomes the pipeline's initial {user_message}."
                },
                "command": { "type": "string", "description": "For shell jobs: the command to run" },
                "timeout_secs": { "type": "integer", "description": "For shell jobs only: per-job command timeout in seconds (default 120, max 86400). Ignored for agent/notification jobs." },
                "prompt": {
                    "type": "string",
                    "description": "For notification: the text shown to user. For agent: the instruction for AI to execute."
                },
                "context_summary": {
                    "type": "string",
                    "description": "For agent jobs: AI-generated summary of the current conversation context, requirements, and key details. This is injected into the agent's prompt at execution time so it has background knowledge."
                },
                "session_target": { "type": "string", "enum": ["isolated", "main"] },
                "model": { "type": "string" },
                "plaw_session": { "type": "string", "description": "Plaw Desktop session ID to deliver results to" },
                "delivery": {
                    "type": "object",
                    "description": "Delivery config to send job output to a channel. Example: {\"mode\":\"announce\",\"channel\":\"discord\",\"to\":\"<channel_id>\"}",
                    "properties": {
                        "mode": { "type": "string", "enum": ["none", "announce"] },
                        "channel": { "type": "string", "enum": ["telegram", "discord", "slack", "mattermost", "qq", "email"] },
                        "to": { "type": "string" },
                        "best_effort": { "type": "boolean" }
                    }
                },
                "delete_after_run": { "type": "boolean" },
                "approved": {
                    "type": "boolean",
                    "description": "Set true to explicitly approve medium/high-risk shell commands in supervised mode",
                    "default": false
                }
            },
            "required": ["schedule"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if !self.config.cron.enabled {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("cron is disabled by config (cron.enabled=false)".to_string()),
            });
        }

        let schedule = match args.get("schedule") {
            Some(v) => match serde_json::from_value::<Schedule>(v.clone()) {
                Ok(mut schedule) => {
                    // Auto-fill local timezone for cron schedules when tz is not specified,
                    // so "25 23 * * *" means 23:25 local time, not UTC.
                    if let Schedule::Cron { ref mut tz, .. } = schedule {
                        if tz.is_none() {
                            *tz = Some(iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()));
                        }
                    }
                    schedule
                }
                Err(e) => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Invalid schedule: {e}")),
                    });
                }
            },
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing 'schedule' parameter".to_string()),
                });
            }
        };

        let name = args
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let job_type = match args.get("job_type").and_then(serde_json::Value::as_str) {
            Some("agent") => JobType::Agent,
            Some("shell") => JobType::Shell,
            Some("notification") => JobType::Notification,
            Some("pipeline") => JobType::Pipeline,
            Some(other) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid job_type: {other}")),
                });
            }
            None => {
                if args.get("prompt").is_some() {
                    JobType::Agent
                } else {
                    JobType::Shell
                }
            }
        };

        let default_delete_after_run = matches!(schedule, Schedule::At { .. });
        let delete_after_run = args
            .get("delete_after_run")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_delete_after_run);
        let approved = args
            .get("approved")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let result = match job_type {
            JobType::Shell => {
                let command = match args.get("command").and_then(serde_json::Value::as_str) {
                    Some(command) if !command.trim().is_empty() => command,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'command' for shell job".to_string()),
                        });
                    }
                };

                if let Err(reason) = self.security.validate_command_execution(command, approved) {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(reason),
                    });
                }

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
                }

                let plaw_session = args
                    .get("plaw_session")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                let timeout_secs = args.get("timeout_secs").and_then(serde_json::Value::as_u64);

                cron::add_shell_job(
                    &self.config,
                    name,
                    schedule,
                    command,
                    plaw_session,
                    timeout_secs,
                )
            }
            JobType::Agent => {
                let prompt = match args.get("prompt").and_then(serde_json::Value::as_str) {
                    Some(prompt) if !prompt.trim().is_empty() => prompt,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'prompt' for agent job".to_string()),
                        });
                    }
                };

                let session_target = match args.get("session_target") {
                    Some(v) => match serde_json::from_value::<SessionTarget>(v.clone()) {
                        Ok(target) => target,
                        Err(e) => {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!("Invalid session_target: {e}")),
                            });
                        }
                    },
                    None => SessionTarget::Isolated,
                };

                let model = args
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                let delivery = match args.get("delivery") {
                    Some(v) => match serde_json::from_value::<DeliveryConfig>(v.clone()) {
                        Ok(cfg) => Some(cfg),
                        Err(e) => {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!("Invalid delivery config: {e}")),
                            });
                        }
                    },
                    None => None,
                };

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
                }

                let plaw_session = args
                    .get("plaw_session")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                let context_summary = args
                    .get("context_summary")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                cron::add_agent_job(
                    &self.config,
                    name,
                    schedule,
                    prompt,
                    session_target,
                    model,
                    delivery,
                    delete_after_run,
                    plaw_session,
                    context_summary,
                )
            }
            JobType::Notification => {
                let prompt = match args.get("prompt").and_then(serde_json::Value::as_str) {
                    Some(prompt) if !prompt.trim().is_empty() => prompt,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'prompt' for notification job (the text to show the user)".to_string()),
                        });
                    }
                };

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
                }

                let plaw_session = args
                    .get("plaw_session")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                cron::add_notification_job(
                    &self.config,
                    name,
                    schedule,
                    prompt,
                    delete_after_run,
                    plaw_session,
                )
            }
            JobType::Pipeline => {
                let pipeline_name = match args
                    .get("pipeline_name")
                    .and_then(serde_json::Value::as_str)
                {
                    Some(p) if !p.trim().is_empty() => p,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'pipeline_name' for pipeline job".to_string()),
                        });
                    }
                };

                // Fail-fast: the pipeline must already be declared under
                // [pipelines.*] so the user can't schedule a job that can
                // never run.
                if !self.config.pipelines.contains_key(pipeline_name) {
                    let mut available: Vec<&str> =
                        self.config.pipelines.keys().map(String::as_str).collect();
                    available.sort_unstable();
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(if available.is_empty() {
                            format!(
                                "pipeline '{pipeline_name}' not found; no [pipelines.*] are configured"
                            )
                        } else {
                            format!(
                                "pipeline '{pipeline_name}' not found. Available: {}",
                                available.join(", ")
                            )
                        }),
                    });
                }

                // The pipeline's initial {user_message} comes from the prompt.
                let user_message = args
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();

                let delivery = match args.get("delivery") {
                    Some(v) => match serde_json::from_value::<DeliveryConfig>(v.clone()) {
                        Ok(cfg) => Some(cfg),
                        Err(e) => {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!("Invalid delivery config: {e}")),
                            });
                        }
                    },
                    None => None,
                };

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
                }

                let plaw_session = args
                    .get("plaw_session")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                cron::add_pipeline_job(
                    &self.config,
                    name,
                    schedule,
                    pipeline_name,
                    user_message,
                    delivery,
                    delete_after_run,
                    plaw_session,
                )
            }
        };

        match result {
            Ok(job) => Ok(ToolResult {
                success: true,
                output: serde_json::to_string_pretty(&json!({
                    "id": job.id,
                    "name": job.name,
                    "job_type": job.job_type,
                    "schedule": job.schedule,
                    "next_run": job.next_run,
                    "enabled": job.enabled
                }))?,
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::security::AutonomyLevel;
    use tempfile::TempDir;

    async fn test_config(tmp: &TempDir) -> Arc<Config> {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        tokio::fs::create_dir_all(&config.workspace_dir)
            .await
            .unwrap();
        Arc::new(config)
    }

    fn test_security(cfg: &Config) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy::from_config(
            &cfg.autonomy,
            &cfg.workspace_dir,
        ))
    }

    #[tokio::test]
    async fn adds_shell_job() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));
        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "echo ok"
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("next_run"));
    }

    fn config_with_pipeline(tmp: &TempDir) -> Arc<Config> {
        use crate::config::{PipelineConfig, PipelineErrorPolicy, PipelineStage};
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.pipelines.insert(
            "p1".to_string(),
            PipelineConfig {
                stages: vec![PipelineStage {
                    agent: "planner".into(),
                    prompt: "Plan: {user_message}".into(),
                    output_key: "plan".into(),
                    context: None,
                }],
                on_error: PipelineErrorPolicy::Abort,
            },
        );
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        Arc::new(config)
    }

    #[tokio::test]
    async fn pipeline_job_requires_pipeline_name() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_with_pipeline(&tmp);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));
        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "0 2 * * *" },
                "job_type": "pipeline",
                "prompt": "summarize"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .unwrap_or_default()
            .contains("Missing 'pipeline_name'"));
    }

    #[tokio::test]
    async fn pipeline_job_fails_fast_for_unknown_pipeline() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_with_pipeline(&tmp);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));
        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "0 2 * * *" },
                "job_type": "pipeline",
                "pipeline_name": "ghost",
                "prompt": "summarize"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap_or_default();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("p1"), "{err}");
    }

    #[tokio::test]
    async fn pipeline_job_created_when_configured() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_with_pipeline(&tmp);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));
        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "0 2 * * *" },
                "job_type": "pipeline",
                "pipeline_name": "p1",
                "prompt": "summarize today"
            }))
            .await
            .unwrap();
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("\"job_type\": \"pipeline\""));

        // Persisted with the pipeline name + user_message.
        let jobs = crate::cron::list_jobs(&cfg).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].pipeline_name.as_deref(), Some("p1"));
        assert_eq!(jobs[0].prompt.as_deref(), Some("summarize today"));
    }

    #[tokio::test]
    async fn blocks_disallowed_shell_command() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.allowed_commands = vec!["echo".into()];
        config.autonomy.level = AutonomyLevel::Supervised;
        tokio::fs::create_dir_all(&config.workspace_dir)
            .await
            .unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "curl https://example.com"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("not allowed"));
    }

    #[tokio::test]
    async fn blocks_mutation_in_read_only_mode() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.level = AutonomyLevel::ReadOnly;
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "echo ok"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        let error = result.error.unwrap_or_default();
        assert!(error.contains("read-only") || error.contains("not allowed"));
    }

    #[tokio::test]
    async fn blocks_add_when_rate_limited() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.level = AutonomyLevel::Full;
        config.autonomy.max_actions_per_hour = 0;
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "echo ok"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .unwrap_or_default()
            .contains("Rate limit exceeded"));
        assert!(cron::list_jobs(&cfg).unwrap().is_empty());
    }

    #[tokio::test]
    async fn medium_risk_shell_command_requires_approval() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.allowed_commands = vec!["touch".into()];
        config.autonomy.level = AutonomyLevel::Supervised;
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let denied = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "touch cron-approval-test"
            }))
            .await
            .unwrap();
        assert!(!denied.success);
        assert!(denied
            .error
            .unwrap_or_default()
            .contains("explicit approval"));

        let approved = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "touch cron-approval-test",
                "approved": true
            }))
            .await
            .unwrap();
        assert!(approved.success, "{:?}", approved.error);
    }

    #[tokio::test]
    async fn rejects_invalid_schedule() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "every", "every_ms": 0 },
                "job_type": "shell",
                "command": "echo nope"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .unwrap_or_default()
            .contains("every_ms must be > 0"));
    }

    #[tokio::test]
    async fn agent_job_requires_prompt() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "agent"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .unwrap_or_default()
            .contains("Missing 'prompt'"));
    }
}
