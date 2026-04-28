//! Render comparison reports as SARIF 2.1.0 for GitHub Code Scanning.
//!
//! Each metric in the comparison becomes a SARIF rule; each failing or
//! inconclusive metric becomes a result. Eval results aren't tied to
//! source code lines, so locations point at a synthetic
//! `eval/<run>/<metric>` URI — Code Scanning still surfaces the alert
//! in the PR's "Files changed" view, just without click-through to a
//! code location.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

use crate::report::gate::{ComparisonReport, GateVerdict, MetricComparison, MetricVerdict};

/// Render a [`ComparisonReport`] to a SARIF 2.1.0 JSON string.
pub fn render_comparison(report: &ComparisonReport) -> Result<String> {
    let sarif = build_sarif(report);
    serde_json::to_string_pretty(&sarif).context("serialising SARIF report")
}

/// Write a SARIF JSON report to disk.
pub fn write_comparison(report: &ComparisonReport, path: impl AsRef<Path>) -> Result<()> {
    let body = render_comparison(report)?;
    std::fs::write(&path, body)
        .with_context(|| format!("writing SARIF report to {}", path.as_ref().display()))
}

fn build_sarif(report: &ComparisonReport) -> Value {
    let rules: Vec<Value> = report
        .metrics
        .iter()
        .map(|m| {
            json!({
                "id": format!("plaw-eval/{}", m.metric),
                "name": m.metric,
                "shortDescription": { "text": format!("Eval metric: {}", m.metric) },
                "fullDescription": {
                    "text": format!(
                        "Gate compares lower 95% CI of candidate against \
                         baseline mean minus epsilon ({}).",
                        report.epsilon
                    )
                },
                "defaultConfiguration": { "level": "error" },
                "helpUri": "https://github.com/newzlong/plaw-desktop/blob/main/docs/eval/methodology.md"
            })
        })
        .collect();

    let results: Vec<Value> = report
        .metrics
        .iter()
        .enumerate()
        .filter_map(|(idx, m)| match m.verdict {
            MetricVerdict::Pass => None,
            v => Some(metric_to_result(idx, m, v, report)),
        })
        .collect();

    let invocation_status = match report.verdict {
        GateVerdict::Pass => "success",
        GateVerdict::Fail => "failure",
        GateVerdict::Inconclusive => "timedOut",
    };

    json!({
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "plaw-eval",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/newzlong/plaw-desktop",
                    "rules": rules
                }
            },
            "invocations": [{
                "executionSuccessful": matches!(report.verdict, GateVerdict::Pass),
                "exitSignalName": invocation_status,
                "properties": {
                    "baseline_run_id": report.baseline_run_id,
                    "candidate_run_id": report.candidate_run_id,
                    "paired_case_count": report.paired_case_count,
                    "epsilon": report.epsilon,
                    "alpha": report.alpha,
                }
            }],
            "results": results
        }]
    })
}

