//! LLM-driven memory consolidation: collapse duplicate / restated / updated
//! facts into a single canonical entry, off-loop and **reversibly**.
//!
//! Safe-by-design. The only mutation is [`Memory::supersede`], which stamps a
//! row's `valid_to` and writes one merged canonical row — nothing is
//! hard-deleted, so every consolidation stays recoverable through the
//! bi-temporal history (`recall_as_of`). A per-pass change cap
//! ([`ConsolidationOptions::max_merges`]) bounds blast radius, and the default
//! is dry-run (decide + report, mutate nothing).
//!
//! The merge primitive composes [`Memory::supersede`]: calling
//! `supersede(id, key, merged)` for every row in a duplicate set, all under the
//! same canonical `key`, retires each original (and each intermediate
//! canonical via the partial-unique-key collision rule) and leaves exactly one
//! live canonical row — no new trait surface, no hard deletes.

use crate::memory::traits::{Memory, MemoryEntry};
use crate::providers::Provider;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashSet;

/// What to do with a candidate memory and its near-duplicate neighbours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsolidationOp {
    /// Distinct fact — leave everything untouched.
    Noop,
    /// The rows in `supersede_ids` all describe the same fact. Retire each of
    /// them (always including the candidate) and write `merged_content` as the
    /// single live canonical row under `key`.
    Merge {
        key: String,
        merged_content: String,
        supersede_ids: Vec<String>,
    },
}

/// The decision boundary: an LLM in production ([`LlmDecider`]), a canned
/// responder in tests. Isolating it keeps the orchestrator deterministically
/// testable without a live model.
#[async_trait]
pub trait ConsolidationDecider: Send + Sync {
    /// Decide what to do with `candidate` given its similar `neighbors`.
    async fn decide(
        &self,
        candidate: &MemoryEntry,
        neighbors: &[MemoryEntry],
    ) -> Result<ConsolidationOp>;
}

/// Bounds for a single consolidation pass.
#[derive(Debug, Clone)]
pub struct ConsolidationOptions {
    /// Max candidate (most-recent live) memories to examine.
    pub limit: usize,
    /// Max merges to APPLY this pass — the runaway guard. Further merges are
    /// still reported (`merges_planned`) but not applied.
    pub max_merges: usize,
    /// Similar neighbours to retrieve per candidate.
    pub neighbors: usize,
    /// When `false` (the default), decide + report but mutate nothing.
    pub apply: bool,
}

impl Default for ConsolidationOptions {
    fn default() -> Self {
        Self {
            limit: 100,
            max_merges: 20,
            neighbors: 5,
            apply: false,
        }
    }
}

/// Outcome of a consolidation pass.
#[derive(Debug, Default, Clone)]
pub struct ConsolidationReport {
    /// Candidates examined.
    pub examined: usize,
    /// Merges the decider proposed (whether or not applied).
    pub merges_planned: usize,
    /// Merges actually applied (≤ `max_merges` and only when `apply`).
    pub merges_applied: usize,
    /// Rows whose `valid_to` was stamped by applied merges.
    pub rows_superseded: usize,
    /// Decider errors that were swallowed (treated as Noop).
    pub errors: usize,
    /// Human-readable, one line per planned merge.
    pub details: Vec<String>,
}

