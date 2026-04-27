//! Statistical primitives — confidence intervals, paired analysis, Bradley-Terry MLE.
//!
//! All implementations are cross-checked against `scipy.stats` reference values
//! in `tests/stats_correctness.rs`.

pub mod ci;
pub mod cluster_se;
pub mod paired;
pub mod power;
pub mod bradley_terry;

pub use ci::{
    bootstrap_ci, t_distribution_ci, wilson_score_ci, ConfidenceInterval, DEFAULT_ALPHA,
};
pub use cluster_se::{cluster_robust_se, count_clusters, should_use_cluster_se};
pub use paired::{paired_difference, PairedResult};
pub use power::required_sample_size;
pub use bradley_terry::{
    bradley_terry_bootstrap_ci, bradley_terry_mle, BradleyTerryEstimate, Comparison, Winner,
    DEFAULT_MAX_ITERS, DEFAULT_TOLERANCE,
};
