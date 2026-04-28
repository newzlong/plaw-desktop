//! SQLite-backed storage for runs, case results, judge cache, flywheel queue.

pub mod repo;
pub mod schema;

pub use repo::EvalRepo;
pub use schema::{
    AggregateReport, CaseResult, FlywheelEntry, JudgeCacheEntry, MetricAggregate, MetricScore,
    RecordedToolCall, Run, SCHEMA_SQL,
};
