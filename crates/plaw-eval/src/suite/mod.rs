//! Eval suite types and TOML loader.

pub mod case;
pub mod loader;
pub mod version;

pub use case::{
    Case, CaseExpected, CaseInput, ChatMsg, ChatRole, JudgeMode, JudgeSpec, JuryAggregator,
    MetricSpec, Suite,
};
pub use loader::{discover_suites, load_suite, SUITE_SCHEMA_MAJOR};
pub use version::{ensure_compatible, parse_semver};
