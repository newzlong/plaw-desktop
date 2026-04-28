//! Report rendering — JSON, Markdown, PR comment, plus the gate logic
//! that compares two runs.

pub mod gate;
pub mod json;
pub mod markdown;
pub mod pr_comment;

pub use gate::{
    aggregate_and_compare, compare_in_memory, compare_runs, compare_runs_default, ComparisonReport,
    GateVerdict, MetricComparison, MetricVerdict, PairedDiffSummary, DEFAULT_EPSILON,
};
pub use json::{
    render_aggregate as render_aggregate_json, render_comparison as render_comparison_json,
    write_aggregate as write_aggregate_json, write_comparison as write_comparison_json,
};
pub use markdown::{
    render_aggregate as render_aggregate_md, render_comparison as render_comparison_md,
};
pub use pr_comment::{extract_failing_rows, render as render_pr_comment, FailingCaseRow};
