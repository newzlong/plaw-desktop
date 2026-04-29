//! Eval suite data model.
//!
//! Mirrors the schema in `design.md` §3.1. Suites are authored as TOML files
//! under `evals/<suite>/cases.toml`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A complete eval suite — metadata plus a list of cases.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Suite {
    pub name: String,
    /// Semver. Major-version mismatches are rejected on load (`version.rs`).
    pub version: String,
    pub description: String,
    pub default_judge: JudgeSpec,
    #[serde(default)]
    pub metrics: Vec<MetricSpec>,
    pub cases: Vec<Case>,
}

/// Single eval case.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Case {
    pub id: String,
    pub input: CaseInput,
    #[serde(default)]
    pub expected: Option<CaseExpected>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Cluster identifier — when set, observations sharing the same cluster
    /// are correlated and the runner switches to cluster-robust SE.
    #[serde(default)]
    pub cluster_id: Option<String>,
    /// Provenance: 'authored' (hand-written) or 'flywheel' (promoted trace).
    #[serde(default = "default_source")]
    pub source: String,
    /// Optional ISO-8601 timestamp set when the case was promoted.
    #[serde(default)]
    pub promoted_at: Option<String>,
    /// Per-case metric whitelist. When `None` (default) every metric in the
    /// suite is applied; when `Some(vec)` only those metrics run for this
    /// case. Useful when a case is only meaningful under one metric — e.g.
    /// a creative-writing case shouldn't be scored on keyword_coverage.
    #[serde(default)]
    pub metrics: Option<Vec<String>>,
}

fn default_source() -> String {
    "authored".to_string()
}

/// What we actually feed to plaw to produce the response under evaluation.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaseInput {
    /// Single-shot or multi-turn chat.
    Chat { messages: Vec<ChatMsg> },
    /// Agent task with a step budget.
    Agent {
        task: String,
        #[serde(default = "default_max_steps")]
        max_steps: usize,
    },
    /// RAG question (optional ground-truth doc to seed the corpus).
    Rag {
        question: String,
        #[serde(default)]
        ground_truth_doc: Option<String>,
    },
}

fn default_max_steps() -> usize {
    20
}

/// One turn of a chat input.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMsg {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// Optional ground-truth signals supplied to graders.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CaseExpected {
    /// Free-text reference answer.
    #[serde(default)]
    pub answer: Option<String>,
    /// Keywords that must appear in the response.
    #[serde(default)]
    pub answer_keywords: Vec<String>,
    /// Ordered list of tool names the agent should call.
    #[serde(default)]
    pub tool_sequence: Vec<String>,
    /// Final-state JSON expected after the agent finishes.
    #[serde(default)]
    pub final_state: Option<serde_json::Value>,
}

/// Judge configuration for a suite (overridable per-metric).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JudgeSpec {
    pub model: String,
    pub provider: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub mode: JudgeMode,
}

fn default_temperature() -> f32 {
    0.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JudgeMode {
    /// Pairwise A vs B comparison; `dual_pass` swaps positions to mitigate
    /// position bias (the only mode allowed in production gating).
    Pairwise {
        #[serde(default = "default_true")]
        dual_pass: bool,
    },
    /// Absolute scoring on a 1..=`scale` integer scale.
    Score { scale: u8 },
    /// Multi-judge jury with cross-family models. Aggregator decides how
    /// the votes are combined.
    Jury {
        models: Vec<JudgeSpec>,
        #[serde(default)]
        aggregator: JuryAggregator,
    },
}

fn default_true() -> bool {
    true
}

impl Default for JudgeMode {
    fn default() -> Self {
        JudgeMode::Pairwise { dual_pass: true }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JuryAggregator {
    /// At least 3-of-N must agree.
    #[default]
    Majority,
    /// LLM-as-a-Fuser style confidence-weighted aggregation.
    ConfidenceWeighted,
}

/// A metric we want to compute per case. Matches a metric impl by `name`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricSpec {
    pub name: String,
    /// Optional per-metric judge override.
    #[serde(default)]
    pub judge: Option<JudgeSpec>,
    /// Free-form parameters passed to the metric implementation.
    #[serde(default)]
    pub params: BTreeMap<String, toml::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialises_a_minimal_suite() {
        let toml_src = r#"
            name = "smoke"
            version = "1.0.0"
            description = "Tiny smoke suite"

            [default_judge]
            model = "kimi-k2.5"
            provider = "kimi"
            mode = { kind = "pairwise", dual_pass = true }

            [[cases]]
            id = "case-1"
            tags = ["smoke"]
            input = { kind = "chat", messages = [
                { role = "user", content = "Hello" }
            ] }
        "#;
        let suite: Suite = toml::from_str(toml_src).unwrap();
        assert_eq!(suite.name, "smoke");
        assert_eq!(suite.cases.len(), 1);
        assert_eq!(suite.cases[0].source, "authored");
        assert!(matches!(suite.cases[0].input, CaseInput::Chat { .. }));
    }

    #[test]
    fn judge_mode_defaults_to_pairwise_dual_pass() {
        let mode = JudgeMode::default();
        match mode {
            JudgeMode::Pairwise { dual_pass } => assert!(dual_pass),
            _ => panic!("default should be pairwise dual-pass"),
        }
    }

    #[test]
    fn agent_input_uses_default_max_steps() {
        let toml_src = r#"
            name = "agent"
            version = "0.1.0"
            description = ""
            [default_judge]
            model = "kimi-k2.5"
            provider = "kimi"
            mode = { kind = "score", scale = 5 }
            [[cases]]
            id = "a1"
            input = { kind = "agent", task = "ls" }
        "#;
        let suite: Suite = toml::from_str(toml_src).unwrap();
        match &suite.cases[0].input {
            CaseInput::Agent { max_steps, .. } => assert_eq!(*max_steps, 20),
            _ => panic!("expected agent input"),
        }
    }
}
