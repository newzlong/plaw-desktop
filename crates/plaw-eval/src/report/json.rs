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

/// Write a pretty JSON aggregate report to disk. Creates parent
/// directories as needed.
pub fn write_aggregate(report: &AggregateReport, path: impl AsRef<Path>) -> Result<()> {
    let body = render_aggregate(report)?;
    write_with_mkdir(path.as_ref(), &body, "aggregate")
}

/// Read an [`AggregateReport`] previously written by [`write_aggregate`].
/// Used by `plaw-eval check --baseline <PATH>` to load a file-baseline.
pub fn read_aggregate(path: impl AsRef<Path>) -> Result<AggregateReport> {
    let path = path.as_ref();
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&body)
        .with_context(|| format!("parsing {} as AggregateReport", path.display()))
}

/// Write a pretty JSON comparison report to disk. Creates parent
/// directories as needed.
pub fn write_comparison(report: &ComparisonReport, path: impl AsRef<Path>) -> Result<()> {
    let body = render_comparison(report)?;
    write_with_mkdir(path.as_ref(), &body, "comparison")
}

fn write_with_mkdir(path: &Path, body: &str, kind: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir for {}", path.display()))?;
        }
    }
    std::fs::write(path, body)
        .with_context(|| format!("writing {kind} report to {}", path.display()))
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
            suite_name: None,
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

    #[test]
    fn write_then_read_round_trip_preserves_aggregate() {
        let original = fixture_aggregate();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agg.json");
        write_aggregate(&original, &path).unwrap();
        let loaded = read_aggregate(&path).unwrap();
        assert_eq!(loaded.run_id, original.run_id);
        assert_eq!(loaded.metrics.len(), original.metrics.len());
        assert!((loaded.metrics["g_eval"].mean - 0.80).abs() < 1e-12);
    }

    #[test]
    fn read_aggregate_legacy_file_without_suite_name() {
        // Older JSON without the `suite_name` field — must deserialise with None.
        let body = r#"{
            "run_id": "old-r1",
            "metrics": {
                "g_eval": {
                    "mean": 0.5,
                    "stderr": 0.0,
                    "stderr_clustered": null,
                    "ci_lower": 0.5,
                    "ci_upper": 0.5,
                    "n": 1,
                    "n_clusters": null
                }
            }
        }"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.json");
        std::fs::write(&path, body).unwrap();
        let loaded = read_aggregate(&path).unwrap();
        assert_eq!(loaded.run_id, "old-r1");
        assert_eq!(loaded.suite_name, None);
    }
}
