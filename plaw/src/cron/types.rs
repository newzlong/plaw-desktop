use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum JobType {
    #[default]
    Shell,
    Agent,
    Notification,
    Pipeline,
    /// Native LLM-driven memory consolidation pass (no prompt/command — the
    /// scheduler runs `memory::consolidation::run_consolidation_pass`).
    Consolidation,
}

impl From<JobType> for &'static str {
    fn from(value: JobType) -> Self {
        match value {
            JobType::Shell => "shell",
            JobType::Agent => "agent",
            JobType::Notification => "notification",
            JobType::Pipeline => "pipeline",
            JobType::Consolidation => "consolidation",
        }
    }
}

impl TryFrom<&str> for JobType {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "shell" => Ok(JobType::Shell),
            "agent" => Ok(JobType::Agent),
            "notification" => Ok(JobType::Notification),
            "pipeline" => Ok(JobType::Pipeline),
            "consolidation" => Ok(JobType::Consolidation),
            _ => Err(format!(
                "Invalid job type '{}'. Expected one of: 'shell', 'agent', 'notification', 'pipeline', 'consolidation'",
                value
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionTarget {
    #[default]
    Isolated,
    Main,
}

impl SessionTarget {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::Main => "main",
        }
    }

    pub(crate) fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("main") {
            Self::Main
        } else {
            Self::Isolated
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Schedule {
    Cron {
        expr: String,
        #[serde(default)]
        tz: Option<String>,
    },
    At {
        at: DateTime<Utc>,
    },
    Every {
        every_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryConfig {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default = "default_true")]
    pub best_effort: bool,
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            mode: "none".to_string(),
            channel: None,
            to: None,
            best_effort: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub expression: String,
    pub schedule: Schedule,
    pub command: String,
    pub prompt: Option<String>,
    pub name: Option<String>,
    pub job_type: JobType,
    pub session_target: SessionTarget,
    pub model: Option<String>,
    pub enabled: bool,
    pub delivery: DeliveryConfig,
    pub delete_after_run: bool,
    /// Plaw Desktop session ID to deliver results to (None = auto/active session)
    #[serde(default)]
    pub plaw_session: Option<String>,
    /// AI-generated summary of creation context, injected into agent prompt at execution time
    #[serde(default)]
    pub context_summary: Option<String>,
    /// Per-job wall-clock timeout in seconds (1..=86400). `None` falls back to
    /// a job-type default: 120s for shell jobs, 1800s for agent/pipeline jobs.
    /// Notification jobs ignore it (there is no execution to bound).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Name of the `[pipelines.*]` workflow to run. Only meaningful for
    /// `JobType::Pipeline`; the job's `prompt` carries the pipeline's
    /// initial `{user_message}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub next_run: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub last_status: Option<String>,
    pub last_output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRun {
    pub id: i64,
    pub job_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub status: String,
    pub output: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronJobPatch {
    pub schedule: Option<Schedule>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub delivery: Option<DeliveryConfig>,
    pub model: Option<String>,
    pub session_target: Option<SessionTarget>,
    pub delete_after_run: Option<bool>,
    pub plaw_session: Option<String>,
    pub context_summary: Option<String>,
    /// Per-job shell timeout override in seconds (shell jobs only). `Some(v)`
    /// sets the override; `None` leaves the existing value unchanged.
    pub timeout_secs: Option<u64>,
    /// Pipeline name override (pipeline jobs only). `Some(v)` sets it;
    /// `None` leaves the existing value unchanged.
    pub pipeline_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::JobType;

    #[test]
    fn job_type_try_from_accepts_known_values_case_insensitive() {
        assert_eq!(JobType::try_from("shell").unwrap(), JobType::Shell);
        assert_eq!(JobType::try_from("SHELL").unwrap(), JobType::Shell);
        assert_eq!(JobType::try_from("agent").unwrap(), JobType::Agent);
        assert_eq!(JobType::try_from("AgEnT").unwrap(), JobType::Agent);
        assert_eq!(
            JobType::try_from("notification").unwrap(),
            JobType::Notification
        );
        assert_eq!(
            JobType::try_from("NOTIFICATION").unwrap(),
            JobType::Notification
        );
        assert_eq!(JobType::try_from("pipeline").unwrap(), JobType::Pipeline);
        assert_eq!(JobType::try_from("PipeLine").unwrap(), JobType::Pipeline);
        assert_eq!(
            JobType::try_from("consolidation").unwrap(),
            JobType::Consolidation
        );
        assert_eq!(
            JobType::try_from("CONSOLIDATION").unwrap(),
            JobType::Consolidation
        );
        let s: &str = JobType::Consolidation.into();
        assert_eq!(s, "consolidation");
    }

    #[test]
    fn job_type_str_roundtrip_includes_pipeline() {
        let s: &str = JobType::Pipeline.into();
        assert_eq!(s, "pipeline");
        assert_eq!(JobType::try_from(s).unwrap(), JobType::Pipeline);
    }

    #[test]
    fn job_type_try_from_rejects_invalid_values() {
        assert!(JobType::try_from("").is_err());
        assert!(JobType::try_from("unknown").is_err());
    }
}
