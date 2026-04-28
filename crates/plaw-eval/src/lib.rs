//! plaw-eval — evaluation foundation for the plaw agent runtime.
//!
//! See `.kiro/specs/plaw-elite/phase-1-eval/design.md` for the full design.

pub mod flywheel;
pub mod judges;
pub mod metrics;
pub mod report;
pub mod runner;
pub mod stats;
pub mod storage;
pub mod suite;

/// Crate version exposed for telemetry.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
