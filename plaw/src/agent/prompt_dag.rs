//! Prompt-section DAG — F-7 PR-1 skeleton.
//!
//! This is the first implementation slice of the
//! `docs/prompt-section-dag-design.md` proposal. It introduces the
//! [`PromptNode`] trait, the [`PromptDag`] graph holder, and a
//! deterministic topological build with cycle detection — and proves
//! the byte-for-byte parity claim (G4 in the RFC) by wrapping the
//! existing [`IdentitySection`] as a [`LegacySectionNode`] and
//! comparing the output against [`SystemPromptBuilder`] under the
//! same `PromptContext`.
//!
//! **What this PR does NOT do** (intentional scope):
//!
//!   - it does not wire the DAG into [`Agent::turn`] / agent.rs — production
//!     paths still go through `SystemPromptBuilder::with_defaults()`. The
//!     wiring lands in a later PR (RFC §5.2) once all nine baseline
//!     sections are nodes and the parity gate has held across all of
//!     them, not just `IdentitySection`;
//!   - it does not migrate the remaining eight sections (Tools, Safety,
//!     Calibration, Skills, Workspace, Runtime, DateTime, ChannelMedia);
//!   - it does not introduce L1 / L4 nodes — the `NodeId` enum reserves
//!     identifiers for them so future PRs don't have to renumber, but
//!     no node implementations exist yet.
//!
//! Why a sibling module rather than `prompt/dag.rs` as the RFC sketched:
//! the existing `prompt.rs` is a flat single-file module. Converting it
//! to a directory mid-RFC mixes structural reorganisation with the new
//! type introduction, which violates the "one concern per PR" rule in
//! `plaw/CLAUDE.md` §3.4. PR-1 stays minimal; promotion to
//! `agent/prompt/` can happen in PR-2 when the migration of remaining
//! sections actually wants the directory.

use crate::agent::prompt::{IdentitySection, PromptContext, PromptSection};
use anyhow::{bail, Result};
use std::collections::HashSet;

/// Stable identifier for a prompt-graph node. Doubles as the
/// dependency-edge key, so a section that wants to declare
/// `dependencies() = &[NodeId::Safety]` references the safety node by
/// this enum variant — never by string, so a typo is a compile error.
///
/// Variants for unmigrated sections (everything except `Identity`) are
/// reserved here so the [`crate::agent::prompt::PromptSection`]
/// migration in subsequent PRs doesn't have to renumber. The reserved
/// variants are intentionally unused right now; they exist so the
/// `dependencies()` arrays of nodes that will be migrated later can
/// already cite them once we want to test ordering with stubs.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
#[allow(dead_code)] // reserved variants for future PRs (RFC §5.2-5.5)
pub enum NodeId {
    Identity,
    Tools,
    Safety,
    Calibration,
    Skills,
    Workspace,
    Runtime,
    DateTime,
    ChannelMedia,
    /// L1 — appended only when the intent classifier returned a
    /// scaffold-bearing intent (RFC §4.5).
    IntentScaffold,
    /// L4 — emitted only when the DAG is rebuilt at an iteration
    /// boundary after an external tool returned (RFC §4.5).
    PerToolCalibrationReminder,
    /// L2 — placeholder; not implemented (RFC §5.5).
    ToolFreshnessMetadata,
    /// L3 — placeholder; not implemented (RFC §5.5).
    GroundingVerifierTail,
}

/// A node in the prompt-section DAG.
///
/// Implementations declare their identity, their incoming dependency
/// edges, an optional gate that decides whether the node contributes
/// to the current build, and the body the node renders.
///
/// The contract for `build` matches [`PromptSection::build`] verbatim:
/// return an empty / whitespace-only string to skip, or a non-empty
/// section body to be joined with `\n\n` separators by [`PromptDag::build`].
pub trait PromptNode: Send + Sync {
    fn id(&self) -> NodeId;