/// Run one consolidation pass over the most-recent live memories.
///
/// For each candidate that has not already been folded into an earlier merge,
/// retrieve similar same-session neighbours, ask the decider what to do, and —
/// when `opts.apply` and under the `max_merges` cap — apply merges via
/// [`Memory::supersede`]. Decider errors are swallowed as Noop (and counted)
/// so a single bad response never aborts the pass.
pub async fn run_consolidation(
    memory: &dyn Memory,
    decider: &dyn ConsolidationDecider,
    opts: &ConsolidationOptions,
) -> Result<ConsolidationReport> {
    let mut candidates = memory.list(None, None).await?;
    candidates.truncate(opts.limit);

    let mut report = ConsolidationReport::default();
    // Rows already retired (or chosen as canonical) this pass — never reprocess.
    let mut handled: HashSet<String> = HashSet::new();

    for candidate in &candidates {
        if handled.contains(&candidate.id) {
            continue;
        }
        report.examined += 1;

        // Similar neighbours, same session as the candidate, excluding the
        // candidate itself and anything already retired this pass.
        let mut neighbors = memory
            .recall(&candidate.content, opts.neighbors + 1, None)
            .await
            .unwrap_or_default();
        neighbors.retain(|n| {
            n.id != candidate.id && !handled.contains(&n.id) && n.session_id == candidate.session_id
        });
        if neighbors.is_empty() {
            continue;
        }

        let op = match decider.decide(candidate, &neighbors).await {
            Ok(op) => op,
            Err(e) => {
                report.errors += 1;
                tracing::debug!("consolidation decider error for {}: {e}", candidate.id);
                continue;
            }
        };

        let ConsolidationOp::Merge {
            key,
            merged_content,
            supersede_ids,
        } = op
        else {
            continue;
        };

        // Defence in depth: never touch a row outside the candidate + its
        // retrieved neighbours, even if the decider hallucinated an id.
        let allowed: HashSet<&str> = std::iter::once(candidate.id.as_str())
            .chain(neighbors.iter().map(|n| n.id.as_str()))
            .collect();
        let mut to_supersede: Vec<String> = supersede_ids
            .into_iter()
            .filter(|id| allowed.contains(id.as_str()))
            .collect();
        // The candidate is always part of the merge.
        if !to_supersede.iter().any(|id| id == &candidate.id) {
            to_supersede.push(candidate.id.clone());
        }
        to_supersede.sort();
        to_supersede.dedup();
        // A merge must collapse at least two rows (candidate + ≥1 neighbour).
        if to_supersede.len() < 2 || merged_content.trim().is_empty() {
            continue;
        }

        report.merges_planned += 1;
        report
            .details
            .push(format!("merge {} rows → key '{key}'", to_supersede.len()));

        // Mark every involved row handled regardless of apply, so a later
        // candidate that was a neighbour here is not reprocessed.
        for id in &to_supersede {
            handled.insert(id.clone());
        }

        if opts.apply && report.merges_applied < opts.max_merges {
            let session = candidate.session_id.as_deref();
            for id in &to_supersede {
                match memory
                    .supersede(
                        id,
                        &key,
                        &merged_content,
                        candidate.category.clone(),
                        session,
                    )
                    .await
                {
                    Ok(()) => report.rows_superseded += 1,
                    // A row may already be retired (it was a neighbour of an
                    // earlier candidate in the same set) — skip, don't abort.
                    Err(e) => tracing::debug!("supersede {id} skipped: {e}"),
                }
            }
            report.merges_applied += 1;
        }
    }

    Ok(report)
}

// ── LLM decider ─────────────────────────────────────────────────────────────

const DECIDER_SYSTEM: &str = "\
You are a memory-consolidation assistant. You are given a CANDIDATE memory and \
a list of NEIGHBOUR memories retrieved by similarity. Decide whether the \
candidate and one or more neighbours describe THE SAME underlying fact \
(duplicates, restatements, or an updated version of the same fact).

Respond with ONLY a JSON object — no prose, no code fences:
- If they are all distinct facts: {\"action\":\"noop\"}
- If the candidate and some neighbours are the same fact: \
{\"action\":\"merge\",\"key\":\"<canonical snake_case key>\",\
\"merged_content\":\"<one clear current sentence>\",\
\"redundant_ids\":[\"<id>\", ...]}

Rules:
- redundant_ids MUST be a subset of the ids shown (candidate and/or neighbours).
- merged_content must be a single, current, non-contradictory statement; if \
neighbours contradict (old vs new value), keep the NEWEST/correct value.
- Reuse the candidate's key when reasonable.
- When in doubt, choose noop — never merge unrelated facts.";

/// Production decider backed by a [`Provider`] LLM call.
pub struct LlmDecider<'a> {
    provider: &'a dyn Provider,
    model: String,
    temperature: f64,
}

impl<'a> LlmDecider<'a> {
    pub fn new(provider: &'a dyn Provider, model: impl Into<String>, temperature: f64) -> Self {
        Self {
            provider,
            model: model.into(),
            temperature,
        }
    }
}

#[async_trait]
impl ConsolidationDecider for LlmDecider<'_> {
    async fn decide(
        &self,
        candidate: &MemoryEntry,
        neighbors: &[MemoryEntry],
    ) -> Result<ConsolidationOp> {
        let prompt = build_decision_prompt(candidate, neighbors);
        let raw = self
            .provider
            .chat_with_system(Some(DECIDER_SYSTEM), &prompt, &self.model, self.temperature)
            .await?;
        Ok(parse_decision(&raw, candidate, neighbors))
    }
}

fn build_decision_prompt(candidate: &MemoryEntry, neighbors: &[MemoryEntry]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = write!(
        s,
        "CANDIDATE:\n- id: {}\n  key: {}\n  content: {}\n\nNEIGHBOURS:\n",
        candidate.id, candidate.key, candidate.content
    );
    for n in neighbors {
        let _ = write!(
            s,
            "- id: {}\n  key: {}\n  content: {}\n",
            n.id, n.key, n.content
        );
    }
    s
}

