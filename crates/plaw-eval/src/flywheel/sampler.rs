//! Flywheel sampler — pick interesting case results from a finished run
//! and queue them for human review.
//!
//! Phase 1's "production trace" is necessarily an eval-driven trace
//! (Phase 3 wires real OTel traces from plaw). The sampler reads
//! `case_results` and writes `flywheel_queue` rows, optionally filtering
//! to only failed / low-score cases. Each sampled row records
//! `source_run_id` + `source_case_id` so the promoter can later rebuild
//! the case from those references.

use anyhow::{anyhow, Result};
use uuid::Uuid;

use crate::storage::{CaseResult, EvalRepo, FlywheelEntry};

/// Strategy controlling which cases the sampler keeps.
#[derive(Debug, Clone)]
pub enum SampleStrategy {
    /// Random subsample at the given rate `[0, 1]`. Deterministic with
    /// seed.
    Random { rate: f64, seed: u64 },
    /// Keep every case that has `error.is_some()`.
    FailedOnly,
    /// Keep every case whose named metric scored below the threshold.
    LowScore { metric: String, threshold: f64 },
    /// Keep all cases — useful for one-shot manual review.
    All,
}

/// Outcome of a sampling pass.
#[derive(Debug, Clone)]
pub struct SampleSummary {
    pub run_id: String,
    pub total_cases: usize,
    pub queued: usize,
}

/// Sample cases from `run_id` and enqueue them with the given target
/// suite for promotion.
pub fn sample_run(
    repo: &EvalRepo,
    run_id: &str,
    strategy: SampleStrategy,
    target_suite: Option<&str>,
) -> Result<SampleSummary> {
    let run = repo
        .load_run(run_id)?
        .ok_or_else(|| anyhow!("run '{run_id}' not found"))?;
    let cases = repo.load_case_results(run_id)?;
    let total_cases = cases.len();

    let selected: Vec<&CaseResult> = match &strategy {
        SampleStrategy::Random { rate, seed } => {
            let rate = rate.clamp(0.0, 1.0);
            let mut state = (*seed).max(1);
            cases
                .iter()
                .filter(|_| {
                    state = xorshift64star(state);
                    let draw = (state as f64) / (u64::MAX as f64);
                    draw < rate
                })
                .collect()
        }
        SampleStrategy::FailedOnly => cases.iter().filter(|c| c.error.is_some()).collect(),
        SampleStrategy::LowScore { metric, threshold } => cases
            .iter()
            .filter(|c| {
                c.metric_scores
                    .get(metric)
                    .map(|s| s.value < *threshold)
                    .unwrap_or(false)
            })
            .collect(),
        SampleStrategy::All => cases.iter().collect(),
    };

    let now = chrono::Utc::now().timestamp();
    let mut queued = 0usize;
    for case in &selected {
        let judge_score = primary_metric_score(case);
        let entry = FlywheelEntry {
            id: Uuid::new_v4().to_string(),
            // Use the eval-internal identifier; Phase 3 will overwrite
            // with a real OTel trace id when one is available.
            trace_id: format!("{}:{}", run.id, case.case_id),
            sampled_at: now,
            judge_score,
            review_status: "pending".into(),
            reviewed_at: None,
            promoted_to_suite: None,
            promoted_case_id: None,
            source_run_id: Some(run.id.clone()),
            source_case_id: Some(case.case_id.clone()),
            target_suite: target_suite.map(|s| s.to_string()),
        };
        repo.flywheel_enqueue(&entry)?;
        queued += 1;
    }
    Ok(SampleSummary {
        run_id: run.id,
        total_cases,
        queued,
    })
}

/// Pick a representative metric score for the queue entry's `judge_score`
/// summary column. Prefers `g_eval` if present, otherwise the smallest
/// observed score (worst metric) so reviewers see the weakest dimension
/// first.
fn primary_metric_score(case: &CaseResult) -> Option<f64> {
    if let Some(s) = case.metric_scores.get("g_eval") {
        return Some(s.value);
    }
    case.metric_scores
        .values()
        .map(|s| s.value)
        .fold(None, |acc, v| match acc {
            Some(prev) if prev <= v => Some(prev),
            _ => Some(v),
        })
}

