//! Flywheel reviewer — thin facade over the storage layer for the
//! human-in-the-loop review workflow. The CLI binds these directly.

use anyhow::Result;
use chrono::Utc;

use crate::storage::{EvalRepo, FlywheelEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    Reject,
}

/// List entries awaiting review, most-recent-first.
pub fn list_pending(repo: &EvalRepo, limit: usize) -> Result<Vec<FlywheelEntry>> {
    repo.flywheel_list_pending(limit)
}

/// Apply a verdict to a queue entry.
pub fn review(repo: &EvalRepo, id: &str, verdict: ReviewVerdict) -> Result<()> {
    let status = match verdict {
        ReviewVerdict::Approve => "approved",
        ReviewVerdict::Reject => "rejected",
    };
    repo.flywheel_set_status(id, status, Some(Utc::now().timestamp()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FlywheelEntry;

    fn enqueue(repo: &EvalRepo, id: &str, status: &str) {
        repo.flywheel_enqueue(&FlywheelEntry {
            id: id.into(),
            trace_id: format!("trace-{id}"),
            sampled_at: 0,
            judge_score: None,
            review_status: status.into(),
            reviewed_at: None,
            promoted_to_suite: None,
            promoted_case_id: None,
            source_run_id: None,
            source_case_id: None,
            target_suite: None,
        })
        .unwrap();
    }

    #[test]
    fn approve_removes_entry_from_pending_list() {
        let repo = EvalRepo::open_in_memory().unwrap();
        enqueue(&repo, "a", "pending");
        enqueue(&repo, "b", "pending");
        assert_eq!(list_pending(&repo, 10).unwrap().len(), 2);

        review(&repo, "a", ReviewVerdict::Approve).unwrap();
        let pending = list_pending(&repo, 10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "b");

        let approved = repo.flywheel_get("a").unwrap().unwrap();
        assert_eq!(approved.review_status, "approved");
        assert!(approved.reviewed_at.is_some());
    }

    #[test]
    fn reject_marks_entry_rejected() {
        let repo = EvalRepo::open_in_memory().unwrap();
        enqueue(&repo, "a", "pending");
        review(&repo, "a", ReviewVerdict::Reject).unwrap();
        let entry = repo.flywheel_get("a").unwrap().unwrap();
        assert_eq!(entry.review_status, "rejected");
    }
}