    /// Node IDs that must appear *before* this one in the topological
    /// order. Declaring a dependency on an inactive node (one whose
    /// `applies` returned `false`) is silently treated as satisfied —
    /// the dependent still runs. RFC §4.4.
    fn dependencies(&self) -> &'static [NodeId] {
        &[]
    }

    /// Decide whether this node contributes to the current build.
    /// Returning `false` excludes the node entirely (and excludes its
    /// `build` from being called). Default: always applicable.
    fn applies(&self, _ctx: &PromptContext<'_>) -> bool {
        true
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String>;
}

/// The prompt-section graph itself.
///
/// Holds an ordered list of nodes (declaration order doubles as the
/// tie-break for topological sorting), provides a deterministic
/// `build` that filters by gate, topo-sorts, joins with `\n\n` to match
/// [`SystemPromptBuilder::build`] byte-for-byte.
#[derive(Default)]
pub struct PromptDag {
    nodes: Vec<Box<dyn PromptNode>>,
}

impl PromptDag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(mut self, node: Box<dyn PromptNode>) -> Self {
        self.nodes.push(node);
        self
    }

    /// Filter to applicable nodes, topo-sort with declaration-order
    /// tie-break, render each, and join with `\n\n`. Errors:
    ///
    ///   - `cycle detected in PromptDag` if the active subgraph
    ///     contains a cycle.
    ///   - any error returned by a node's `build`.
    pub fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let active: Vec<&dyn PromptNode> = self
            .nodes
            .iter()
            .filter(|n| n.applies(ctx))
            .map(|n| n.as_ref())
            .collect();

        let order = topo_sort(&active)?;

        let mut output = String::new();
        for node in order {
            let part = node.build(ctx)?;
            if part.trim().is_empty() {
                continue;
            }
            output.push_str(part.trim_end());
            output.push_str("\n\n");
        }
        Ok(output)
    }
}

/// Kahn-style topological sort with declaration-order tie-break.
///
/// Determinism contract: when multiple nodes have indegree 0
/// simultaneously, the one declared earliest in the input slice runs
/// first. Tested by [`tests::topo_sort_breaks_ties_by_declaration_order`].
///
/// Dependency edges to nodes that aren't in the active set (because
/// their gate returned `false`) are treated as satisfied — i.e. they
/// don't contribute to the dependent's indegree. RFC §4.4.
fn topo_sort<'a>(nodes: &'a [&'a dyn PromptNode]) -> Result<Vec<&'a dyn PromptNode>> {
    let n = nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let active_ids: HashSet<NodeId> = nodes.iter().map(|node| node.id()).collect();

    // Indegree counts only deps on *active* nodes; deps on filtered-out
    // nodes are auto-satisfied.
    let mut indeg: Vec<usize> = nodes
        .iter()
        .map(|node| {
            node.dependencies()
                .iter()
                .filter(|dep| active_ids.contains(dep))
                .count()
        })
        .collect();

    let mut visited = vec![false; n];
    let mut order: Vec<&dyn PromptNode> = Vec::with_capacity(n);

    while order.len() < n {
        // Pick the smallest declaration index with indeg 0 not yet
        // visited. O(N²) but N ≤ 13 in practice (RFC §4.1 enum size).
        let pick = (0..n).find(|&i| !visited[i] && indeg[i] == 0);
        match pick {
            Some(i) => {
                visited[i] = true;
                order.push(nodes[i]);
                let id = nodes[i].id();
                for (j, node) in nodes.iter().enumerate() {
                    if visited[j] {
                        continue;
                    }
                    if node.dependencies().iter().any(|d| *d == id) {
                        indeg[j] = indeg[j].saturating_sub(1);
                    }
                }
            }
            None => bail!("cycle detected in PromptDag"),
        }
    }

    Ok(order)
}

/// Adapter wrapping any [`PromptSection`] as a [`PromptNode`] with
/// declared identity + dependencies. Lets the existing nine sections
/// migrate one at a time without rewriting their bodies — PR-1 uses it
/// for `IdentitySection` only; PR-2 will wrap the rest.
pub struct LegacySectionNode<S: PromptSection + 'static> {
    pub id: NodeId,
    pub deps: &'static [NodeId],
    pub section: S,
}