/// xorshift64* — same PRNG used by `stats::ci::bootstrap_ci`.
#[inline]
fn xorshift64star(mut state: u64) -> u64 {
    state ^= state >> 12;
    state ^= state << 25;
    state ^= state >> 27;
    state.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{MetricScore, Run};
    use std::collections::HashMap;

    fn seed_run(repo: &EvalRepo, n: usize, scores: &[f64]) -> String {
        let run = Run {
            id: format!("run-{}", Uuid::new_v4()),
            suite_name: "smoke".into(),
            suite_version: "1.0.0".into(),
            started_at: 0,
            finished_at: Some(1),
            plaw_commit: "x".into(),
            model_version: "kimi".into(),
            config_hash: "h".into(),
            n_total: n,
            n_completed: n,
            n_failed: 0,
        };
        repo.insert_run(&run).unwrap();
        for (i, score) in scores.iter().enumerate().take(n) {
            let mut metric_scores = HashMap::new();
            metric_scores.insert(
                "g_eval".into(),
                MetricScore {
                    value: *score,
                    raw: serde_json::Value::Null,
                    judge_model: "mock".into(),
                },
            );
            let cr = CaseResult {
                run_id: run.id.clone(),
                case_id: format!("c{i}"),
                case_cluster: None,
                plaw_response: format!("response {i}"),
                plaw_trace_id: None,
                metric_scores,
                latency_ms: 0,
                tokens_in: 0,
                tokens_out: 0,
                cache_read_tokens: 0,
                error: if *score < 0.0 {
                    Some("forced failure".into())
                } else {
                    None
                },
            };
            repo.insert_case_result(&cr).unwrap();
        }
        run.id
    }

    #[test]
    fn random_sampling_is_deterministic() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let scores: Vec<f64> = (0..50).map(|i| (i as f64) / 50.0).collect();
        let run_id = seed_run(&repo, 50, &scores);

        let s1 = sample_run(
            &repo,
            &run_id,
            SampleStrategy::Random { rate: 0.5, seed: 7 },
            Some("chat_quality"),
        )
        .unwrap();

        // Reset and resample with the same seed against a fresh DB
        let repo2 = EvalRepo::open_in_memory().unwrap();
        let run_id2 = seed_run(&repo2, 50, &scores);
        let s2 = sample_run(
            &repo2,
            &run_id2,
            SampleStrategy::Random { rate: 0.5, seed: 7 },
            Some("chat_quality"),
        )
        .unwrap();

        assert_eq!(s1.queued, s2.queued);
        assert!(s1.queued > 0);
        assert!(s1.queued < 50);
    }

    #[test]
    fn failed_only_keeps_only_error_cases() {
        let repo = EvalRepo::open_in_memory().unwrap();
        // Three failures, two successes (negative score => failure).
        let scores = vec![0.8, -1.0, 0.5, -1.0, -1.0];
        let run_id = seed_run(&repo, 5, &scores);

        let summary = sample_run(&repo, &run_id, SampleStrategy::FailedOnly, None).unwrap();
        assert_eq!(summary.queued, 3);
        let pending = repo.flywheel_list_pending(10).unwrap();
        assert!(pending.iter().all(|e| e.source_run_id.is_some()));
    }

    #[test]
    fn low_score_filters_by_threshold() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let scores = vec![0.95, 0.4, 0.55, 0.2, 0.85];
        let run_id = seed_run(&repo, 5, &scores);

        let summary = sample_run(
            &repo,
            &run_id,
            SampleStrategy::LowScore {
                metric: "g_eval".into(),
                threshold: 0.5,
            },
            Some("chat_quality"),
        )
        .unwrap();
        // 0.4 and 0.2 are below 0.5
        assert_eq!(summary.queued, 2);
    }

    #[test]
    fn rate_zero_queues_nothing() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let scores: Vec<f64> = (0..10).map(|i| i as f64 / 10.0).collect();
        let run_id = seed_run(&repo, 10, &scores);

        let summary = sample_run(
            &repo,
            &run_id,
            SampleStrategy::Random { rate: 0.0, seed: 1 },
            None,
        )
        .unwrap();
        assert_eq!(summary.queued, 0);
    }

    #[test]
    fn rate_one_queues_everything() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let scores: Vec<f64> = (0..10).map(|i| i as f64 / 10.0).collect();
        let run_id = seed_run(&repo, 10, &scores);

        let summary = sample_run(
            &repo,
            &run_id,
            SampleStrategy::Random { rate: 1.0, seed: 1 },
            None,
        )
        .unwrap();
        assert_eq!(summary.queued, 10);
    }

    #[test]
    fn missing_run_is_an_error() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let err = sample_run(&repo, "does-not-exist", SampleStrategy::All, None).unwrap_err();
        assert!(format!("{err:#}").contains("not found"));
    }
}
