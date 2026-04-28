//! Production-trace flywheel — sample, review, promote.
//!
//! Phase 1 ships the eval-driven path: sample low-score / failed cases
//! from existing runs, queue them for human review, then promote
//! approved cases into a target suite's `cases.toml`. Phase 3's OTel
//! integration will let us swap the data source for real production
//! traces without changing the queue / promote pipeline.

pub mod promoter;
pub mod reviewer;
pub mod sampler;

pub use promoter::{promote, read_promoted_case, PromotionResult};
pub use reviewer::{list_pending, review, ReviewVerdict};
pub use sampler::{sample_run, SampleStrategy, SampleSummary};