#[derive(Deserialize)]
struct RawDecision {
    action: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    merged_content: Option<String>,
    #[serde(default)]
    redundant_ids: Vec<String>,
}

/// Parse a decider response into a [`ConsolidationOp`]. Robust by design: any
/// malformed / non-JSON / unexpected response degrades to [`ConsolidationOp::Noop`]
/// (never an error and never a destructive op), and `redundant_ids` are filtered
/// to the candidate + neighbour id set so a hallucinated id can do no harm.
fn parse_decision(
    raw: &str,
    candidate: &MemoryEntry,
    neighbors: &[MemoryEntry],
) -> ConsolidationOp {
    let Some(json) = extract_json_object(raw) else {
        return ConsolidationOp::Noop;
    };
    let Ok(decision) = serde_json::from_str::<RawDecision>(json) else {
        return ConsolidationOp::Noop;
    };
    if decision.action != "merge" {
        return ConsolidationOp::Noop;
    }
    let merged_content = decision.merged_content.unwrap_or_default();
    if merged_content.trim().is_empty() {
        return ConsolidationOp::Noop;
    }
    let allowed: HashSet<&str> = std::iter::once(candidate.id.as_str())
        .chain(neighbors.iter().map(|n| n.id.as_str()))
        .collect();
    let mut supersede_ids: Vec<String> = decision
        .redundant_ids
        .into_iter()
        .filter(|id| allowed.contains(id.as_str()))
        .collect();
    if !supersede_ids.iter().any(|id| id == &candidate.id) {
        supersede_ids.push(candidate.id.clone());
    }
    supersede_ids.sort();
    supersede_ids.dedup();
    if supersede_ids.len() < 2 {
        // Nothing real to collapse.
        return ConsolidationOp::Noop;
    }
    let key = decision
        .key
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| candidate.key.clone());
    ConsolidationOp::Merge {
        key,
        merged_content,
        supersede_ids,
    }
}

