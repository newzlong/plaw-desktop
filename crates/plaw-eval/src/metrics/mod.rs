//! Quality metrics — G-Eval, keyword coverage, tool-call accuracy.
//!
//! Phase 1 ships the metrics needed for the smoke / nightly suites (M9):
//! G-Eval for free-form chat quality, tool accuracy for agent tasks,
//! keyword coverage for grounded RAG-style cases. Faithfulness, answer
//! relevancy, context precision/recall, plan-quality, repeatability, and
//! error-recovery are stubs in tasks.md slated for follow-up work.

pub mod g_eval;
pub mod keywords;
pub mod runner;
pub mod tool;

pub use g_eval::{score as g_eval_score, GEvalConfig, GEvalScore};
pub use keywords::{coverage as keyword_coverage, KeywordConfig};
pub use runner::{compute_metric, question_text, score_run};
pub use tool::{
    arg_validity_rate, into_metric_map as tool_into_metric_map, redundant_call_rate,
    selection_f1, summarise as tool_summarise, ToolAccuracySummary,
};
