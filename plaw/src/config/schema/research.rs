//! Research-phase configuration (`[research_phase]` section).
//!
//! Extracted from `config/schema/mod.rs` on 2026-05-24 as the second
//! focused slice of the 10K-LoC `schema.rs` mega-file split (audit
//! Top-4 #3b — see [[project-2026-05-23-four-lens-synthesis]]).
//!
//! Public items are re-exported from `config::schema` via
//! `pub use research::*` so `crate::config::ResearchPhaseConfig` and
//! friends keep working without consumer churn.

use super::default_true;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Research Phase ───────────────────────────────────────────────

/// Research phase trigger mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResearchTrigger {
    /// Never trigger research phase.
    #[default]
    Never,
    /// Always trigger research phase before responding.
    Always,
    /// Trigger when message contains configured keywords.
    Keywords,
    /// Trigger when message exceeds minimum length.
    Length,
    /// Trigger when message contains a question mark.
    Question,
}

/// Research phase configuration (`[research]` section).
///
/// When enabled, the agent proactively gathers information using tools
/// before generating its main response. This creates a "thinking" phase
/// where the agent explores the codebase, searches memory, or fetches
/// external data to inform its answer.
///
/// ```toml
/// [research]
/// enabled = true
/// trigger = "keywords"
/// keywords = ["find", "search", "check", "investigate"]
/// max_iterations = 5
/// show_progress = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResearchPhaseConfig {
    /// Enable the research phase.
    #[serde(default)]
    pub enabled: bool,

    /// When to trigger research phase.
    #[serde(default)]
    pub trigger: ResearchTrigger,

    /// Keywords that trigger research phase (when `trigger = "keywords"`).
    #[serde(default = "default_research_keywords")]
    pub keywords: Vec<String>,

    /// Minimum message length to trigger research (when `trigger = "length"`).
    #[serde(default = "default_research_min_length")]
    pub min_message_length: usize,

    /// Maximum tool call iterations during research phase.
    #[serde(default = "default_research_max_iterations")]
    pub max_iterations: usize,

    /// Show detailed progress during research (tool calls, results).
    #[serde(default = "default_true")]
    pub show_progress: bool,

    /// Custom system prompt prefix for research phase.
    /// If empty, uses default research instructions.
    #[serde(default)]
    pub system_prompt_prefix: String,
}

fn default_research_keywords() -> Vec<String> {
    vec![
        "find".into(),
        "search".into(),
        "check".into(),
        "investigate".into(),
        "look".into(),
        "research".into(),
        "найди".into(),
        "проверь".into(),
        "исследуй".into(),
        "поищи".into(),
    ]
}

fn default_research_min_length() -> usize {
    50
}

fn default_research_max_iterations() -> usize {
    5
}

impl Default for ResearchPhaseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trigger: ResearchTrigger::default(),
            keywords: default_research_keywords(),
            min_message_length: default_research_min_length(),
            max_iterations: default_research_max_iterations(),
            show_progress: true,
            system_prompt_prefix: String::new(),
        }
    }
}

