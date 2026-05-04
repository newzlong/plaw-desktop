# Prompt-Section DAG — Design Proposal

> **Status:** Draft (RFC, not yet implemented)
> **Tracking:** F-7 (Phase 3 architecture work)
> **Last updated:** 2026-05-04
> **Scope:** `plaw/src/agent/prompt.rs`, `plaw/src/agent/loop_/tool_io.rs` (re-injection), `plaw/src/agent/intent.rs` (L1 wiring)
> **Non-scope:** runtime configuration of node graphs, user-editable prompts, prompt-injection content controls (handled by `security::prompt_guard`)

This document is a design proposal. It does **not** describe shipped behavior. The current implementation uses an ordered `Vec<Box<dyn PromptSection>>` (`SystemPromptBuilder::with_defaults()`) — see "Background" below.

---

## 1. Background — what's there today

The system prompt that plaw sends to the LLM at the start of every turn is composed by [`SystemPromptBuilder::build()`](../src/agent/prompt.rs) from a fixed-order vector of nine sections:

```text
1. IdentitySection      — workspace identity files (AGENTS.md / SOUL.md / TOOLS.md / …)
2. ToolsSection         — tool registry: name, description, JSON schema, dispatcher hints
3. SafetySection        — 11 hardcoded rules (no exfiltration, injection defense, etc.)
4. CalibrationSection   — honesty / precision / ambiguity / conflict / borderline rules
5. SkillsSection        — XML-formatted skills (Full / Compact / Silent mode)
6. WorkspaceSection     — current working directory
7. RuntimeSection       — hostname, OS, model name
8. DateTimeSection      — current local time + timezone (Phase-2 T-1 grounding)
9. ChannelMediaSection  — voice / image / document marker conventions
```

Each section implements [`PromptSection::build(&self, ctx) -> Result<String>`](../src/agent/prompt.rs); empty results are skipped, non-empty results are joined with `\n\n`. The order is hardcoded inside `with_defaults()` — there is no compile-time or run-time check that a reorder preserves the ordering invariants documented in the codebase.

Adjacent to the builder, two newer concerns mutate the prompt surface **outside** of `SystemPromptBuilder`:

- **L1 (intent router)** — `agent::intent::HybridRouter` classifies the user message into one of seven intents and prepends an intent-specific scaffold to the *user message*, not the system prompt. Wired in [`Agent::turn`](../src/agent/agent.rs) and the channel path; gated by `config.intent_routing_enabled`.
- **L4 (per-tool re-injection)** — `loop_::tool_io::append_calibration_reminder` appends a ~100-token "verify before answering" tail to external-tool output every iteration, fighting recency-window dilution of the system-prompt-level calibration rule.

Neither L1 nor L4 is modeled as a section. They live in different files, have different gates, and there is no place that lists "all things that contribute to what the LLM sees in turn N."

## 2. Problem statement

Three concrete failure modes have surfaced from Phase 2 eval work (`../../.kiro/specs/plaw-elite/phase-1-eval/phase-2-targets.md`) that the current shape can't address cleanly:

### 2.1 Prompt saturation (math-003 ↔ ambiguity-001 collision)

`CalibrationSection` accumulated rules for both "user gave a wrong premise → correct it first" (T-3) and "missing context → ask one clarifying question" (T-6). When math-003 asks "已知 5+5=11, 求 5+5+1" and ambiguity-001 asks "总统的身高?", the surface sentence shapes are too similar for prompt-level rules to separate. Strengthening one rule weakens the other:

| prompt variant | math-003 score | ambiguity-001 score |
|---|---|---|
| v1 (T-3 wins, "correct premise first") | 4.0/5 | 2.5/5 (defaults to US president) |
| v2 (T-6 wins, MUST ask clarifying)     | 2.0/5 (asks "in which number system?") | 3.5/5 |
| v3 (precedence: wrong-premise wins)    | 4.0/5 | 2.0/5 (back to defaulting) |

T-10 in `phase-2-targets.md` records this collision as **prompt-only unfixable**. The structural fix is to classify the intent *before* the prompt is rendered and select the rule set per-intent, not pile both into one section. **L1 already exists**; the gap is that nothing in the prompt-construction surface knows L1 ran, so L1's classification cannot prune sections.

### 2.2 Recency-window dilution

