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
use futures_util::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use serde_json::json;
use tracing::{debug, warn};

use crate::judges::client::JudgeClient;
use crate::metrics::{
    g_eval::{score as g_eval_score, GEvalConfig},
    keywords::{coverage as keyword_coverage, KeywordConfig},
    tool::summarise as tool_summarise,
};
use crate::runner::executor::strip_repetition_suffix;
use crate::storage::{EvalRepo, MetricScore, RecordedToolCall};
use crate::suite::{Case, CaseInput, ChatRole, MetricSpec, Suite};

/// Default upper bound on simultaneous in-flight (case, metric) scoring
/// futures. Most judge endpoints rate-limit hard somewhere in the 10–20
/// concurrent-requests range, so 8 is a conservative default that still
/// shaves an order of magnitude off sequential scoring (n=400 cases × 3
/// metrics ≈ 1200 calls).
pub const DEFAULT_SCORING_CONCURRENCY: usize = 8;

/// Outcome of scoring one run. Replaces an earlier signature that returned
/// just `usize` (pair count) — the additional fields surface partial /
/// silent failures that the bare count hid.
#[derive(Debug, Clone, Default)]
pub struct ScoreRunSummary {
    /// Total (case, metric) score pairs successfully written.
    pub pairs_scored: usize,
    /// case_ids where at least one whitelisted metric returned `Err`
    /// from `compute_metric` AND no metric for that case produced a
    /// successful score. These are the cases that were silently ignored
    /// before we surfaced this signal — typically long-response cases
    /// where the judge LLM timed out.
    pub cases_all_metrics_failed: Vec<String>,
}

impl ScoreRunSummary {
    pub fn has_silent_failures(&self) -> bool {
        !self.cases_all_metrics_failed.is_empty()
    }
}

/// Apply every metric in `suite.metrics` to every successful case_result
/// of the given run. Returns a summary including a list of case_ids whose
/// metrics ALL errored (i.e. silent-failure cases the caller should
/// surface explicitly).
///
/// Uses [`DEFAULT_SCORING_CONCURRENCY`] in-flight (case, metric) futures.
/// Use [`score_run_with_concurrency`] if you need a different bound (for
/// example to dial down for a flaky judge or up for a high-quota one).
pub async fn score_run(
    repo: &EvalRepo,
    run_id: &str,
    suite: &Suite,
    judge: &dyn JudgeClient,
) -> Result<ScoreRunSummary> {
    score_run_with_concurrency(repo, run_id, suite, judge, DEFAULT_SCORING_CONCURRENCY).await
}

/// Same as [`score_run`] but with a caller-supplied concurrency bound.
///
/// Concurrency is across *(case, metric)* pairs — each pair issues one
/// judge call (or runs deterministically), and `concurrency=1` gives
/// strictly sequential behavior (useful for tests that need deterministic
/// progress order). The repo writes are still serialized at the SQLite
/// connection layer, so this does not introduce a write race.
pub async fn score_run_with_concurrency(
    repo: &EvalRepo,
    run_id: &str,
    suite: &Suite,
    judge: &dyn JudgeClient,
    concurrency: usize,
) -> Result<ScoreRunSummary> {
    score_run_with_concurrency_and_progress(repo, run_id, suite, judge, concurrency, false).await
}

