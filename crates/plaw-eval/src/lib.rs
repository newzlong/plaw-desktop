//! plaw-eval — evaluation foundation for the plaw agent runtime.
//!
//! See `.kiro/specs/plaw-elite/phase-1-eval/design.md` for the full design.

pub mod stats;
pub mod metrics;
pub mod judges;
pub mod suite;
pub mod runner;
pub mod storage;
pub mod report;
pub mod flywheel;

/// Crate version exposed for telemetry.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