/// Extract the first balanced `{...}` JSON object from a possibly-fenced or
/// prose-wrapped response. Returns `None` if no plausible object is present.
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, ch) in raw[start..].char_indices() {
        match ch {
            '"' if !escaped => in_str = !in_str,
            '\\' if in_str => {
                escaped = !escaped;
                continue;
            }
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=start + i]);
                }
            }
            _ => {}
        }
        escaped = false;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::sqlite::SqliteMemory;
    use crate::memory::traits::MemoryCategory;
    use tempfile::TempDir;

    fn entry(id: &str, key: &str, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.into(),
            key: key.into(),
            content: content.into(),
            category: MemoryCategory::Core,
            timestamp: "2026-06-21T00:00:00Z".into(),
            session_id: None,
            score: None,
            valid_from: None,
            valid_to: None,
            supersedes_id: None,
        }
    }

    /// Merge the candidate with all its neighbours.
    struct MergeAllMock;
    #[async_trait]
    impl ConsolidationDecider for MergeAllMock {
        async fn decide(&self, c: &MemoryEntry, ns: &[MemoryEntry]) -> Result<ConsolidationOp> {
            if ns.is_empty() {
                return Ok(ConsolidationOp::Noop);
            }
            let mut ids = vec![c.id.clone()];
            ids.extend(ns.iter().map(|n| n.id.clone()));
            Ok(ConsolidationOp::Merge {
                key: c.key.clone(),
                merged_content: format!("merged: {}", c.content),
                supersede_ids: ids,
            })
        }
    }

    struct NoopMock;
    #[async_trait]
    impl ConsolidationDecider for NoopMock {
        async fn decide(&self, _c: &MemoryEntry, _ns: &[MemoryEntry]) -> Result<ConsolidationOp> {
            Ok(ConsolidationOp::Noop)
        }
    }

    async fn seed_two_duplicates() -> (TempDir, SqliteMemory) {
        let tmp = TempDir::new().unwrap();
        let mem = SqliteMemory::new(tmp.path()).unwrap();
        mem.store(
            "pref_a",
            "user enjoys green tea",
            MemoryCategory::Core,
            None,
        )
        .await
        .unwrap();
        mem.store(
            "pref_b",
            "user enjoys green tea",
            MemoryCategory::Core,
            None,
        )
        .await
        .unwrap();
        (tmp, mem)
    }

    #[tokio::test]
    async fn apply_merge_collapses_duplicates_to_one_live_row() {
        let (_tmp, mem) = seed_two_duplicates().await;
        assert_eq!(mem.count().await.unwrap(), 2);

        let opts = ConsolidationOptions {
            apply: true,
            ..Default::default()
        };
        let report = run_consolidation(&mem, &MergeAllMock, &opts).await.unwrap();

        assert_eq!(report.merges_applied, 1);
        assert_eq!(
            mem.count().await.unwrap(),
            1,
            "duplicates collapse to a single live canonical row"
        );
        // The surviving live row is the merged canonical (not an original).
        let live = mem.recall("green tea", 5, None).await.unwrap();
        assert_eq!(live.len(), 1, "exactly one live row remains");
        assert!(
            live[0].content.starts_with("merged:"),
            "the live row is the merged canonical, got {:?}",
            live[0].content
        );
    }

    #[tokio::test]
    async fn dry_run_reports_but_does_not_mutate() {
        let (_tmp, mem) = seed_two_duplicates().await;
        let opts = ConsolidationOptions::default(); // apply = false
        let report = run_consolidation(&mem, &MergeAllMock, &opts).await.unwrap();

        assert_eq!(report.merges_planned, 1, "merge is planned");
        assert_eq!(report.merges_applied, 0, "but not applied in dry-run");
        assert_eq!(mem.count().await.unwrap(), 2, "memory untouched");
    }

    #[tokio::test]
    async fn max_merges_zero_caps_application() {
        let (_tmp, mem) = seed_two_duplicates().await;
        let opts = ConsolidationOptions {
            apply: true,
            max_merges: 0,
            ..Default::default()
        };
        let report = run_consolidation(&mem, &MergeAllMock, &opts).await.unwrap();
        assert_eq!(report.merges_planned, 1);
        assert_eq!(report.merges_applied, 0, "cap blocks application");
        assert_eq!(mem.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn noop_decider_changes_nothing() {
        let (_tmp, mem) = seed_two_duplicates().await;
        let opts = ConsolidationOptions {
            apply: true,
            ..Default::default()
        };
        let report = run_consolidation(&mem, &NoopMock, &opts).await.unwrap();
        assert_eq!(report.merges_planned, 0);
        assert_eq!(mem.count().await.unwrap(), 2);
    }

    // ── parse_decision robustness ────────────────────────────────────────

    #[test]
    fn parse_plain_merge_json() {
        let c = entry("c1", "k", "x");
        let ns = [entry("n1", "k2", "y")];
        let raw = r#"{"action":"merge","key":"k","merged_content":"x","redundant_ids":["n1"]}"#;
        match parse_decision(raw, &c, &ns) {
            ConsolidationOp::Merge { supersede_ids, .. } => {
                assert_eq!(supersede_ids, vec!["c1".to_string(), "n1".to_string()]);
            }
            other => panic!("expected merge, got {other:?}"),
        }
    }

    #[test]
    fn parse_fenced_json_and_prose() {
        let c = entry("c1", "k", "x");
        let ns = [entry("n1", "k2", "y")];
        let raw = "Sure!\n```json\n{\"action\":\"merge\",\"merged_content\":\"x\",\"redundant_ids\":[\"n1\"]}\n```";
        assert!(matches!(
            parse_decision(raw, &c, &ns),
            ConsolidationOp::Merge { .. }
        ));
    }

    #[test]
    fn parse_garbage_degrades_to_noop() {
        let c = entry("c1", "k", "x");
        let ns = [entry("n1", "k2", "y")];
        assert_eq!(
            parse_decision("not json at all", &c, &ns),
            ConsolidationOp::Noop
        );
        assert_eq!(parse_decision("", &c, &ns), ConsolidationOp::Noop);
        assert_eq!(
            parse_decision(r#"{"action":"noop"}"#, &c, &ns),
            ConsolidationOp::Noop
        );
    }

    #[test]
    fn parse_filters_hallucinated_ids_and_needs_two() {
        let c = entry("c1", "k", "x");
        let ns = [entry("n1", "k2", "y")];
        // Only a bogus id → after filtering just the candidate remains → <2 → Noop.
        let raw = r#"{"action":"merge","merged_content":"x","redundant_ids":["ghost"]}"#;
        assert_eq!(parse_decision(raw, &c, &ns), ConsolidationOp::Noop);
    }

    #[test]
    fn parse_empty_merged_content_is_noop() {
        let c = entry("c1", "k", "x");
        let ns = [entry("n1", "k2", "y")];
        let raw = r#"{"action":"merge","merged_content":"   ","redundant_ids":["n1"]}"#;
        assert_eq!(parse_decision(raw, &c, &ns), ConsolidationOp::Noop);
    }
}