/// Score a run with both an explicit concurrency bound and an optional
/// CLI-style progress bar. `show_progress=true` renders an indicatif bar
/// keyed on (case, metric)-pair completion; `false` is a no-op so library
/// callers (tests, programmatic re-scoring) don't see any output.
pub async fn score_run_with_concurrency_and_progress(
    repo: &EvalRepo,
    run_id: &str,
    suite: &Suite,
    judge: &dyn JudgeClient,
    concurrency: usize,
    show_progress: bool,
) -> Result<ScoreRunSummary> {
    let concurrency = concurrency.max(1);
    let mut summary = ScoreRunSummary::default();
    if suite.metrics.is_empty() {
        debug!(suite = %suite.name, "no metrics declared; skipping scoring");
        return Ok(summary);
    }
    let mut case_lookup: HashMap<String, &Case> = HashMap::with_capacity(suite.cases.len());
    for c in &suite.cases {
        case_lookup.insert(c.id.clone(), c);
    }

    let mut results = repo.load_case_results(run_id)?;

    // ── Phase 1: build a flat (case_idx, spec, case, response, tool_calls)
    //    task list, applying the per-case metric whitelist up-front. We index
    //    into `results` so the eventual write-back can mutate `r.metric_scores`
    //    in place without re-cloning.
    type Task<'a> = (usize, &'a MetricSpec, &'a Case, &'a str, &'a [RecordedToolCall]);
    let mut tasks: Vec<Task<'_>> = Vec::new();
    for (idx, r) in results.iter().enumerate() {
        if r.error.is_some() {
            continue;
        }
        let lookup_id = strip_repetition_suffix(&r.case_id);
        let Some(case) = case_lookup.get(lookup_id) else {
            warn!(case_id = %r.case_id, "case not found in suite; skipping");
            continue;
        };
        for spec in &suite.metrics {
            if let Some(allowed) = case.metrics.as_ref() {
                if !allowed.iter().any(|m| m == &spec.name) {
                    continue;
                }
            }
            tasks.push((idx, spec, case, r.plaw_response.as_str(), r.tool_calls.as_slice()));
        }
    }

    // ── Phase 2: drive all (case, metric) futures concurrently with a
    //    bounded in-flight cap. Aggregation per case happens after the stream
    //    drains. We collect the per-task outcome rather than write inline so
    //    repo writes happen once per case (matching the legacy semantics).
    let total_pairs = tasks.len() as u64;
    let pb = if show_progress && total_pairs > 0 {
        let pb = ProgressBar::new(total_pairs);
        pb.set_style(
            ProgressStyle::with_template(
                "scoring {pos:>5}/{len:5} [{wide_bar:.cyan/blue}] {elapsed_precise} {msg}",
            )
            .unwrap()
            .progress_chars("=> "),
        );
        Some(pb)
    } else {
        None
    };
    let stream = stream::iter(tasks.into_iter().map(|(idx, spec, case, response, tool_calls)| {
        let pb = pb.clone();
        async move {
            let res = compute_metric(spec, case, response, tool_calls, judge).await;
            if let Some(pb) = &pb {
                pb.inc(1);
            }
            (idx, spec.name.clone(), res)
        }
    }))
    .buffer_unordered(concurrency);
    let outcomes: Vec<(usize, String, Result<Option<MetricScore>>)> = stream.collect().await;
    if let Some(pb) = pb {
        pb.finish_with_message("done");
    }

    // ── Phase 3: group by case_idx, derive summary fields, persist.
    #[derive(Default)]
    struct CaseAggregate {
        scored: HashMap<String, MetricScore>,
        any_succeeded: bool,
        any_errored: bool,
    }
    let mut per_case: HashMap<usize, CaseAggregate> = HashMap::new();
    for (idx, name, res) in outcomes {
        let entry = per_case.entry(idx).or_default();
        match res {
            Ok(Some(score)) => {
                entry.scored.insert(name, score);
                entry.any_succeeded = true;
                summary.pairs_scored += 1;
            }
            Ok(None) => {
                debug!(metric = %name, "metric returned no score");
            }
            Err(e) => {
                let case_id = results.get(idx).map(|r| r.case_id.as_str()).unwrap_or("?");
                warn!(metric = %name, case_id = %case_id, error = %e, "metric scoring failed");
                entry.any_errored = true;
            }
        }
    }

    for (idx, agg) in per_case {
        let r = &mut results[idx];
        if agg.any_errored && !agg.any_succeeded {
            summary.cases_all_metrics_failed.push(r.case_id.clone());
        }
        let mut merged = std::mem::take(&mut r.metric_scores);
        for (k, v) in agg.scored {
            merged.insert(k, v);
        }
        repo.update_metric_scores(run_id, &r.case_id, &merged)?;
    }

    if summary.has_silent_failures() {
        warn!(
            run_id = %run_id,
            cases_failed = summary.cases_all_metrics_failed.len(),
            "metric scoring: cases with all-metrics-errored — investigate judge timeouts/rate limits"
        );
    }
    Ok(summary)
}

