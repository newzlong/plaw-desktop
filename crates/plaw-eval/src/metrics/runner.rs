//! Score a finished run's case results against the suite's metric specs.
//!
//! The plaw-eval runner is split into two phases by design:
//!   1. `runner::execute()` records raw plaw responses into SQLite.
//!   2. `metrics::runner::score_run()` reads those rows back and applies
//!      the metric impls, writing `metric_scores` per case.
//!
//! Splitting them lets us re-score an old run with a new metric without
//! re-driving plaw — and lets the eval system swap judges between runs
//! without touching the response data.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use serde_json::json;
use tracing::{debug, warn};

use crate::judges::client::JudgeClient;
use crate::metrics::{
    g_eval::{score as g_eval_score, GEvalConfig},
    keywords::{coverage as keyword_coverage, KeywordConfig},
};
use crate::storage::{EvalRepo, MetricScore};
use crate::suite::{Case, CaseInput, ChatRole, MetricSpec, Suite};

/// Apply every metric in `suite.metrics` to every successful case_result
/// of the given run. Returns the number of (case, metric) pairs scored.
pub async fn score_run(
    repo: &EvalRepo,
    run_id: &str,
    suite: &Suite,
    judge: &dyn JudgeClient,
) -> Result<usize> {
    if suite.metrics.is_empty() {
        debug!(suite = %suite.name, "no metrics declared; skipping scoring");
        return Ok(0);
    }
    let mut case_lookup: HashMap<String, &Case> = HashMap::with_capacity(suite.cases.len());
    for c in &suite.cases {
        case_lookup.insert(c.id.clone(), c);
    }

    let mut results = repo.load_case_results(run_id)?;
    let mut total = 0usize;
    for r in results.iter_mut() {
        if r.error.is_some() {
            continue;
        }
        let Some(case) = case_lookup.get(r.case_id.as_str()) else {
            warn!(case_id = %r.case_id, "case not found in suite; skipping");
            continue;
        };
        let mut scored: HashMap<String, MetricScore> = HashMap::new();
        for spec in &suite.metrics {
            match compute_metric(spec, case, &r.plaw_response, judge).await {
                Ok(Some(score)) => {
                    scored.insert(spec.name.clone(), score);
                    total += 1;
                }
                Ok(None) => {
                    debug!(metric = %spec.name, "metric returned no score");
                }
                Err(e) => {
                    warn!(metric = %spec.name, case_id = %r.case_id, error = %e, "metric scoring failed");
                }
            }
        }
        // Merge with anything the runner pre-populated (currently empty,
        // but reserved for future).
        let mut merged = std::mem::take(&mut r.metric_scores);
        for (k, v) in scored {
            merged.insert(k, v);
        }
        repo.update_metric_scores(run_id, &r.case_id, &merged)?;
    }
    Ok(total)
}

/// Produce a single metric score for one case. Returns `Ok(None)` when
/// the metric is recognised but cannot be applied (e.g. keyword coverage
/// without expected keywords).
pub async fn compute_metric(
    spec: &MetricSpec,
    case: &Case,
    response_text: &str,
    judge: &dyn JudgeClient,
) -> Result<Option<MetricScore>> {
    match spec.name.as_str() {
        "g_eval" => {
            let cfg = parse_g_eval_params(spec)?;
            let question = question_text(&case.input);
            let result = g_eval_score(judge, &cfg, &question, response_text).await?;
            Ok(Some(MetricScore {
                value: result.value,
                raw: json!({
                    "raw_score": result.raw_score,
                    "confidence": result.confidence,
                    "raw_text": result.raw_text,
                }),
                judge_model: judge.model().to_string(),
            }))
        }
        "keyword_coverage" => {
            let keywords = case
                .expected
                .as_ref()
                .map(|e| e.answer_keywords.clone())
                .unwrap_or_default();
            if keywords.is_empty() {
                return Ok(None);
            }
            let cfg = parse_keyword_params(spec);
            let value = keyword_coverage(response_text, &keywords, &cfg);
            Ok(Some(MetricScore {
                value,
                raw: json!({"keywords": keywords, "config": {
                    "case_insensitive": cfg.case_insensitive,
                    "whole_word": cfg.whole_word,
                }}),
                judge_model: "deterministic".into(),
            }))
        }
        unknown => {
            // Unknown metrics shouldn't crash the run; warn and skip.
            warn!(metric = %unknown, "metric not implemented in M7 runner");
            Ok(None)
        }
    }
}