impl<S: PromptSection + 'static> PromptNode for LegacySectionNode<S> {
    fn id(&self) -> NodeId {
        self.id
    }

    fn dependencies(&self) -> &'static [NodeId] {
        self.deps
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        self.section.build(ctx)
    }
}

/// Construct an `Identity`-only DAG using the legacy adapter. Convenience
/// for tests and for the parity-gate call site that PR-2 will introduce.
pub fn identity_only_dag() -> PromptDag {
    PromptDag::new().add_node(Box::new(LegacySectionNode {
        id: NodeId::Identity,
        deps: &[],
        section: IdentitySection,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::prompt::SystemPromptBuilder;
    use crate::config::SkillsPromptInjectionMode;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Minimal node helper for graph-shape tests that don't need a real
    /// section body. Records the build calls for ordering assertions.
    struct Probe {
        id: NodeId,
        deps: &'static [NodeId],
        body: &'static str,
        applies_flag: bool,
    }

    impl PromptNode for Probe {
        fn id(&self) -> NodeId {
            self.id
        }
        fn dependencies(&self) -> &'static [NodeId] {
            self.deps
        }
        fn applies(&self, _: &PromptContext<'_>) -> bool {
            self.applies_flag
        }
        fn build(&self, _: &PromptContext<'_>) -> Result<String> {
            Ok(self.body.to_string())
        }
    }

    fn fixture_ctx(workspace: &std::path::Path) -> PromptContext<'_> {
        PromptContext {
            workspace_dir: workspace,
            model_name: "test-model",
            tools: &[],
            skills: &[],
            skills_prompt_mode: SkillsPromptInjectionMode::Compact,
            identity_config: None,
            dispatcher_instructions: "",
        }
    }

    // ── Empty / single-node sanity ────────────────────────────────

    #[test]
    fn empty_dag_builds_to_empty_string() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let out = PromptDag::new().build(&ctx).unwrap();
        assert!(out.is_empty(), "empty DAG must produce empty output, got {out:?}");
    }

    #[test]
    fn single_zero_dep_node_renders_with_trailing_separator() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let out = PromptDag::new()
            .add_node(Box::new(Probe {
                id: NodeId::Identity,
                deps: &[],
                body: "alpha",
                applies_flag: true,
            }))
            .build(&ctx)
            .unwrap();
        // Match SystemPromptBuilder shape: section body trimmed of
        // trailing newlines, separated by "\n\n", trailing "\n\n".
        assert_eq!(out, "alpha\n\n");
    }

    // ── Topological correctness ───────────────────────────────────

    #[test]
    fn dependent_node_runs_after_its_dependency() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        // Declare B *before* A in vec order, but B depends on A. Topo
        // sort must place A first regardless of declaration order.
        let out = PromptDag::new()
            .add_node(Box::new(Probe {
                id: NodeId::Tools,
                deps: &[NodeId::Identity],
                body: "B",
                applies_flag: true,
            }))
            .add_node(Box::new(Probe {
                id: NodeId::Identity,
                deps: &[],
                body: "A",
                applies_flag: true,
            }))
            .build(&ctx)
            .unwrap();
        assert_eq!(out, "A\n\nB\n\n", "A must precede B; got {out:?}");
    }

    #[test]
    fn topo_sort_breaks_ties_by_declaration_order() {
        // Two zero-dep nodes: A declared first, B declared second.
        // Output must be `A then B`, not `B then A`. This is the
        // determinism guarantee (G4: byte parity with the legacy
        // builder, which iterates the vec in insertion order).
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let out = PromptDag::new()
            .add_node(Box::new(Probe {
                id: NodeId::Identity,
                deps: &[],
                body: "first",
                applies_flag: true,
            }))
            .add_node(Box::new(Probe {
                id: NodeId::Workspace,
                deps: &[],
                body: "second",
                applies_flag: true,
            }))
            .build(&ctx)
            .unwrap();
        assert_eq!(out, "first\n\nsecond\n\n");
    }

    // ── Cycle detection ───────────────────────────────────────────

    #[test]
    fn cycle_returns_error_with_pinned_message() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let err = PromptDag::new()
            .add_node(Box::new(Probe {
                id: NodeId::Identity,
                deps: &[NodeId::Tools],
                body: "A",
                applies_flag: true,
            }))
            .add_node(Box::new(Probe {
                id: NodeId::Tools,
                deps: &[NodeId::Identity],
                body: "B",
                applies_flag: true,
            }))
            .build(&ctx)
            .unwrap_err();
        assert!(
            err.to_string().contains("cycle detected"),
            "expected cycle-detection error, got: {err}"
        );
    }

    // ── Gate semantics ────────────────────────────────────────────

    #[test]
    fn inactive_node_is_skipped_and_does_not_block_dependents() {
        // B depends on A; A's `applies` is false. B must still build
        // (RFC §4.4: deps on inactive nodes are auto-satisfied).
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let out = PromptDag::new()
            .add_node(Box::new(Probe {
                id: NodeId::Identity,
                deps: &[],
                body: "A",
                applies_flag: false, // gated out
            }))
            .add_node(Box::new(Probe {
                id: NodeId::Tools,
                deps: &[NodeId::Identity],
                body: "B",
                applies_flag: true,
            }))
            .build(&ctx)
            .unwrap();
        assert_eq!(out, "B\n\n", "B must run despite A being inactive");
    }

    #[test]
    fn applies_gate_is_called_with_context() {
        // Pin that `applies` actually receives the live ctx — a gate
        // that toggles based on a ctx field must work. Uses an
        // Arc<AtomicBool> for the side-channel because PromptNode is
        // `Send + Sync` (rules out `Cell`) and `lib.rs` forbids unsafe
        // (rules out raw-pointer peeking).
        struct CtxGated {
            saw_workspace: Arc<AtomicBool>,
        }
        impl PromptNode for CtxGated {
            fn id(&self) -> NodeId {
                NodeId::Workspace
            }
            fn applies(&self, ctx: &PromptContext<'_>) -> bool {
                self.saw_workspace
                    .store(ctx.workspace_dir.exists(), Ordering::SeqCst);
                true
            }
            fn build(&self, _: &PromptContext<'_>) -> Result<String> {
                Ok(String::new())
            }
        }

        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let saw = Arc::new(AtomicBool::new(false));
        let _ = PromptDag::new()
            .add_node(Box::new(CtxGated {
                saw_workspace: saw.clone(),
            }))
            .build(&ctx)
            .unwrap();
        assert!(
            saw.load(Ordering::SeqCst),
            "applies must have been called with the live PromptContext"
        );
    }

    // ── Byte-for-byte parity gate (G4) ────────────────────────────

    #[test]
    fn dag_with_identity_node_matches_legacy_builder_byte_for_byte() {
        // The whole point of PR-1: prove the DAG path produces the
        // *same bytes* as the legacy SystemPromptBuilder when the
        // same single section is wired. This is the regression net
        // for the entire RFC migration plan — if this fails, do not
        // proceed with PR-2.
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());

        let legacy = SystemPromptBuilder::default()
            .add_section(Box::new(IdentitySection))
            .build(&ctx)
            .unwrap();

        let dag_out = identity_only_dag().build(&ctx).unwrap();

        assert_eq!(
            legacy, dag_out,
            "DAG output must equal SystemPromptBuilder output byte-for-byte"
        );
    }

    // ── Migration dependency-shape pin ────────────────────────────

    #[test]
    fn identity_node_declares_zero_dependencies() {
        // RFC §4.3 declares Identity has no incoming edges. Pin this
        // here so a future PR that accidentally adds a dep on, say,
        // Tools (which would create a cycle once Tools depends on
        // Identity) gets caught at unit-test time.
        let node = LegacySectionNode {
            id: NodeId::Identity,
            deps: &[],
            section: IdentitySection,
        };
        assert_eq!(node.dependencies().len(), 0);
        assert_eq!(node.id(), NodeId::Identity);
    }
}
