# Prompt-Section DAG — Design Proposal

> **Status:** Draft (RFC, not yet implemented)
> **Tracking:** F-7 (Phase 3 architecture work)
> **Last updated:** 2026-05-04
> **Scope:** `plaw/src/agent/prompt.rs`, `plaw/src/agent/prompt_dag.rs` (PR-1 skeleton), `plaw/src/agent/loop_/tool_io.rs` (re-injection), `plaw/src/agent/intent.rs` (L1 wiring)
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

### 5.1 PR-1: introduce `PromptDag` + one node migrated *(landed)*

- New module `agent/prompt_dag.rs`: trait, graph, topo-sort, cycle detector. *(Sibling module rather than `prompt/dag.rs` subdir; converting `prompt.rs` to a directory is structural reorganisation deferred to PR-2 per the "one concern per PR" rule.)*
- Migrate `IdentitySection` via the [`LegacySectionNode`] adapter; the legacy `SystemPromptBuilder` continues to drive production paths unchanged.
- Tests: empty graph, single node, cycle detection, dep ordering, topo determinism, gate semantics (active + inactive), byte-for-byte parity with `SystemPromptBuilder` for an identity-only build.
- *Production wiring deferred*: PR-1 ships the DAG as standalone module + tests proving parity. The agent loop is **not** rewired yet — that flips atomically in PR-2 once all nine sections are nodes, avoiding a transient hybrid path.
- Eval: byte-for-byte parity vs `d5ae203b` (G4) — held under the unit-test gate.

### 5.2 PR-2: migrate the remaining 8 baseline sections

- All 9 sections now nodes.
- `with_defaults()` becomes a `PromptDag::with_defaults()` that declares the dependency edges from §4.3.
- `SystemPromptBuilder` becomes a thin compatibility shim → eventually deleted.
- Eval: byte-for-byte parity.

### 5.3 PR-3: model L4 as `PerToolCalibrationReminder` node *(deferred)*

**Status:** deferred indefinitely after closer review post PR-2B.

The original sketch proposed making `loop_::tool_io::append_calibration_reminder` a DAG node with `applies` gating on `is_iteration_rebuild`. Three concrete obstacles surfaced when planning the implementation:

1. **Placement contract differs.** The L4 reminder is appended to a *tool result* (which the loop emits as a `<tool_result>...` block carried in user-message content). System-prompt nodes are joined into the **system** message at turn-start. Conflating them in one DAG either:
   - bypasses the node's `applies` / `dependencies` machinery and just calls `build` directly (ceremony with no structural gain), or
   - moves the reminder out of the tool-result block into a fresh system-side message between iterations (a behavior change the RFC §1 explicitly disallowed: "not a rewrite of `SystemPromptBuilder`'s output format").

2. **Context type mismatch.** `PromptNode::applies` and `build` take `&PromptContext<'_>`, which carries fields (workspace_dir, tools, skills, identity_config, dispatcher_instructions) the L4 reminder doesn't read. Building a dummy `PromptContext` at the reminder call site to call `build` is awkward; parameterising the trait over context type loses the ergonomic single-trait shape PR-1 chose.

3. **No second concrete reminder yet.** The `add_node` benefit (one place to register iteration-time reminders) only earns its keep when there's a second reminder. Today there's exactly one (the T-2 calibration check). Per `plaw/CLAUDE.md` §3.2 (YAGNI) and §3.3 (rule-of-three), do not extract until repetition justifies the abstraction.

**Decision.** L4 stays as the imperative `append_calibration_reminder` helper in `loop_/tool_io.rs`. The DAG model proved its worth on system-prompt sections (PR-1 / PR-2A / PR-2B); per-iteration text injection is a different abstraction whose shape will become clearer once L2 (freshness metadata) and L3 (grounding verifier) — both also iteration-time concerns — are actually written. PR-5 may revisit a unified iteration-time abstraction at that point with three concrete users to design against.

**Implication for the RFC.** The `NodeId::PerToolCalibrationReminder` variant in `agent/prompt_dag.rs` keeps its reservation (cheap to keep, signals intent) but is not wired. The §4.4 row for it stays as a documented future possibility, not a planned PR.

