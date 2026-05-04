//! Task-local scope wrappers around [`run_tool_call_loop`].
//!
//! Some agent-loop call sites (channel runtimes, web gateway, the
//! non-CLI approval driver) need to thread per-task context into the
//! tool loop without rewriting `run_tool_call_loop`'s 16-argument
//! signature. We use `tokio::task_local!` for this — the scope holds
//! a value that the loop body fishes out via the getter helpers in
//! this module.
//!
//! Two scopes are exposed:
//!
//!   1. **Reply target** — the channel-side ID of who triggered the
//!      turn (Telegram chat ID, Discord channel ID, etc.). Used by
//!      `tool_io::maybe_inject_cron_add_delivery` to auto-fill cron
//!      job delivery routing so scheduled reminders go back to the
//!      conversation that asked for them.
//!
//!   2. **Non-CLI approval context** — sender / reply-target /
//!      prompt-tx triple used by [`super::non_cli_approval`] to fan
//!      approval requests out to a chat channel and wait for the
//!      user's reaction-based response.
//!
//! The two scopes interact: setting the non-CLI approval context
//! also sets the reply target (the approval context carries one),
//! so the second wrapper nests the first. Encoding that nesting in
//! one place — here — means call sites don't have to remember the
//! ordering invariant.
//!
//! **Why isolate this in its own module:** keeping the task_local
//! statics private to a small module limits the surface that can
//! observe / mutate them. The only public-API touchpoints are the
//! two `run_tool_call_loop_with_*` wrapper functions and the two
//! `current_*` getters consumed by the loop body. Lateral access
//! from other parts of the crate is impossible.

use crate::approval::ApprovalManager;
use crate::observability::Observer;
use crate::providers::{ChatMessage, Provider};
use crate::tools::Tool;
use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::non_cli_approval::NonCliApprovalContext;
use super::run_tool_call_loop;

tokio::task_local! {
    /// Channel-side identifier of who triggered the current turn
    /// (e.g. Telegram chat ID). Read by tool-input preprocessing in
    /// `tool_io::maybe_inject_cron_add_delivery` to auto-route cron
    /// output back to the originating conversation.
    static TOOL_LOOP_REPLY_TARGET: Option<String>;
}

tokio::task_local! {
    /// Sender / reply-target / prompt-tx triple used by
    /// [`super::non_cli_approval::await_non_cli_approval_decision`]
    /// to fan approval requests out to a chat channel and wait for
    /// the user's reaction-based response.
    static TOOL_LOOP_NON_CLI_APPROVAL_CONTEXT: Option<NonCliApprovalContext>;
}

/// Read the current task's reply-target if a scope has been set.
/// Returns `None` outside any scope (e.g. CLI / direct invocations).
pub(super) fn current_reply_target() -> Option<String> {
    TOOL_LOOP_REPLY_TARGET
        .try_with(Clone::clone)
        .ok()
        .flatten()
}

/// Read the current task's non-CLI approval context if set. Returns
/// `None` when running outside a non-CLI scope (CLI agents that
/// approve via stdin, or test runners that pre-approve in config).
pub(super) fn current_non_cli_approval_context() -> Option<NonCliApprovalContext> {
    TOOL_LOOP_NON_CLI_APPROVAL_CONTEXT
        .try_with(Clone::clone)
        .ok()
        .flatten()
}

/// Run the tool loop with optional non-CLI approval context scoped
/// to this task. The reply_target scope is automatically derived
/// from the approval context's `reply_target` field so chat-channel
/// auto-routing keeps working without the caller having to set both.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_tool_call_loop_with_non_cli_approval_context(
    provider: &dyn Provider,
    history: &mut Vec<ChatMessage>,
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    provider_name: &str,
    model: &str,
    temperature: f64,
    silent: bool,
    approval: Option<&ApprovalManager>,
    channel_name: &str,
    non_cli_approval_context: Option<NonCliApprovalContext>,
    multimodal_config: &crate::config::MultimodalConfig,
    max_tool_iterations: usize,
    cancellation_token: Option<CancellationToken>,
    on_delta: Option<tokio::sync::mpsc::Sender<String>>,
    hooks: Option<&crate::hooks::HookRunner>,
    excluded_tools: &[String],
) -> Result<String> {
    let reply_target = non_cli_approval_context
        .as_ref()
        .map(|ctx| ctx.reply_target.clone());

    TOOL_LOOP_NON_CLI_APPROVAL_CONTEXT
        .scope(
            non_cli_approval_context,
            TOOL_LOOP_REPLY_TARGET.scope(
                reply_target,
                run_tool_call_loop(
                    provider,
                    history,
                    tools_registry,
                    observer,
                    provider_name,
                    model,
                    temperature,
                    silent,
                    approval,
                    channel_name,
                    multimodal_config,
                    max_tool_iterations,
                    cancellation_token,
                    on_delta,
                    hooks,
                    excluded_tools,
                ),
            ),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Getter behaviour outside any scope ───────────────────────

    #[tokio::test]
    async fn current_reply_target_returns_none_outside_scope() {
        // No `.scope(...)` wrapper around this future — the getter
        // must report None, not panic. The CLI path (where no
        // task-local scope is set) depends on this contract.
        assert_eq!(current_reply_target(), None);
    }

    #[tokio::test]
    async fn current_non_cli_approval_context_returns_none_outside_scope() {
        assert!(current_non_cli_approval_context().is_none());
    }

    // ── Scope propagation ────────────────────────────────────────

    #[tokio::test]
    async fn reply_target_scope_is_visible_inside_inner_future() {
        // The wrapper sets a scope; the getter called inside that
        // scope must observe the value.
        let result = TOOL_LOOP_REPLY_TARGET
            .scope(Some("chat:42".to_string()), async {
                current_reply_target()
            })
            .await;
        assert_eq!(result, Some("chat:42".to_string()));
    }

    #[tokio::test]
    async fn reply_target_scope_does_not_leak_outside() {
        // After the scoped future completes, the outer task must
        // see None again — task_local scopes are dynamically scoped,
        // not module-global.
        let _ = TOOL_LOOP_REPLY_TARGET
            .scope(Some("chat:42".to_string()), async {
                let _ = current_reply_target();
            })
            .await;
        assert_eq!(
            current_reply_target(),
            None,
            "scope must not leak after the inner future completes"
        );
    }

    #[tokio::test]
    async fn nested_scopes_inner_takes_precedence() {
        // Inner scope rebinding: outer says "outer", inner says
        // "inner"; the getter inside the inner future sees "inner".
        // After the inner future returns, the outer scope sees its
        // own value. This is the contract the
        // `run_tool_call_loop_with_non_cli_approval_context` wrapper
        // depends on when it nests REPLY_TARGET inside
        // NON_CLI_APPROVAL_CONTEXT.
        let result = TOOL_LOOP_REPLY_TARGET
            .scope(Some("outer".to_string()), async {
                let inner = TOOL_LOOP_REPLY_TARGET
                    .scope(Some("inner".to_string()), async { current_reply_target() })
                    .await;
                let outer_after = current_reply_target();
                (inner, outer_after)
            })
            .await;
        assert_eq!(result.0, Some("inner".to_string()));
        assert_eq!(result.1, Some("outer".to_string()));
    }
}
