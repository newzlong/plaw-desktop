//! Typed errors raised by the agent tool-call loop.
//!
//! Extracted from `loop_.rs` so the error vocabulary is auditable in one
//! file. Both types travel up through `anyhow::Error::chain()` and can be
//! detected via the `is_*` predicates below — callers should prefer those
//! over substring-matching `to_string()`.

#[derive(Debug)]
pub(crate) struct ToolLoopCancelled;

impl std::fmt::Display for ToolLoopCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tool loop cancelled")
    }
}

impl std::error::Error for ToolLoopCancelled {}

/// Returned when the agent loop exits because it ran `max_tool_iterations`
/// turns without producing a final assistant message. Replaces the previous
/// stringly-typed `anyhow!("Agent exceeded maximum tool iterations …")` so
/// callers can match on the type rather than substring-grep the chain.
///
/// The Display message is kept identical to the legacy formatted string so
/// downstream telemetry / log queries that match on the human text continue
/// to work.
#[derive(Debug)]
pub(crate) struct ToolIterationLimit {
    pub limit: usize,
}

impl std::fmt::Display for ToolIterationLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Agent exceeded maximum tool iterations ({})", self.limit)
    }
}

impl std::error::Error for ToolIterationLimit {}

pub(crate) fn is_tool_loop_cancelled(err: &anyhow::Error) -> bool {
    err.chain().any(|source| source.is::<ToolLoopCancelled>())
}

pub(crate) fn is_tool_iteration_limit_error(err: &anyhow::Error) -> bool {
    err.chain().any(|source| source.is::<ToolIterationLimit>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_chain_detected_via_predicate() {
        let err: anyhow::Error = ToolLoopCancelled.into();
        assert!(is_tool_loop_cancelled(&err));
        assert!(!is_tool_iteration_limit_error(&err));
    }

    #[test]
    fn iteration_limit_chain_detected_via_predicate() {
        let err: anyhow::Error = ToolIterationLimit { limit: 20 }.into();
        assert!(is_tool_iteration_limit_error(&err));
        assert!(!is_tool_loop_cancelled(&err));
    }

    #[test]
    fn iteration_limit_display_preserves_legacy_message_text() {
        // Downstream log/telemetry queries match on this exact phrase;
        // changing it would silently break alerting on iteration overruns.
        let err = ToolIterationLimit { limit: 42 };
        assert_eq!(
            err.to_string(),
            "Agent exceeded maximum tool iterations (42)"
        );
    }

    #[test]
    fn cancelled_display_is_short_and_stable() {
        assert_eq!(ToolLoopCancelled.to_string(), "tool loop cancelled");
    }

    #[test]
    fn predicates_walk_anyhow_context_chain() {
        // Wrapping in additional context must not hide the typed cause.
        let inner: anyhow::Error = ToolIterationLimit { limit: 5 }.into();
        let wrapped = inner.context("while running channel-message tool loop");
        assert!(is_tool_iteration_limit_error(&wrapped));
    }
}