### 5.4 PR-4: model L1 scaffold as `IntentScaffold` node *(deferred)*

**Status:** deferred indefinitely after closer review post PR-2B.

The original sketch proposed adding a one-line intent hint to the system prompt at the `IntentScaffold` node position so the LLM would see a consistent picture between the system-level rule context and the user-message-level scaffold body. Closer inspection while planning the implementation surfaced the same class of obstacles that deferred PR-3:

1. **System prompt is built once and cached.** [`Agent::turn`](../src/agent/agent.rs) builds the system prompt only when `self.history.is_empty()` and stores the result at `history[0]` for the rest of the conversation. To make an intent hint refresh per turn, the system prompt would have to be rebuilt and `history[0]` swapped out every turn — a behavior change that breaks LLM-side prompt caching (Anthropic / OpenRouter cache the system+early-history), distorts cost accounting, and complicates streaming-trace observability that assumes the system prompt is invariant within a conversation.

2. **Provider compatibility for mid-conversation system messages.** The alternative — injecting a fresh system message between iterations rather than rebuilding `history[0]` — is rejected by several providers that expect at most one leading system message. Some accept mid-conversation `system` role messages, some silently downgrade them to `user`, and some bail. The current single-cached-system-prompt invariant sidesteps this entirely.

3. **The per-turn scaffold already works where it lives.** [`apply_intent_scaffold`](../src/agent/intent.rs) prepends the full scaffold body to the user message every turn at agent.rs:567. The LLM sees the intent context in the right place for the right turn. Adding a *second* (and necessarily stale, per #1) copy in the system prompt is redundant signal at best and conflicting signal at worst when the intent shifts mid-conversation.

4. **YAGNI: no concrete failure mode is asking for this.** The Phase 2 eval cases that drove the L1 design (math-003, ambiguity-001, etc.) are addressed by the existing user-message-prepend wiring. There is no eval gap that a system-prompt-side hint would close.

**Decision.** L1 stays as the user-message-prepend mechanism in `apply_intent_scaffold`. The DAG model proved its worth on static system-prompt composition (PR-1 / PR-2A / PR-2B); per-turn dynamic content placement remains a separate concern that the user-message scaffold path handles well today.

**Implication for the RFC.** The `NodeId::IntentScaffold` variant in `agent/prompt_dag.rs` keeps its reservation (cheap to keep, signals intent for some future per-turn-rebuild architecture) but is not wired. The §4.3 dependency row for it stays as a documented future possibility, not a planned PR. Combined with PR-3's deferral, the two iteration-time concerns (L1 and L4) will jointly inform a unified per-turn-injection abstraction whenever PR-5's L2 (freshness) and L3 (grounding) are written — three concrete users will then exist to design against.

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

The decision criteria below were the ones the *original* draft used to gate adoption. As of 2026-05-23 the proposal has been **partially adopted with two iteration-time deferrals**: PR-1 (skeleton), PR-2A (9 nodes), and PR-2B (production wiring through `SystemPromptBuilder` shim) have shipped, with byte-parity gates held under eight realistic-fixture tests. PR-3 (L4 reminder) and PR-4 (L1 scaffold) are deferred per §5.3 and §5.4 — both hit the same wall: per-turn dynamic content doesn't fit the once-per-conversation system-prompt model that the DAG layer composes. PR-5 remains optional and gated on the criteria below:

- a concrete blocker for L2 / L3 lands that the current ordered-vector / DAG-of-static-sections model can't express, or
- a second collision case in the same shape as math-003 ↔ ambiguity-001 surfaces from a future eval pass that L1 + the existing system-prompt rules can't separate, or
- a contributor accidentally reorders `with_defaults()` and the result merges, breaking an invariant.

The shipped portions (PR-1, PR-2A, PR-2B) earned their keep by replacing the implicit ordering invariants of the legacy vec-iteration with a typed dependency graph + cycle detection + parity-gate test suite. Further migration is YAGNI-gated.
