//! Non-CLI approval flow plumbing.
//!
//! When the agent loop wants to execute a tool that requires user
//! approval AND the conversation is happening through a channel
//! (Telegram, Discord, Slack, ...), it must:
//!
//!   1. Create a pending approval entry in `ApprovalManager`.
//!   2. Send a prompt to the user through the channel
//!      (via `prompt_tx` ferried in `NonCliApprovalContext`).
//!   3. Poll until the user replies, the request is cancelled, or the
//!      wait timeout expires — then return the resolved
//!      `ApprovalResponse`.
//!
//! Step (3) is the meaty part. It lives in
//! [`await_non_cli_approval_decision`] below. The two struct types
//! (the prompt-payload and the runtime context that ferries the
//! sender/reply_target/prompt_tx triple into the loop) are
//! co-located so the whole non-CLI approval contract is editable in
//! one file.

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::approval::{ApprovalManager, ApprovalResponse};

use super::budgets::{NON_CLI_APPROVAL_POLL_INTERVAL_MS, NON_CLI_APPROVAL_WAIT_TIMEOUT_SECS};

/// Payload sent on `NonCliApprovalContext::prompt_tx` when the loop
/// needs the user to approve a tool call. The channel layer renders
/// this into a chat message asking "approve / deny / always" and
/// records the user's reply via `ApprovalManager`.
#[derive(Debug, Clone)]
pub(crate) struct NonCliApprovalPrompt {
    pub request_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

/// Per-turn runtime context the agent loop needs to drive a non-CLI
/// approval flow. `prompt_tx` is unbounded because individual
/// approval prompts are tiny and the channel layer drains them on a
/// dedicated task; backpressure here would only show up as missed
/// prompts.
#[derive(Debug, Clone)]
pub(crate) struct NonCliApprovalContext {
    pub sender: String,
    pub reply_target: String,
    pub prompt_tx: tokio::sync::mpsc::UnboundedSender<NonCliApprovalPrompt>,
}

/// Block until the user resolves a non-CLI approval request.
///
/// Returns:
///   - the user's reply if it lands inside [`NON_CLI_APPROVAL_WAIT_TIMEOUT_SECS`]
///   - `ApprovalResponse::No` if the request disappears (the channel
///     layer evicted it for some reason) — fail-closed
///   - `ApprovalResponse::No` if `cancellation_token` fires (a newer
///     message preempted this turn)
///   - `ApprovalResponse::No` after the timeout elapses (the user
///     went silent; we reject the pending request and clean up the
///     resolution slot so a late reply doesn't poison the next turn)
///
/// The polling cadence is [`NON_CLI_APPROVAL_POLL_INTERVAL_MS`]; both
/// constants live in `loop_/budgets.rs` so they're tunable in one place.
pub(super) async fn await_non_cli_approval_decision(
    mgr: &ApprovalManager,
    request_id: &str,
    sender: &str,
    channel_name: &str,
    reply_target: &str,
    cancellation_token: Option<&CancellationToken>,
) -> ApprovalResponse {
    let started = Instant::now();

    loop {
        if let Some(decision) = mgr.take_non_cli_pending_resolution(request_id) {
            return decision;
        }

        if !mgr.has_non_cli_pending_request(request_id) {
            // Fail closed when the request disappears without an explicit resolution.
            return ApprovalResponse::No;
        }

        if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
            // Clean up the pending entry (and any racing resolution) so a late
            // reply can't poison a future turn — mirrors the timeout path below.
            let _ =
                mgr.reject_non_cli_pending_request(request_id, sender, channel_name, reply_target);
            let _ = mgr.take_non_cli_pending_resolution(request_id);
            return ApprovalResponse::No;
        }

        if started.elapsed() >= Duration::from_secs(NON_CLI_APPROVAL_WAIT_TIMEOUT_SECS) {
            let _ =
                mgr.reject_non_cli_pending_request(request_id, sender, channel_name, reply_target);
            let _ = mgr.take_non_cli_pending_resolution(request_id);
            return ApprovalResponse::No;
        }

        tokio::time::sleep(Duration::from_millis(NON_CLI_APPROVAL_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutonomyConfig;

    #[tokio::test]
    async fn await_returns_resolution_when_user_replies() {
        // Pre-resolve the request before calling await — the function
        // should return the resolution immediately on its first poll
        // without sleeping for the full poll interval.
        let mgr = ApprovalManager::from_config(&AutonomyConfig::default());
        let pending = mgr.create_non_cli_pending_request(
            "shell",
            "alice",
            "telegram",
            "chat-1",
            None,
        );
        mgr.record_non_cli_pending_resolution(&pending.request_id, ApprovalResponse::Yes);

        let started = Instant::now();
        let decision =
            await_non_cli_approval_decision(&mgr, &pending.request_id, "alice", "telegram", "chat-1", None)
                .await;
        assert_eq!(decision, ApprovalResponse::Yes);
        // Sanity: pre-resolved requests must not wait a full poll cycle.
        assert!(
            started.elapsed() < Duration::from_millis(NON_CLI_APPROVAL_POLL_INTERVAL_MS),
            "expected immediate return, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn await_fails_closed_when_pending_request_disappears() {
        // No pending request was ever created → the await loop should
        // hit the fail-closed branch on its first iteration.
        let mgr = ApprovalManager::from_config(&AutonomyConfig::default());
        let decision = await_non_cli_approval_decision(
            &mgr,
            "nonexistent-request",
            "alice",
            "telegram",
            "chat-1",
            None,
        )
        .await;
        assert_eq!(
            decision,
            ApprovalResponse::No,
            "missing request must fail closed"
        );
    }

    #[tokio::test]
    async fn await_returns_no_when_cancellation_token_fires() {
        let mgr = ApprovalManager::from_config(&AutonomyConfig::default());
        let pending = mgr.create_non_cli_pending_request(
            "shell",
            "alice",
            "telegram",
            "chat-1",
            None,
        );
        let token = CancellationToken::new();
        token.cancel();

        let decision = await_non_cli_approval_decision(
            &mgr,
            &pending.request_id,
            "alice",
            "telegram",
            "chat-1",
            Some(&token),
        )
        .await;
        assert_eq!(decision, ApprovalResponse::No);
    }
}
