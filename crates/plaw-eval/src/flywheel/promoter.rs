//! Flywheel promoter — turn an approved queue entry into a new Case
//! appended to a target suite's `cases.toml`.
//!
//! The pipeline:
//! 1. Load the queue entry (must be in `approved` status).
//! 2. Resolve the source run + case to recover the original input shape.
//! 3. Synthesise a new Case ID (collision-resistant) tagged with
//!    `source = "flywheel"` and `promoted_at`.
//! 4. Append the rendered TOML to the target suite file.
//! 5. Stamp the queue entry with `promoted_to_suite` + `promoted_case_id`
//!    and flip status to `promoted`.
//!
//! The promoter never auto-edits the suite's `cases.toml` in place
//! beyond appending — operators are expected to inspect the diff and
//! commit it themselves. Auto-commit hooks are explicitly out of scope
//! for Phase 1.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;

use crate::storage::EvalRepo;
use crate::suite::{load_suite, Case, CaseExpected, CaseInput, ChatMsg, ChatRole, Suite};

/// Outcome of a successful promotion.
#[derive(Debug, Clone)]
pub struct PromotionResult {
    pub queue_id: String,
    pub target_suite_path: String,
    pub new_case_id: String,
    pub appended_bytes: usize,
}

/// Promote one approved queue entry into the named suite.
pub fn promote(
    repo: &EvalRepo,
    queue_id: &str,
    target_suite_path: impl AsRef<Path>,
) -> Result<PromotionResult> {
    let target_suite_path = target_suite_path.as_ref();

    // 1. Resolve the queue entry.
    let entry = repo
        .flywheel_get(queue_id)?
        .ok_or_else(|| anyhow!("flywheel entry '{queue_id}' not found"))?;
    if entry.review_status != "approved" {
        return Err(anyhow!(
            "flywheel entry '{queue_id}' is in status '{}'; only approved entries can be promoted",
            entry.review_status
        ));
    }

    let source_run_id = entry.source_run_id.as_deref().ok_or_else(|| {
        anyhow!("queue entry '{queue_id}' has no source_run_id (cannot rebuild case)")
    })?;
    let source_case_id = entry
        .source_case_id
        .as_deref()
        .ok_or_else(|| anyhow!("queue entry '{queue_id}' has no source_case_id"))?;

    // 2. Load the source run + case_result.
    let case_result = repo
        .load_case_results(source_run_id)?
        .into_iter()
        .find(|c| c.case_id == source_case_id)
        .ok_or_else(|| {
            anyhow!("source case '{source_case_id}' not found in run '{source_run_id}'")
        })?;

    // We need the original Case to recover input + expected. Look it up
    // by matching case_id in the target suite; if it's already there
    // (the operator wants to re-record an interesting variant) we use
    // it as the input template.
    let target_suite = load_suite(target_suite_path)
        .with_context(|| format!("loading target suite at {}", target_suite_path.display()))?;
    let template_case = target_suite
        .cases
        .iter()
        .find(|c| c.id == source_case_id)
        .cloned();

    // 3. Synthesise a new case id.
    let new_case_id = synthesise_case_id(&target_suite, source_case_id);

    // 4. Build the new Case.
    let new_case = build_promoted_case(
        &new_case_id,
        source_case_id,
        &case_result.plaw_response,
        template_case,
    );

    // 5. Append to the suite TOML.
    let appended = append_case_to_suite(target_suite_path, &new_case)?;

    // 6. Stamp the queue entry.
    repo.flywheel_record_promotion(queue_id, &target_suite.name, &new_case_id)?;

    Ok(PromotionResult {
        queue_id: queue_id.to_string(),
        target_suite_path: target_suite_path.display().to_string(),
        new_case_id,
        appended_bytes: appended,
    })
}