fn parse_g_eval_params(spec: &MetricSpec) -> Result<GEvalConfig> {
    let mut cfg = GEvalConfig::default();
    if let Some(v) = spec.params.get("dimension") {
        cfg.dimension = v
            .as_str()
            .ok_or_else(|| anyhow!("g_eval.dimension must be a string"))?
            .to_string();
    }
    if let Some(v) = spec.params.get("scale") {
        let n = v
            .as_integer()
            .ok_or_else(|| anyhow!("g_eval.scale must be an integer"))?;
        if !(1..=10).contains(&n) {
            return Err(anyhow!("g_eval.scale must be between 1 and 10"));
        }
        cfg.scale = n as u8;
    }
    if let Some(v) = spec.params.get("task_context") {
        cfg.task_context = v
            .as_str()
            .ok_or_else(|| anyhow!("g_eval.task_context must be a string"))?
            .to_string();
    }
    Ok(cfg)
}

fn parse_keyword_params(spec: &MetricSpec) -> KeywordConfig {
    let mut cfg = KeywordConfig::default();
    if let Some(v) = spec.params.get("case_insensitive") {
        if let Some(b) = v.as_bool() {
            cfg.case_insensitive = b;
        }
    }
    if let Some(v) = spec.params.get("whole_word") {
        if let Some(b) = v.as_bool() {
            cfg.whole_word = b;
        }
    }
    cfg
}

