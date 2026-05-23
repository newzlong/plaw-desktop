//! Trace context propagation across async hand-off points.
//!
//! Minimum-viable distributed-tracing primitive — OpenTelemetry-shaped
//! ([`trace_id`], [`span_id`], [`parent_span_id`]) but local-only. Propagated
//! implicitly via [`tokio::task_local`] so [`runtime_trace::record_event`]
//! and any other emitter can stamp the current context onto every JSONL
//! entry without changing existing call signatures.
//!
//! # Usage
//!
//! Create a root context at the start of a logical trace (cron fire, WS
//! turn, sub-agent spawn) and wrap the work in a scope:
//!
//! ```ignore
//! use crate::observability::trace_context::{TraceContext, CURRENT_TRACE};
//!
//! let root = TraceContext::root();
//! CURRENT_TRACE
//!     .scope(Some(root), async move { do_work().await })
//!     .await;
//! ```
//!
//! Inside a scope, child spans inherit the [`trace_id`] and chain
//! [`parent_span_id`] to the current span:
//!
//! ```ignore
//! let child = TraceContext::child_of_current()
//!     .unwrap_or_else(TraceContext::root);
//! CURRENT_TRACE
//!     .scope(Some(child), async move { sub_work().await })
//!     .await;
//! ```
//!
//! Reading the current context never panics; outside any scope it returns
//! [`None`] and emitters simply skip the trace fields.
//!
//! # Why no `tracing::Span` integration yet
//!
//! `tracing` crate spans are richer (level/target/attributes) but the
//! plaw runtime trace JSONL is the primary consumer today. Once a future
//! observer wants to feed OpenTelemetry exporters, mirroring these fields
//! onto a `tracing::Span` is a one-call upgrade — kept out of scope here
//! per YAGNI ([`CLAUDE.md`] §3.2).

use uuid::Uuid;

/// Trace context attached to a logical span of work. Cheap to clone
/// (three short owned strings).
#[derive(Debug, Clone)]
pub struct TraceContext {
    /// Root identifier shared by every span in this trace.
    pub trace_id: String,
    /// Identifier of the current span.
    pub span_id: String,
    /// Identifier of the parent span, if any.
    pub parent_span_id: Option<String>,
}

impl TraceContext {
    /// Create a new root context. Both `trace_id` and `span_id` are fresh
    /// UUIDs; `parent_span_id` is `None`.
    pub fn root() -> Self {
        let id = Uuid::new_v4().to_string();
        Self {
            trace_id: id.clone(),
            span_id: id,
            parent_span_id: None,
        }
    }

    /// Derive a child span from this context. Inherits [`trace_id`], gets
    /// a fresh [`span_id`], and points [`parent_span_id`] at the parent's
    /// `span_id`.
    pub fn child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: Uuid::new_v4().to_string(),
            parent_span_id: Some(self.span_id.clone()),
        }
    }

    /// Read the ambient trace context, if a scope is active on this task.
    /// Returns `None` outside any [`CURRENT_TRACE`] scope.
    pub fn current() -> Option<Self> {
        CURRENT_TRACE.try_with(Clone::clone).ok().flatten()
    }

    /// Derive a child span from the current ambient context. Returns
    /// `None` if no scope is active — callers can fall back to
    /// [`TraceContext::root`] when they want to start a new trace
    /// instead of continuing one.
    pub fn child_of_current() -> Option<Self> {
        Self::current().as_ref().map(Self::child)
    }
}

tokio::task_local! {
    /// Ambient trace context for the current async task. `None` outside
    /// any scope; emitters treat absence as "untracked event".
    pub static CURRENT_TRACE: Option<TraceContext>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_has_distinct_uuids_each_call() {
        let a = TraceContext::root();
        let b = TraceContext::root();
        assert_ne!(a.trace_id, b.trace_id);
        assert_ne!(a.span_id, b.span_id);
        assert_eq!(a.trace_id, a.span_id, "root span_id == trace_id");
        assert!(a.parent_span_id.is_none());
    }

    #[test]
    fn child_inherits_trace_id_and_chains_parent() {
        let parent = TraceContext::root();
        let child = parent.child();
        assert_eq!(child.trace_id, parent.trace_id);
        assert_ne!(child.span_id, parent.span_id);
        assert_eq!(child.parent_span_id.as_deref(), Some(parent.span_id.as_str()));
    }

    #[test]
    fn grandchild_chains_through_parent_span() {
        let root = TraceContext::root();
        let child = root.child();
        let grandchild = child.child();

        assert_eq!(grandchild.trace_id, root.trace_id);
        assert_eq!(
            grandchild.parent_span_id.as_deref(),
            Some(child.span_id.as_str())
        );
        // grandchild's parent is child, not root
        assert_ne!(grandchild.parent_span_id.as_deref(), Some(root.span_id.as_str()));
    }

    #[tokio::test]
    async fn current_returns_none_outside_scope() {
        assert!(TraceContext::current().is_none());
    }

    #[tokio::test]
    async fn current_returns_active_context_inside_scope() {
        let ctx = TraceContext::root();
        let expected_trace = ctx.trace_id.clone();

        CURRENT_TRACE
            .scope(Some(ctx), async move {
                let seen = TraceContext::current().expect("inside scope");
                assert_eq!(seen.trace_id, expected_trace);
            })
            .await;
    }

    #[tokio::test]
    async fn child_of_current_returns_none_outside_scope() {
        assert!(TraceContext::child_of_current().is_none());
    }

    #[tokio::test]
    async fn child_of_current_chains_to_active_span() {
        let root = TraceContext::root();
        let expected_parent = root.span_id.clone();
        let expected_trace = root.trace_id.clone();

        CURRENT_TRACE
            .scope(Some(root), async move {
                let child = TraceContext::child_of_current().expect("inside scope");
                assert_eq!(child.trace_id, expected_trace);
                assert_eq!(child.parent_span_id, Some(expected_parent));
            })
            .await;
    }

    #[tokio::test]
    async fn nested_scopes_isolate_per_task() {
        let outer = TraceContext::root();
        let outer_trace = outer.trace_id.clone();

        CURRENT_TRACE
            .scope(Some(outer.clone()), async move {
                let inner = outer.child();
                let inner_trace = inner.trace_id.clone();
                let inner_span = inner.span_id.clone();

                CURRENT_TRACE
                    .scope(Some(inner), async move {
                        let seen = TraceContext::current().expect("inner scope");
                        assert_eq!(seen.trace_id, inner_trace);
                        assert_eq!(seen.span_id, inner_span);
                    })
                    .await;

                // After inner scope exits, outer is visible again on this task.
                let seen = TraceContext::current().expect("outer scope still active");
                assert_eq!(seen.trace_id, outer_trace);
            })
            .await;
    }
}