/// Produce a single metric score for one case. Returns `Ok(None)` when
/// the metric is recognised but cannot be applied (e.g. keyword coverage
/// without expected keywords).
pub async fn compute_metric(
    spec: &MetricSpec,
    case: &Case,
    response_text: &str,
    tool_calls: &[RecordedToolCall],
    judge: &dyn JudgeClient,
) -> Result<Option<MetricScore>> {
    match spec.name.as_str() {
        "tool_call_accuracy" => {
            let expected = case
                .expected
                .as_ref()
                .map(|e| e.tool_sequence.clone())
                .unwrap_or_default();
            // No expected_tool_sequence + no actual calls → trivially correct,
            // but uninformative. Skip rather than dilute the metric mean.
            if expected.is_empty() && tool_calls.is_empty() {
                return Ok(None);
            }
            let names: Vec<String> = tool_calls.iter().map(|t| t.name.clone()).collect();
            let args: Vec<serde_json::Value> = tool_calls.iter().map(|t| t.args.clone()).collect();
            let s = tool_summarise(&names, &args, &expected);
            // Composite scalar: mean of selection_f1 and (1 − redundant_rate),
            // with arg_validity acting as a penalty when low. This matches
            // the framing in docs/eval/methodology.md §10.
            let composite = (s.selection_f1 * 0.6
                + (1.0 - s.redundant_call_rate) * 0.2
                + s.arg_validity_rate * 0.2)
                .clamp(0.0, 1.0);
            Ok(Some(MetricScore {
                value: composite,
                raw: json!({
                    "selection_precision": s.selection_precision,
                    "selection_recall": s.selection_recall,
                    "selection_f1": s.selection_f1,
                    "arg_validity_rate": s.arg_validity_rate,
                    "redundant_call_rate": s.redundant_call_rate,
                    "n_calls": s.n_calls,
                    "expected": expected,
                    "actual": names,
                }),
                judge_model: "deterministic".into(),
            }))
        }
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
                metrics: None,
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
            tool_calls: Vec::new(),
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
        let result = compute_metric(&suite.metrics[0], case, "Paris", &[], &judge)
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
        let score = compute_metric(
            &suite.metrics[0],
            case,
            "Paris is the capital.",
            &[],
            &judge,
        )
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
        let result = compute_metric(&spec, case, "anything", &[], &judge)
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

        let summary = score_run(&repo, "r1", &suite, &judge).await.unwrap();
        assert_eq!(summary.pairs_scored, 1);
        assert!(summary.cases_all_metrics_failed.is_empty());

        let stored = repo.load_case_results("r1").unwrap();
        assert_eq!(stored.len(), 1);
        let score = stored[0].metric_scores.get("keyword_coverage").unwrap();
        assert!((score.value - 1.0).abs() < 1e-12);
    }

    /// Test-only judge that always errors. Reproduces the silent-failure
    /// situation where every judge call fails (timeout / rate limit / etc).
    struct AlwaysErrJudge;
    #[async_trait::async_trait]
    impl JudgeClient for AlwaysErrJudge {
        fn family(&self) -> JudgeFamily {
            JudgeFamily::Kimi
        }
        fn model(&self) -> &str {
            "always-err"
        }
        async fn complete(
            &self,
            _system: &str,
            _user: &str,
        ) -> Result<crate::judges::client::JudgeCompletion> {
            Err(anyhow::anyhow!("simulated judge timeout"))
        }
    }

    #[tokio::test]
    async fn score_run_records_silent_failure_when_all_metrics_error() {
        // Reproduces the bug we found in the post-Phase-2 baseline: when
        // every metric scoring attempt errors (e.g. judge timeout on a
        // long response), the case's metric_scores is left empty and —
        // pre-fix — that's invisible at the caller.
        let repo = EvalRepo::open_in_memory().unwrap();
        let mut suite = suite_with_metrics(vec![MetricSpec {
            name: "g_eval".into(),
            judge: None,
            params: Default::default(),
        }]);
        // Pin the case to whitelist g_eval so the loop attempts it.
        suite.cases[0].metrics = Some(vec!["g_eval".into()]);

        let run = crate::storage::Run {
            id: "r_err".into(),
            suite_name: suite.name.clone(),
            suite_version: suite.version.clone(),
            started_at: 0,
            finished_at: Some(1),
            plaw_commit: "x".into(),
            model_version: "test".into(),
            config_hash: "h".into(),
            n_total: 1,
            n_completed: 1,
            n_failed: 0,
        };
        repo.insert_run(&run).unwrap();
        let mut cr = case_result(&suite.cases[0].id, "anything");
        cr.run_id = "r_err".into();
        repo.insert_case_result(&cr).unwrap();

        let judge = AlwaysErrJudge;
        let summary = score_run(&repo, "r_err", &suite, &judge).await.unwrap();
        assert_eq!(summary.pairs_scored, 0);
        assert_eq!(summary.cases_all_metrics_failed.len(), 1);
        assert!(summary.has_silent_failures());
        assert_eq!(
            &summary.cases_all_metrics_failed[0],
            &suite.cases[0].id
        );
    }

    /// Build a (suite, repo, run_id) triple seeded with `n_cases` deterministic
    /// case_results. Each case's response contains "Paris" so keyword_coverage
    /// scores 1.0; that lets a parity test check that pairs_scored matches
    /// regardless of `concurrency`.
    fn seed_keyword_run(n_cases: usize) -> (Suite, EvalRepo, String) {
        let repo = EvalRepo::open_in_memory().unwrap();
        let suite = Suite {
            name: "parity".into(),
            version: "1.0.0".into(),
            description: "".into(),
            default_judge: JudgeSpec {
                model: "kimi-k2.5".into(),
                provider: "kimi".into(),
                temperature: 0.0,
                mode: JudgeMode::default(),
            },
            metrics: vec![MetricSpec {
                name: "keyword_coverage".into(),
                judge: None,
                params: Default::default(),
            }],
            cases: (0..n_cases)
                .map(|i| Case {
                    id: format!("c{i}"),
                    input: CaseInput::Chat {
                        messages: vec![ChatMsg {
                            role: ChatRole::User,
                            content: format!("Question {i}?"),
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
                    metrics: None,
                })
                .collect(),
        };
        let run_id = "rparity".to_string();
        let run = crate::storage::Run {
            id: run_id.clone(),
            suite_name: suite.name.clone(),
            suite_version: suite.version.clone(),
            started_at: 0,
            finished_at: Some(1),
            plaw_commit: "x".into(),
            model_version: "kimi-k2.5".into(),
            config_hash: "h".into(),
            n_total: n_cases,
            n_completed: n_cases,
            n_failed: 0,
        };
        repo.insert_run(&run).unwrap();
        for i in 0..n_cases {
            let mut cr = case_result(&format!("c{i}"), "Paris is the answer.");
            cr.run_id = run_id.clone();
            repo.insert_case_result(&cr).unwrap();
        }
        (suite, repo, run_id)
    }

    #[tokio::test]
    async fn score_run_with_concurrency_matches_sequential() {
        // Concurrency must not change the outcome: same pairs scored, same
        // stored values. We compare concurrency=1 (strictly sequential) to
        // concurrency=8 (default production setting) over a 12-case run.
        let (suite_seq, repo_seq, run_seq) = seed_keyword_run(12);
        let judge_seq = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let summary_seq = score_run_with_concurrency(&repo_seq, &run_seq, &suite_seq, &judge_seq, 1)
            .await
            .unwrap();

        let (suite_par, repo_par, run_par) = seed_keyword_run(12);
        let judge_par = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let summary_par = score_run_with_concurrency(&repo_par, &run_par, &suite_par, &judge_par, 8)
            .await
            .unwrap();

        assert_eq!(summary_seq.pairs_scored, summary_par.pairs_scored);
        assert_eq!(summary_seq.pairs_scored, 12);
        assert!(summary_seq.cases_all_metrics_failed.is_empty());
        assert!(summary_par.cases_all_metrics_failed.is_empty());

        let stored_seq = repo_seq.load_case_results(&run_seq).unwrap();
        let stored_par = repo_par.load_case_results(&run_par).unwrap();
        for r in stored_seq.iter().chain(stored_par.iter()) {
            let s = r.metric_scores.get("keyword_coverage").unwrap();
            assert!((s.value - 1.0).abs() < 1e-12);
        }
    }

    #[tokio::test]
    async fn score_run_default_concurrency_matches_explicit_default() {
        // Sanity: score_run() without an explicit concurrency must behave
        // identically to score_run_with_concurrency(..., DEFAULT_SCORING_CONCURRENCY).
        let (suite_a, repo_a, run_a) = seed_keyword_run(5);
        let judge_a = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let s_a = score_run(&repo_a, &run_a, &suite_a, &judge_a).await.unwrap();

        let (suite_b, repo_b, run_b) = seed_keyword_run(5);
        let judge_b = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let s_b = score_run_with_concurrency(
            &repo_b,
            &run_b,
            &suite_b,
            &judge_b,
            DEFAULT_SCORING_CONCURRENCY,
        )
        .await
        .unwrap();

        assert_eq!(s_a.pairs_scored, s_b.pairs_scored);
    }

    #[tokio::test]
    async fn score_run_concurrency_zero_falls_back_to_one() {
        // Defensive: concurrency=0 would be a buffer_unordered no-op (it
        // requires >= 1). We clamp to 1 inside score_run_with_concurrency
        // so a misconfigured caller still makes progress.
        let (suite, repo, run) = seed_keyword_run(3);
        let judge = MockJudgeClient::new(JudgeFamily::Kimi, "kimi-k2.5", vec![]);
        let summary = score_run_with_concurrency(&repo, &run, &suite, &judge, 0)
            .await
            .unwrap();
        assert_eq!(summary.pairs_scored, 3);
    }
}
