//! Serialise reports to JSON. Both [`AggregateReport`] and
//! [`ComparisonReport`] are already `Serialize`, so these helpers are
//! about consistent pretty-printing and writing to disk.

use std::path::Path;

use anyhow::{Context, Result};

use crate::report::gate::ComparisonReport;
use crate::storage::AggregateReport;

/// Serialize an [`AggregateReport`] to a pretty JSON string.
pub fn render_aggregate(report: &AggregateReport) -> Result<String> {
    serde_json::to_string_pretty(report).context("serialising aggregate report")
}

/// Serialize a [`ComparisonReport`] to a pretty JSON string.
pub fn render_comparison(report: &ComparisonReport) -> Result<String> {
    serde_json::to_string_pretty(report).context("serialising comparison report")
}

/// Write a pretty JSON aggregate report to disk.
pub fn write_aggregate(report: &AggregateReport, path: impl AsRef<Path>) -> Result<()> {
    let body = render_aggregate(report)?;
    std::fs::write(&path, body)
        .with_context(|| format!("writing aggregate report to {}", path.as_ref().display()))
}

/// Write a pretty JSON comparison report to disk.
pub fn write_comparison(report: &ComparisonReport, path: impl AsRef<Path>) -> Result<()> {
    let body = render_comparison(report)?;
    std::fs::write(&path, body)
        .with_context(|| format!("writing comparison report to {}", path.as_ref().display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MetricAggregate;
    use std::collections::HashMap;

    fn fixture_aggregate() -> AggregateReport {
        let mut metrics = HashMap::new();
        metrics.insert(
            "g_eval".into(),
            MetricAggregate {
                mean: 0.80,
                stderr: 0.05,
                stderr_clustered: None,
                ci_lower: 0.70,
                ci_upper: 0.90,
                n: 30,
                n_clusters: None,
            },
        );
        AggregateReport {
            run_id: "r1".into(),
            metrics,
        }
    }

    #[test]
    fn render_aggregate_includes_metric_fields() {
        let report = fixture_aggregate();
        let json = render_aggregate(&report).unwrap();
        assert!(json.contains("\"run_id\": \"r1\""));
        assert!(json.contains("\"g_eval\""));
        assert!(json.contains("\"mean\": 0.8"));
        assert!(json.contains("\"ci_lower\": 0.7"));
    }
}