/// Build a deterministic new case id of the form
/// `<source>-flywheel-<NNN>`, finding the smallest unused suffix.
fn synthesise_case_id(suite: &Suite, source_case_id: &str) -> String {
    let mut n = 1usize;
    loop {
        let candidate = format!("{source_case_id}-flywheel-{n:03}");
        if !suite.cases.iter().any(|c| c.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Construct the Case to be written. When a template_case is available
/// we copy its input/expected (the operator wanted to track a variant
/// of an existing case). Otherwise we synthesise a chat-shaped input
/// with the recovered plaw response as a candidate `expected.answer`.
fn build_promoted_case(
    new_case_id: &str,
    source_case_id: &str,
    plaw_response: &str,
    template_case: Option<Case>,
) -> Case {
    let now = Utc::now().to_rfc3339();
    let mut tags = vec!["flywheel".to_string()];

    let (input, expected) = match template_case {
        Some(tc) => {
            tags.extend(tc.tags.iter().filter(|t| *t != "flywheel").cloned());
            let expected = tc.expected.unwrap_or_default();
            (tc.input, Some(expected))
        }
        None => {
            // Synthesise a single-turn chat case derived from the trace.
            // The operator can edit it post-promotion; we just need a
            // valid TOML row.
            let messages = vec![ChatMsg {
                role: ChatRole::User,
                content: format!("Promoted from trace of case `{source_case_id}`. Edit me."),
            }];
            let expected = CaseExpected {
                answer: Some(plaw_response.to_string()),
                ..CaseExpected::default()
            };
            (CaseInput::Chat { messages }, Some(expected))
        }
    };

    Case {
        id: new_case_id.into(),
        input,
        expected,
        tags,
        cluster_id: None,
        source: "flywheel".into(),
        promoted_at: Some(now),
    }
}

/// Append a single Case as TOML to the suite file. Returns bytes
/// written. The file is opened in append mode so existing cases are
/// preserved untouched.
fn append_case_to_suite(target_suite_path: &Path, case: &Case) -> Result<usize> {
    let rendered = render_case_toml(case)?;
    let mut existing = fs::read_to_string(target_suite_path)
        .with_context(|| format!("reading {}", target_suite_path.display()))?;
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push('\n');
    existing.push_str(&rendered);
    fs::write(target_suite_path, &existing)
        .with_context(|| format!("writing {}", target_suite_path.display()))?;
    Ok(rendered.len())
}

/// Render a single Case as a TOML `[[cases]]` block. We don't use
/// `toml::to_string` because that would emit a top-level wrapper; we
/// want the row to slot into the existing suite array.
fn render_case_toml(case: &Case) -> Result<String> {
    // Build a tiny wrapper struct so serde produces the right shape.
    #[derive(serde::Serialize)]
    struct Wrapper<'a> {
        cases: [&'a Case; 1],
    }
    let wrapper = Wrapper { cases: [case] };
    let mut text = toml::to_string_pretty(&wrapper).context("serialising case to TOML")?;
    // Ensure trailing newline so subsequent appends compose cleanly.
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// Read a previously promoted case directly from a suite file by id —
/// useful for tests and for operator review.
pub fn read_promoted_case(target_suite_path: &Path, case_id: &str) -> Result<Option<Case>> {
    let suite = load_suite(target_suite_path)?;
    Ok(suite.cases.into_iter().find(|c| c.id == case_id))
}

/// Suppress an unused-warning when the helper struct is referenced only
/// via tests — we need the type alias visible for `Case` down here.
#[allow(dead_code)]
fn _ensure_case_imported(_case: &Case, _hash_map: HashMap<String, String>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{CaseResult, FlywheelEntry, MetricScore, Run};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn write_minimal_suite(dir: &Path, name: &str, original_case_id: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("cases.toml");
        let toml = format!(
            r#"
name = "{name}"
version = "1.0.0"
description = "test"

[default_judge]
model = "kimi-k2.5"
provider = "kimi"
mode = {{ kind = "pairwise", dual_pass = true }}

[[cases]]
id = "{original_case_id}"
[cases.input]
kind = "chat"
messages = [{{ role = "user", content = "Original case prompt" }}]
[cases.expected]
answer_keywords = ["original"]
"#
        );
        fs::write(&path, toml).unwrap();
        path
    }

    fn seed_repo(repo: &EvalRepo, run_id: &str, case_id: &str, response: &str) {
        repo.insert_run(&Run {
            id: run_id.into(),
            suite_name: "smoke".into(),
            suite_version: "1.0.0".into(),
            started_at: 0,
            finished_at: Some(1),
            plaw_commit: "x".into(),
            model_version: "kimi".into(),
            config_hash: "h".into(),
            n_total: 1,
            n_completed: 1,
            n_failed: 0,
        })
        .unwrap();
        let mut metric_scores = HashMap::new();
        metric_scores.insert(
            "g_eval".into(),
            MetricScore {
                value: 0.4,
                raw: serde_json::Value::Null,
                judge_model: "mock".into(),
            },
        );
        repo.insert_case_result(&CaseResult {
            run_id: run_id.into(),
            case_id: case_id.into(),
            case_cluster: None,
            plaw_response: response.into(),
            plaw_trace_id: None,
            metric_scores,
            latency_ms: 0,
            tokens_in: 0,
            tokens_out: 0,
            cache_read_tokens: 0,
            error: None,
            tool_calls: Vec::new(),
        })
        .unwrap();
    }

    fn enqueue_approved(repo: &EvalRepo, queue_id: &str, run_id: &str, case_id: &str) {
        repo.flywheel_enqueue(&FlywheelEntry {
            id: queue_id.into(),
            trace_id: format!("{run_id}:{case_id}"),
            sampled_at: 0,
            judge_score: Some(0.4),
            review_status: "approved".into(),
            reviewed_at: Some(1),
            promoted_to_suite: None,
            promoted_case_id: None,
            source_run_id: Some(run_id.into()),
            source_case_id: Some(case_id.into()),
            target_suite: None,
        })
        .unwrap();
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("plaw-eval-promoter-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn promote_appends_new_case_with_flywheel_metadata() {
        let tmp = tempdir();
        let suite_dir = tmp.join("chat_quality");
        let suite_path = write_minimal_suite(&suite_dir, "chat_quality", "case-1");

        let repo = EvalRepo::open_in_memory().unwrap();
        seed_repo(&repo, "run-1", "case-1", "production-grade response");
        enqueue_approved(&repo, "f1", "run-1", "case-1");

        let result = promote(&repo, "f1", &suite_path).unwrap();
        assert_eq!(result.new_case_id, "case-1-flywheel-001");
        assert!(result.appended_bytes > 0);

        // Suite should now have 2 cases including the promoted one.
        let suite = load_suite(&suite_path).unwrap();
        assert_eq!(suite.cases.len(), 2);
        let promoted = suite
            .cases
            .iter()
            .find(|c| c.id == "case-1-flywheel-001")
            .unwrap();
        assert_eq!(promoted.source, "flywheel");
        assert!(promoted.promoted_at.is_some());
        assert!(promoted.tags.contains(&"flywheel".into()));

        // Queue entry should be marked promoted.
        let entry = repo.flywheel_get("f1").unwrap().unwrap();
        assert_eq!(entry.review_status, "promoted");
        assert_eq!(entry.promoted_to_suite.as_deref(), Some("chat_quality"));
        assert_eq!(
            entry.promoted_case_id.as_deref(),
            Some("case-1-flywheel-001")
        );
    }

    #[test]
    fn rejects_non_approved_entries() {
        let tmp = tempdir();
        let suite_path = write_minimal_suite(&tmp.join("c"), "c", "case-1");
        let repo = EvalRepo::open_in_memory().unwrap();
        seed_repo(&repo, "run-1", "case-1", "x");
        // Enqueue but leave status pending
        repo.flywheel_enqueue(&FlywheelEntry {
            id: "f1".into(),
            trace_id: "run-1:case-1".into(),
            sampled_at: 0,
            judge_score: None,
            review_status: "pending".into(),
            reviewed_at: None,
            promoted_to_suite: None,
            promoted_case_id: None,
            source_run_id: Some("run-1".into()),
            source_case_id: Some("case-1".into()),
            target_suite: None,
        })
        .unwrap();

        let err = promote(&repo, "f1", &suite_path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("approved"));
    }

    #[test]
    fn synthesises_unique_case_id_when_collision() {
        // First promotion creates -flywheel-001. Second should be -002.
        let tmp = tempdir();
        let suite_path = write_minimal_suite(&tmp.join("c"), "c", "case-1");
        let repo = EvalRepo::open_in_memory().unwrap();
        seed_repo(&repo, "run-1", "case-1", "first");
        seed_repo(&repo, "run-2", "case-1", "second");
        enqueue_approved(&repo, "f1", "run-1", "case-1");
        enqueue_approved(&repo, "f2", "run-2", "case-1");

        let r1 = promote(&repo, "f1", &suite_path).unwrap();
        let r2 = promote(&repo, "f2", &suite_path).unwrap();
        assert_eq!(r1.new_case_id, "case-1-flywheel-001");
        assert_eq!(r2.new_case_id, "case-1-flywheel-002");

        let suite = load_suite(&suite_path).unwrap();
        assert_eq!(suite.cases.len(), 3);
    }

    #[test]
    fn synthesises_case_when_template_missing() {
        // Suite has its own cases but not the source one — promoter falls
        // back to a synthesised chat input.
        let tmp = tempdir();
        let dir = tmp.join("c");
        let suite_path = write_minimal_suite(&dir, "c", "different-case");
        let repo = EvalRepo::open_in_memory().unwrap();
        seed_repo(&repo, "run-1", "case-1", "synthetic-answer");
        enqueue_approved(&repo, "f1", "run-1", "case-1");

        let result = promote(&repo, "f1", &suite_path).unwrap();
        let promoted = read_promoted_case(&suite_path, &result.new_case_id)
            .unwrap()
            .unwrap();
        match promoted.input {
            CaseInput::Chat { messages } => {
                assert_eq!(messages.len(), 1);
                assert!(messages[0].content.contains("Promoted from trace"));
            }
            _ => panic!("expected chat input"),
        }
        assert_eq!(
            promoted.expected.unwrap().answer.as_deref(),
            Some("synthetic-answer")
        );
    }
}
