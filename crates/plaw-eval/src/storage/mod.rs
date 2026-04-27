//! SQLite-backed storage for runs, case results, judge cache, flywheel queue.

pub mod schema;
pub mod repo;

pub use schema::{
    AggregateReport, CaseResult, FlywheelEntry, JudgeCacheEntry, MetricAggregate, MetricScore, Run,
    SCHEMA_SQL,
};
pub use repo::EvalRepo;