/// Pull the user-facing question from a case's input. For chat inputs we
/// take the last user-role message; for agents the task; for RAG the
/// question. Falls back to an empty string when nothing is present.
pub fn question_text(input: &CaseInput) -> String {
    match input {
        CaseInput::Chat { messages } => messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, ChatRole::User))
            .map(|m| m.content.clone())
            .unwrap_or_default(),
        CaseInput::Agent { task, .. } => task.clone(),
        CaseInput::Rag { question, .. } => question.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judges::client::{JudgeFamily, MockJudgeClient};
    use crate::storage::CaseResult;
    use crate::suite::{Case, CaseExpected, CaseInput, ChatMsg, ChatRole, JudgeMode, JudgeSpec};

    fn suite_with_metrics(metrics: Vec<MetricSpec>) -> Suite {
        Suite {
            name: "t".into(),
            version: "1.0.0".into(),
            description: "".into(),
            default_judge: JudgeSpec {
                model: "kimi-k2.5".into(),
                provider: "kimi".into(),
                temperature: 0.0,
                mode: JudgeMode::default(),
            },
            metrics,
            cases: vec![Case {
                id: "c1".into(),
                input: CaseInput::Chat {
                    messages: vec![ChatMsg {
                        role: ChatRole::User,
                        content: "What is the capital of France?".into(),
                    }],
                },
                expected: Some(CaseExpected {
                    answer_keywords: vec!["Paris".into()],
                    ..CaseExpected::default()
                }),
                tags: vec![],
                cluster_id: None,
                source: "authored".into(),
                promoted_at: None,
            }],
        }
    }

    fn case_result(case_id: &str, response: &str) -> CaseResult {
        CaseResult {
            run_id: "r".into(),
            case_id: case_id.into(),
            case_cluster: None,
            plaw_response: response.into(),
            plaw_trace_id: None,
            metric_scores: HashMap::new(),
            latency_ms: 0,
            tokens_in: 0,
            tokens_out: 0,
            cache_read_tokens: 0,
            error: None,
        }
    }

    #[test]
    fn question_text_extraction() {
        let chat = CaseInput::Chat {
            messages: vec![
                ChatMsg {
                    role: ChatRole::System,
                    content: "be brief".into(),
                },
                ChatMsg {
                    role: ChatRole::User,
                    content: "first?".into(),
                },
                ChatMsg {
                    role: ChatRole::User,
                    content: "second?".into(),
                },
            ],
        };
        // We take the *last* user message since that's the actual prompt.
        assert_eq!(question_text(&chat), "second?");

        let agent = CaseInput::Agent {
            task: "do thing".into(),
            max_steps: 3,
        };
        assert_eq!(question_text(&agent), "do thing");

        let rag = CaseInput::Rag {
            question: "explain X".into(),
            ground_truth_doc: None,
        };
        assert_eq!(question_text(&rag), "explain X");
    }

    #[test]
    fn parse_g_eval_params_uses_defaults_when_missing() {
        let spec = MetricSpec {
            name: "g_eval".into(),
            judge: None,
            params: Default::default(),
        };
        let cfg = parse_g_eval_params(&spec).unwrap();
        assert_eq!(cfg.scale, 5);
        assert_eq!(cfg.dimension, "overall_quality");
    }

    #[test]
    fn parse_g_eval_params_rejects_bad_scale() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("scale".into(), toml::Value::Integer(99));
        let spec = MetricSpec {
            name: "g_eval".into(),
            judge: None,
            params,
        };
        assert!(parse_g_eval_params(&spec).is_err());
    }

    #[tokio::test]
    async fn keyword_metric_skips_when_no_expected_keywords() {
        // Suite case with no expected keywords → keyword_coverage opts out.
        let mut suite = suite_with_metrics(vec![MetricSpec {
            name: "keyword_coverage".into(),
            judge: None,
            params: Default::default(),
        }]);
        suite.cases[0].expected = Some(CaseExpected::default());
        let case = &suite.cases[0];
        let judge = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let result = compute_metric(&suite.metrics[0], case, "Paris", &judge)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn keyword_metric_scores_perfectly_when_keywords_present() {
        let suite = suite_with_metrics(vec![MetricSpec {
            name: "keyword_coverage".into(),
            judge: None,
            params: Default::default(),
        }]);
        let case = &suite.cases[0];
        let judge = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let score = compute_metric(&suite.metrics[0], case, "Paris is the capital.", &judge)
            .await
            .unwrap()
            .unwrap();
        assert!((score.value - 1.0).abs() < 1e-12);
        assert_eq!(score.judge_model, "deterministic");
    }

    #[tokio::test]
    async fn unknown_metric_warns_but_does_not_error() {
        let spec = MetricSpec {
            name: "imaginary_metric".into(),
            judge: None,
            params: Default::default(),
        };
        let suite = suite_with_metrics(vec![]);
        let case = &suite.cases[0];
        let judge = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let result = compute_metric(&spec, case, "anything", &judge)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn score_run_writes_metric_scores_to_storage() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let suite = suite_with_metrics(vec![MetricSpec {
            name: "keyword_coverage".into(),
            judge: None,
            params: Default::default(),
        }]);
        // Insert a run + case_result with empty metric_scores.
        let run = crate::storage::Run {
            id: "r1".into(),
            suite_name: suite.name.clone(),
            suite_version: suite.version.clone(),
            started_at: 0,
            finished_at: Some(1),
            plaw_commit: "x".into(),
            model_version: "kimi-k2.5".into(),
            config_hash: "h".into(),
            n_total: 1,
            n_completed: 1,
            n_failed: 0,
        };
        repo.insert_run(&run).unwrap();
        let mut cr = case_result("c1", "Paris is the answer.");
        cr.run_id = "r1".into();
        repo.insert_case_result(&cr).unwrap();
        let judge = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);

        let n = score_run(&repo, "r1", &suite, &judge).await.unwrap();
        assert_eq!(n, 1);

        let stored = repo.load_case_results("r1").unwrap();
        assert_eq!(stored.len(), 1);
        let score = stored[0].metric_scores.get("keyword_coverage").unwrap();
        assert!((score.value - 1.0).abs() < 1e-12);
    }
}
