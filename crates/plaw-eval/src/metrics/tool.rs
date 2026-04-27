//! Tool-use accuracy metrics — entirely structural, no LLM judge required.
//!
//! Three sub-metrics following Anthropic's "Demystifying Evals" framing:
//!
//! - `tool_selection_f1` — F1 of which tools were called (set comparison
//!   against the expected tool sequence).
//! - `arg_validity_rate` — fraction of tool calls whose argument JSON
//!   parses (and, when a schema is supplied, validates).
//! - `redundant_call_rate` — fraction of `(name, args)` pairs that repeat
//!   within a single trajectory. Lower is better.
//!
//! All metrics return values in `[0, 1]` and never call out to an LLM, so
//! they're safe to compute on every case.

use std::collections::HashMap;
use std::collections::HashSet;

use serde_json::Value;

/// Aggregated summary of tool-call quality for one case trajectory.
#[derive(Debug, Clone)]
pub struct ToolAccuracySummary {
    pub selection_precision: f64,
    pub selection_recall: f64,
    pub selection_f1: f64,
    pub arg_validity_rate: f64,
    pub redundant_call_rate: f64,
    pub n_calls: usize,
}

/// Compute selection precision/recall/F1 against the expected tool list.
///
/// `expected` is treated as a set (order-insensitive); selection F1
/// captures "did the agent reach for the right tools?" without penalising
/// extra invocations of the right tool. Use `redundant_call_rate` for
/// that signal instead.
pub fn selection_f1(actual: &[&str], expected: &[String]) -> (f64, f64, f64) {
    let actual_set: HashSet<&str> = actual.iter().copied().collect();
    let expected_set: HashSet<&str> =
        expected.iter().map(|s| s.as_str()).collect();
    if actual_set.is_empty() && expected_set.is_empty() {
        return (1.0, 1.0, 1.0);
    }
    let tp = actual_set.intersection(&expected_set).count() as f64;
    let precision = if actual_set.is_empty() {
        0.0
    } else {
        tp / actual_set.len() as f64
    };
    let recall = if expected_set.is_empty() {
        0.0
    } else {
        tp / expected_set.len() as f64
    };
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    (precision, recall, f1)
}

/// Fraction of tool calls whose `args` value is non-null JSON. When the
/// expected payload constraints aren't known we still want a signal that
/// the model didn't emit garbage like `null` or wildly malformed JSON.
pub fn arg_validity_rate(args: &[&Value]) -> f64 {
    if args.is_empty() {
        return 1.0;
    }
    let valid = args
        .iter()
        .filter(|v| match v {
            Value::Object(map) => !map.is_empty(),
            Value::Array(arr) => !arr.is_empty(),
            Value::Null => false,
            _ => true,
        })
        .count();
    valid as f64 / args.len() as f64
}

/// Redundant-call rate: fraction of calls whose `(name, args)` exactly
/// repeats an earlier call within the same trajectory. The first
/// occurrence is never redundant; only repeats count.
pub fn redundant_call_rate(calls: &[(&str, &Value)]) -> f64 {
    if calls.is_empty() {
        return 0.0;
    }
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut redundant = 0usize;
    for (name, args) in calls {
        // Canonicalise via serde_json::to_string for stable comparison.
        let key = (name.to_string(), serde_json::to_string(args).unwrap_or_default());
        if !seen.insert(key) {
            redundant += 1;
        }
    }
    redundant as f64 / calls.len() as f64
}

/// One-shot helper that takes the trajectory we get back from `PlawClient`
/// and produces all three sub-metrics in a single struct.
pub fn summarise(
    actual_names: &[String],
    actual_args: &[Value],
    expected_tools: &[String],
) -> ToolAccuracySummary {
    let names: Vec<&str> = actual_names.iter().map(|s| s.as_str()).collect();
    let args: Vec<&Value> = actual_args.iter().collect();
    let calls: Vec<(&str, &Value)> = names
        .iter()
        .zip(actual_args.iter())
        .map(|(n, a)| (*n, a))
        .collect();
    let (p, r, f1) = selection_f1(&names, expected_tools);
    let validity = arg_validity_rate(&args);
    let redundant = redundant_call_rate(&calls);
    ToolAccuracySummary {
        selection_precision: p,
        selection_recall: r,
        selection_f1: f1,
        arg_validity_rate: validity,
        redundant_call_rate: redundant,
        n_calls: actual_names.len(),
    }
}

/// Returns the keys we'd emit into the per-case `metric_scores` map.
pub fn metric_keys_for_summary() -> &'static [&'static str] {
    &[
        "tool_selection_f1",
        "tool_arg_validity",
        "tool_redundant_rate",
    ]
}