After ~6 tool iterations, the system prompt's calibration rule loses attention weight against newer tool output. T-2 (`numerical-cal-001`) drops from 80% confabulation rate to 40% with **L4 already shipped** as `append_calibration_reminder`, but the helper lives in `tool_io.rs` and is invoked imperatively from the agent loop, not declared as part of the prompt graph. Adding a sibling reminder (e.g. for ambiguity persistence over long chains) means editing the loop, not adding a node.

### 2.3 Implicit ordering invariants

The current invariants documented in code:

- Identity must come first (workspace files define baseline before behavior rules).
- Safety must precede Calibration (constraints before refinements).
- Skills must come after Safety+Calibration (skills mustn't override core rules).
- DateTime is injected at turn-start only (must not regenerate mid-loop with stale time).

None of these are checked. A future contributor reordering `with_defaults()` to put `SkillsSection` above `SafetySection` would silently let a malicious skill description override the core safety rule set. The invariants are PR-time review concerns, not compile-time errors.

## 3. Goals & non-goals

### Goals (G)

- **G1.** Make ordering invariants declarative and enforced (cycle-free; "X before Y" expressed as data, not custom prose in code review).
- **G2.** Allow per-iteration prompt mutation (L4) and per-classification mutation (L1) to be expressed *in the same model* as the static sections.
- **G3.** Allow conditional inclusion (e.g. `SkillsSection` only when skills are loaded; intent-scaffold only when L1 router classifies non-default) without ad-hoc null returns.
- **G4.** Preserve byte-for-byte output for the default config until intent-routed paths actually fire — i.e. landing the DAG must not change baseline eval (`d5ae203b`).
- **G5.** Make L2 (freshness metadata) and L3 (grounding verifier) — the two layers that have **not** shipped — trivially expressible as new nodes once they're built. This is the test that the abstraction earned its keep.

### Non-goals (N)

- **N1.** No runtime / config-driven graph editing. The graph is code; only the gates are data. Otherwise prompt injection becomes an attack on the graph itself.
- **N2.** No general-purpose dependency-injection framework. The DAG is for prompt sections specifically — not a vehicle to reorganize the rest of the agent loop.
- **N3.** No "prompt section marketplace" / registry pattern. Each node is a Rust struct with explicit deps; adding one is a code change.
- **N4.** Not a rewrite of `SystemPromptBuilder`'s output format. The final-string contract (sections joined with `\n\n`, no trailing newline) is preserved.

## 4. Proposal — DAG model

### 4.1 Node identity & trait

```rust
// agent/prompt/dag.rs (new)
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
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
    // L1 — only emitted when classification ran:
    IntentScaffold,
    // L4 — only emitted in iteration-boundary build:
    PerToolCalibrationReminder,
    // Reserved for L2/L3 once landed:
    ToolFreshnessMetadata,
    GroundingVerifierTail,
}

pub trait PromptNode: Send + Sync {
    /// Stable identifier; doubles as the dependency key.
    fn id(&self) -> NodeId;

    /// IDs that must appear *before* this node in topo order.
    /// Empty slice = no constraint.
    fn dependencies(&self) -> &'static [NodeId];

    /// Decide whether this node contributes to the current build.
    /// Lets the DAG skip Skills when no skills are loaded, intent
    /// scaffold when classification didn't fire, etc.
    fn applies(&self, ctx: &PromptContext<'_>) -> bool { true }

    /// Render the section body. Empty string = skip (parity with
    /// the legacy PromptSection contract).
    fn build(&self, ctx: &PromptContext<'_>) -> anyhow::Result<String>;
}
```

`PromptSection` (current trait) becomes a thin newtype wrapper that maps `NodeId::*` to the existing struct, so migration is incremental.

### 4.2 Graph + topological build

```rust
pub struct PromptDag {
    nodes: Vec<Box<dyn PromptNode>>,
}

impl PromptDag {
    pub fn with_defaults() -> Self;             // declares the 9 baseline nodes + L4
    pub fn add_node(self, node: Box<dyn PromptNode>) -> Self;

    /// Cycle-detected, topo-sorted build. Returns the joined system prompt.
    pub fn build(&self, ctx: &PromptContext<'_>) -> anyhow::Result<String>;
}
```

`build()`:

1. Filter `self.nodes` to those where `applies(ctx) == true`.
2. Run Kahn's algorithm over `(node.id(), node.dependencies())`. Cycle = panic in dev, `bail!` in prod.
3. For each node in topo order, call `build(ctx)`; collect non-empty strings.
4. Join with `\n\n`, return.

The topo-sort is **deterministic** under tie-breaking by `NodeId` declaration order (matches the current `with_defaults()` order); preserving G4.

### 4.3 Declared dependencies (initial graph)

```text
Identity         → []
Tools            → [Identity]                    # tool list refers to workspace tools
Safety           → [Tools]                       # "don't run shell.exec on …" depends on knowing Tools
Calibration      → [Safety]                      # refines, doesn't contradict, safety
Skills           → [Tools, Safety, Calibration]  # skills must not override
Workspace        → []                            # informational, no deps
Runtime          → []                            # informational, no deps
DateTime         → []                            # turn-start only; gate handles staleness
ChannelMedia     → []                            # informational, no deps
IntentScaffold   → [Calibration]                 # scaffold refines calibration when classifier fired
PerToolCalibrationReminder
                 → [Calibration]                 # mid-iteration refresh of calibration
```

The `IntentScaffold` dep on `Calibration` is the structural fix for §2.1: when the classifier returns `Intent::WrongPremise`, the scaffold is appended **after** `CalibrationSection` and can override it locally without bleeding into the `Intent::Ambiguous` path (which gets a different scaffold body).

### 4.4 Gates (`applies`)

| Node | `applies(ctx)` returns true when |
|---|---|
| Identity | always |
| Tools | `!ctx.tools.is_empty()` |
| Safety | always |
| Calibration | always |
| Skills | `!ctx.skills.is_empty() && ctx.skills_prompt_mode != Silent` |
| Workspace | `ctx.workspace_dir.is_some()` |
| Runtime | always |
| DateTime | `!ctx.is_iteration_rebuild` *(see §4.5)* |
| ChannelMedia | `ctx.channel.is_some()` |
| IntentScaffold | `ctx.intent.map(|i| i.has_scaffold()).unwrap_or(false)` |
| PerToolCalibrationReminder | `ctx.is_iteration_rebuild && ctx.last_tool_was_external` |

### 4.5 Two build modes — turn-start vs iteration

`PromptContext` grows one field:

```rust
pub struct PromptContext<'a> {
    // … existing fields …
    /// True when the DAG is being rebuilt mid-loop (after a tool turn)
    /// rather than at the start of a fresh user turn. Lets nodes that
    /// would carry stale state (DateTime) opt out, and lets nodes that
    /// only matter mid-loop (PerToolCalibrationReminder) opt in.
    pub is_iteration_rebuild: bool,
}
```

This unifies the L4 reminder with the rest of the prompt surface. Currently `append_calibration_reminder` is bolted onto tool output; under the DAG model it's a node that contributes to the prompt rebuild between iterations, governed by the same gate machinery as the rest.

(L1's intent scaffold remains attached to the *user message* in the agent loop, not the system prompt. The DAG knows about it via the `IntentScaffold` node so dependencies on it are expressible, but the join point with the LLM message vector is unchanged. This preserves G4 — adding the DAG without rewiring intent routing must not move bytes around.)

## 5. Migration plan

Five PRs, each independently revertable.

### 5.1 PR-1: introduce `PromptDag` + one node migrated

- New module `agent/prompt/dag.rs`: trait, graph, topo-sort, cycle detector.
- Migrate `IdentitySection` (zero-dep, simplest).
- `SystemPromptBuilder::build()` calls `PromptDag::build_only(NodeId::Identity)` for the migrated slot, falls back to legacy for others.
- Tests: empty graph, single node, cycle detection, dep ordering, topo determinism.
- Eval: byte-for-byte parity vs `d5ae203b` (G4).

### 5.2 PR-2: migrate the remaining 8 baseline sections

- All 9 sections now nodes.
- `with_defaults()` becomes a `PromptDag::with_defaults()` that declares the dependency edges from §4.3.
- `SystemPromptBuilder` becomes a thin compatibility shim → eventually deleted.
- Eval: byte-for-byte parity.

### 5.3 PR-3: model L4 as `PerToolCalibrationReminder` node

- `append_calibration_reminder` deleted from `tool_io.rs`.
- New `PerToolCalibrationReminder` node added to default DAG with the gate from §4.4.
- The agent loop's iteration boundary triggers `dag.build(ctx_with_iteration_rebuild = true)` and threads the result back to the LLM as a system-side reminder.
- Eval: T-2 score must hold (≤ ±1 noise floor) — this is a refactor, not a behavior change.

### 5.4 PR-4: model L1 scaffold as `IntentScaffold` node

- Existing `apply_intent_scaffold` in `agent::intent` keeps producing the user-message-prepended scaffold.
- New `IntentScaffold` node *also* adds a one-line "(intent: WrongPremise)" hint to the system prompt at the position §4.3 declares, so the LLM sees a consistent picture.
- Gate: `applies` returns true only when classification fired AND the chosen intent has a scaffold body.
- Eval: T-3 / T-6 / T-10 scores tracked; intent-routing-enabled cases compared against intent-routing-disabled.

### 5.5 PR-5: add `ToolFreshnessMetadata` and `GroundingVerifierTail` nodes (L2 + L3)

- These nodes ship the implementations of L2 and L3 (which haven't existed before).
- Their tests are the test that the DAG abstraction earned its keep (G5): adding the two new nodes should not require any change to the DAG itself, only `add_node` calls in `with_defaults()`.

## 6. Test strategy

Five categories of tests in `agent/prompt/dag.rs`:

1. **Topo-sort correctness.** Constructed graphs with N nodes assert `build_order(graph)` produces a valid topo order; declared deps come before dependents.
2. **Cycle detection.** A graph with `A → B → A` returns an error from `build()`; tests pin the error type and message.
3. **Determinism under tie-break.** Two zero-dep nodes appear in the same order across 100 builds.
4. **Gate semantics.** A node whose `applies(ctx) == false` doesn't appear in the output, and its dependents are not blocked from running (they treat the absent node as satisfied).
5. **Output parity.** A `PromptDag::with_defaults()` build with the legacy fixture context matches `SystemPromptBuilder::build()` byte-for-byte. This is the regression net for G4.

A sixth category — eval parity — runs at PR time, not unit-test time: the relevant `crates/plaw-eval` config (`cases/grounded/` baseline) re-runs against `d5ae203b` and the PR.

## 7. Open questions

- **Q1.** Should `PromptContext` carry the *result* of L1 classification, or only the user message and the gate? Current intent-routing wiring has the classifier owned by `Agent::turn`, not the prompt builder. The cleanest option is `ctx.intent: Option<Intent>` — the classifier remains in `Agent::turn`; the DAG node consumes the result via gate + body lookup. Documenting this here so PR-4 has a default.
- **Q2.** Where do third-party / plugin-contributed nodes go? Out of scope for now (N3); revisit when peripherals or skills want to inject their own prompt fragments. Current peripherals expose `tools()`, not prompt sections; skills are already a section.
- **Q3.** `is_iteration_rebuild` is a bool today; it could become an enum if L2/L3 want different rebuild contexts (`AfterTool { tool: &str, freshness: ... }`). PR-3 will tell us if the bool is sufficient or if the type wants to grow.

## 8. Risks & mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Topo-sort changes byte order vs `with_defaults()` for some context | medium | high (eval drift) | PR-1 / PR-2 ship with byte-parity tests; deterministic tie-break by `NodeId` declaration order matches current order |
| Adding a node introduces a hidden cycle | low | medium (build error in prod) | Cycle check in `build()`; CI runs `cargo test` on the DAG module |
| `PromptContext` field bloat | medium | low | One field per PR, documented gate; revisit if `>5` new fields accumulate |
| L4 mid-loop rebuild changes timing of `append_calibration_reminder` | medium | low | PR-3 keeps the same insertion point relative to tool output; only the implementation moves |
| Over-abstraction for 9 sections | low | medium | YAGNI gate: PRs land only when the new node payoff is concrete. PR-5 (L2/L3) is the proof-of-keep |

## 9. Rollback plan

Each PR is a standalone refactor with its own eval-parity gate; revert is `git revert <pr_sha>` and a CI run. PRs intentionally do not chain: PR-3 doesn't depend on PR-2 having shipped to production (only on PR-2 having merged) — if the DAG is reverted, L4 falls back to the legacy `append_calibration_reminder` path automatically because the function isn't deleted until PR-3 itself.

## 10. Decision criteria for proceeding

This proposal is **not adopted** unless and until:

- a concrete blocker for L1 / L2 / L3 / L4 lands that the current ordered-vector model can't express, or
- a second collision case in the same shape as math-003 ↔ ambiguity-001 surfaces from a future eval pass, or
- a contributor accidentally reorders `with_defaults()` and the result merges, breaking an invariant.

Until one of those fires, the existing `SystemPromptBuilder` is sufficient. The DAG is a planned response to a class of failure mode, not a speculative refactor.