fn metric_to_result(
    rule_index: usize,
    m: &MetricComparison,
    verdict: MetricVerdict,
    report: &ComparisonReport,
) -> Value {
    let level = match verdict {
        MetricVerdict::Fail => "error",
        MetricVerdict::Inconclusive => "warning",
        MetricVerdict::Pass => "note",
    };

    let baseline_mean = m.baseline.as_ref().map(|a| a.mean);
    let candidate_mean = m.candidate.as_ref().map(|a| a.mean);
    let candidate_ci = m.candidate.as_ref().map(|a| (a.ci_lower, a.ci_upper));

    let mut props = serde_json::Map::new();
    if let Some(b) = baseline_mean {
        props.insert("baseline_mean".into(), json!(b));
    }
    if let Some(c) = candidate_mean {
        props.insert("candidate_mean".into(), json!(c));
    }
    if let Some((lo, hi)) = candidate_ci {
        props.insert("candidate_ci_lower".into(), json!(lo));
        props.insert("candidate_ci_upper".into(), json!(hi));
    }
    if let Some(p) = &m.paired_diff {
        props.insert(
            "paired".into(),
            json!({
                "mean_diff": p.mean_diff,
                "se": p.se,
                "ci_lower": p.ci_lower,
                "ci_upper": p.ci_upper,
                "n": p.n,
            }),
        );
    }

    json!({
        "ruleId": format!("plaw-eval/{}", m.metric),
        "ruleIndex": rule_index,
        "level": level,
        "message": {
            "text": format!("{} — {}", m.metric, m.reason)
        },
        "locations": [{
            "physicalLocation": {
                "artifactLocation": {
                    "uri": format!(
                        "eval/{}/{}.json",
                        report.candidate_run_id, m.metric
                    ),
                    "uriBaseId": "%SRCROOT%"
                }
            }
        }],
        "properties": props
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::gate::{ComparisonReport, MetricComparison, PairedDiffSummary};
    use crate::storage::MetricAggregate;

    fn metric(name: &str, verdict: MetricVerdict, reason: &str) -> MetricComparison {
        MetricComparison {
            metric: name.into(),
            baseline: Some(MetricAggregate {
                mean: 0.80,
                stderr: 0.02,
                stderr_clustered: None,
                ci_lower: 0.76,
                ci_upper: 0.84,
                n: 30,
                n_clusters: None,
            }),
            candidate: Some(MetricAggregate {
                mean: 0.70,
                stderr: 0.03,
                stderr_clustered: None,
                ci_lower: 0.64,
                ci_upper: 0.76,
                n: 30,
                n_clusters: None,
            }),
            paired_diff: Some(PairedDiffSummary {
                mean_diff: -0.10,
                se: 0.02,
                ci_lower: -0.14,
                ci_upper: -0.06,
                n: 30,
            }),
            verdict,
            reason: reason.into(),
        }
    }

    fn report(metrics: Vec<MetricComparison>, verdict: GateVerdict) -> ComparisonReport {
        ComparisonReport {
            baseline_run_id: "base".into(),
            candidate_run_id: "cand".into(),
            epsilon: 0.01,
            alpha: 0.05,
            metrics,
            verdict,
            paired_case_count: 30,
            baseline_case_count: 30,
            candidate_case_count: 30,
        }
    }

    #[test]
    fn renders_minimal_failure() {
        let r = report(
            vec![metric(
                "g_eval",
                MetricVerdict::Fail,
                "lower CI below baseline",
            )],
            GateVerdict::Fail,
        );
        let s = render_comparison(&r).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["version"], "2.1.0");
        assert_eq!(v["runs"][0]["results"].as_array().unwrap().len(), 1);
        assert_eq!(v["runs"][0]["results"][0]["level"], "error");
        assert_eq!(v["runs"][0]["results"][0]["ruleId"], "plaw-eval/g_eval");
        assert_eq!(v["runs"][0]["invocations"][0]["executionSuccessful"], false);
    }

    #[test]
    fn passing_metrics_produce_no_results() {
        let r = report(
            vec![metric("g_eval", MetricVerdict::Pass, "ok")],
            GateVerdict::Pass,
        );
        let v: Value = serde_json::from_str(&render_comparison(&r).unwrap()).unwrap();
        assert_eq!(v["runs"][0]["results"].as_array().unwrap().len(), 0);
        assert_eq!(v["runs"][0]["invocations"][0]["executionSuccessful"], true);
    }

    #[test]
    fn inconclusive_emits_warning() {
        let r = report(
            vec![metric("g_eval", MetricVerdict::Inconclusive, "no overlap")],
            GateVerdict::Inconclusive,
        );
        let v: Value = serde_json::from_str(&render_comparison(&r).unwrap()).unwrap();
        let res = &v["runs"][0]["results"][0];
        assert_eq!(res["level"], "warning");
    }

    #[test]
    fn rules_are_emitted_for_every_metric_even_passing() {
        let r = report(
            vec![
                metric("g_eval", MetricVerdict::Pass, "ok"),
                metric("keyword_coverage", MetricVerdict::Fail, "regression"),
            ],
            GateVerdict::Fail,
        );
        let v: Value = serde_json::from_str(&render_comparison(&r).unwrap()).unwrap();
        let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
    }
}