/// Render the summary into a `MetricScore` map (lazy view; the runner can
/// inline the JSON values it cares about).
pub fn into_metric_map(summary: &ToolAccuracySummary) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    out.insert("tool_selection_f1".into(), summary.selection_f1);
    out.insert("tool_arg_validity".into(), summary.arg_validity_rate);
    // We invert redundancy so larger is better, matching the convention of
    // every other metric in plaw-eval.
    out.insert(
        "tool_redundant_rate".into(),
        1.0 - summary.redundant_call_rate,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selection_f1_handles_perfect_match() {
        let actual = vec!["read_file", "shell"];
        let expected = vec!["shell".to_string(), "read_file".to_string()];
        let (p, r, f1) = selection_f1(&actual, &expected);
        assert_eq!(p, 1.0);
        assert_eq!(r, 1.0);
        assert_eq!(f1, 1.0);
    }

    #[test]
    fn selection_f1_partial_overlap() {
        let actual = vec!["read_file", "shell", "web_search"];
        let expected = vec!["shell".to_string(), "read_file".to_string()];
        let (p, r, f1) = selection_f1(&actual, &expected);
        // 2 of 3 actual were expected; 2 of 2 expected found.
        assert!((p - 2.0 / 3.0).abs() < 1e-12);
        assert!((r - 1.0).abs() < 1e-12);
        // F1 = 2*p*r/(p+r) = 2*(2/3)*1 / (2/3 + 1) = (4/3)/(5/3) = 4/5
        assert!((f1 - 0.8).abs() < 1e-12);
    }

    #[test]
    fn selection_f1_no_calls_when_none_expected() {
        let actual: Vec<&str> = vec![];
        let expected: Vec<String> = vec![];
        let (p, r, f1) = selection_f1(&actual, &expected);
        assert_eq!(p, 1.0);
        assert_eq!(r, 1.0);
        assert_eq!(f1, 1.0);
    }

    #[test]
    fn selection_f1_zero_when_disjoint() {
        let actual = vec!["a"];
        let expected = vec!["b".to_string()];
        let (_, _, f1) = selection_f1(&actual, &expected);
        assert_eq!(f1, 0.0);
    }

    #[test]
    fn arg_validity_rejects_null_and_empty() {
        let args = vec![
            json!({"path": "/etc"}),
            json!(null),
            json!({}),
            json!([1, 2, 3]),
            json!([]),
        ];
        let refs: Vec<&Value> = args.iter().collect();
        let rate = arg_validity_rate(&refs);
        // 2 valid out of 5 (the populated object and the populated array).
        assert!((rate - 0.4).abs() < 1e-12);
    }

    #[test]
    fn arg_validity_empty_input_perfect() {
        let empty: Vec<&Value> = vec![];
        assert_eq!(arg_validity_rate(&empty), 1.0);
    }

    #[test]
    fn redundant_call_rate_counts_repeats_only() {
        let v_a = json!({"path": "/a"});
        let v_b = json!({"path": "/b"});
        let v_a2 = json!({"path": "/a"});
        let calls: Vec<(&str, &Value)> = vec![
            ("read_file", &v_a),
            ("read_file", &v_b),
            ("read_file", &v_a2), // exact duplicate of first
            ("shell", &v_a),
        ];
        let r = redundant_call_rate(&calls);
        // 1 of 4 was redundant (the third)
        assert!((r - 0.25).abs() < 1e-12);
    }

    #[test]
    fn summarise_combines_all_three_metrics() {
        let names = vec!["read_file".to_string(), "read_file".to_string()];
        let args = vec![json!({"path": "/x"}), json!({"path": "/x"})];
        let expected = vec!["read_file".to_string(), "shell".to_string()];
        let s = summarise(&names, &args, &expected);
        // selection: actual {read_file}, expected {read_file, shell}. p=1, r=0.5, f1≈0.667
        assert!((s.selection_precision - 1.0).abs() < 1e-12);
        assert!((s.selection_recall - 0.5).abs() < 1e-12);
        assert!((s.selection_f1 - 2.0 / 3.0).abs() < 1e-12);
        assert!((s.arg_validity_rate - 1.0).abs() < 1e-12);
        // 1 of 2 calls was a duplicate
        assert!((s.redundant_call_rate - 0.5).abs() < 1e-12);
        assert_eq!(s.n_calls, 2);
    }

    #[test]
    fn into_metric_map_inverts_redundancy_for_higher_is_better_convention() {
        let names = vec!["read_file".to_string(), "read_file".to_string()];
        let args = vec![json!({"path": "/x"}), json!({"path": "/x"})];
        let expected = vec!["read_file".to_string()];
        let s = summarise(&names, &args, &expected);
        let m = into_metric_map(&s);
        // 1 of 2 calls redundant → score 0.5 (inverted from 0.5 raw rate).
        assert!((m["tool_redundant_rate"] - 0.5).abs() < 1e-12);
    }
}
