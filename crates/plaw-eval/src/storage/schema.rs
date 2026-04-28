//! Storage schema — Rust types matching the SQLite tables described in
//! `phase-1-eval/design.md` §3.3.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// One full suite execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub suite_name: String,
    pub suite_version: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub plaw_commit: String,
    pub model_version: String,
    pub config_hash: String,
    pub n_total: usize,
    pub n_completed: usize,
    pub n_failed: usize,
}

/// Per-case outcome captured during a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub run_id: String,
    pub case_id: String,
    pub case_cluster: Option<String>,
    pub plaw_response: String,
    pub plaw_trace_id: Option<String>,
    pub metric_scores: HashMap<String, MetricScore>,
    pub latency_ms: u64,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub cache_read_tokens: u32,
    pub error: Option<String>,
}

/// Score returned by a single metric for a single case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricScore {
    /// Normalised value (typically `[0, 1]` or `[-1, 1]`).
    pub value: f64,
    /// Raw judge output (kept for audit).
    pub raw: serde_json::Value,
    /// Which judge model produced this score.
    pub judge_model: String,
}

/// Aggregated statistics across a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateReport {
    pub run_id: String,
    pub metrics: HashMap<String, MetricAggregate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricAggregate {
    pub mean: f64,
    pub stderr: f64,
    pub stderr_clustered: Option<f64>,
    pub ci_lower: f64,
    pub ci_upper: f64,
    pub n: usize,
    pub n_clusters: Option<usize>,
}

/// Cached judge response keyed by `SHA256(prompt + input + model_version)`.
#[derive(Debug, Clone)]
pub struct JudgeCacheEntry {
    pub cache_key: String,
    pub judge_response: String,
    pub created_at: i64,
}

/// Production trace queued for review by the flywheel.
///
/// `trace_id` is the agnostic external identifier (Phase 3 will fill it
/// from OTel). `source_run_id` / `source_case_id` are populated when the
/// queue entry was sampled from an existing eval `case_results` row;
/// `target_suite` records the operator's promotion intent so the
/// promoter knows which `cases.toml` to append to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlywheelEntry {
    pub id: String,
    pub trace_id: String,
    pub sampled_at: i64,
    pub judge_score: Option<f64>,
    pub review_status: String,
    pub reviewed_at: Option<i64>,
    pub promoted_to_suite: Option<String>,
    pub promoted_case_id: Option<String>,
    #[serde(default)]
    pub source_run_id: Option<String>,
    #[serde(default)]
    pub source_case_id: Option<String>,
    #[serde(default)]
    pub target_suite: Option<String>,
}

/// CREATE TABLE statements applied at repo open (idempotent).
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    suite_name TEXT NOT NULL,
    suite_version TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    plaw_commit TEXT NOT NULL,
    model_version TEXT NOT NULL,
    config_hash TEXT NOT NULL,
    n_total INTEGER NOT NULL,
    n_completed INTEGER NOT NULL DEFAULT 0,
    n_failed INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS case_results (
    run_id TEXT NOT NULL,
    case_id TEXT NOT NULL,
    case_cluster TEXT,
    plaw_response TEXT NOT NULL,
    plaw_trace_id TEXT,
    metric_scores TEXT NOT NULL,
    latency_ms INTEGER NOT NULL,
    tokens_in INTEGER NOT NULL,
    tokens_out INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    error TEXT,
    PRIMARY KEY (run_id, case_id),
    FOREIGN KEY (run_id) REFERENCES runs(id)
);

CREATE TABLE IF NOT EXISTS judge_cache (
    cache_key TEXT PRIMARY KEY,
    judge_response TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS flywheel_queue (
    id TEXT PRIMARY KEY,
    trace_id TEXT NOT NULL,
    sampled_at INTEGER NOT NULL,
    judge_score REAL,
    review_status TEXT NOT NULL,
    reviewed_at INTEGER,
    promoted_to_suite TEXT,
    promoted_case_id TEXT,
    source_run_id TEXT,
    source_case_id TEXT,
    target_suite TEXT
);

CREATE INDEX IF NOT EXISTS idx_runs_suite ON runs(suite_name, started_at);
CREATE INDEX IF NOT EXISTS idx_results_run ON case_results(run_id);
CREATE INDEX IF NOT EXISTS idx_flywheel_status ON flywheel_queue(review_status);
"#;
