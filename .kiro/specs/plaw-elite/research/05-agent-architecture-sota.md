# Agent Architecture SOTA — April 2026

Scope: design inputs for plaw's Rust agent runtime. Focuses on what production systems actually ship, not paper fads.

## 1. Agent Loop Patterns

**Verdict: ReAct + structured tool-call loop won.** Every shipping system (Claude Code, Cursor, Devin, Aider, Codex CLI, Deep Agents) is a `while(tool_calls)` loop over a single flat message history. Claude Code's master loop ("nO") is single-threaded with one flat history; ~98% deterministic harness, ~1.6% AI logic.

- **Plan-and-Execute / ReWOO / LLMCompiler**: useful when token cost dominates and task is a known DAG. LLMCompiler: 3.6x speedup via parallel DAG. Fragile when tools surprise — degrades to ReAct via replanner.
- **Reflexion (Shinn 2023)**: verbal self-reflection across attempts. Narrow — retry-bounded eval-driven tasks only.
- **Tree/Graph-of-Thoughts**: research-grade fad. No major shipping agent runs ToT/GoT in inner loop. Beaten by stronger reasoning models.
- **ADaPT**: recursive task decomposition on failure. Survives via Task-tool patterns.
- **Toolformer**: superseded by native function calling (Claude 4.x, GPT-5, Kimi K2.5).

**Plaw:** keep single-threaded ReAct. Add `write_todos`-style planner tool emitting TODO state, not a separate phase.

Refs: [Claude Code agent loop](https://platform.claude.com/docs/en/agent-sdk/agent-loop) · [Master loop "nO" analysis](https://blog.promptlayer.com/claude-code-behind-the-scenes-of-the-master-agent-loop/) · [LLMCompiler paper](https://arxiv.org/abs/2312.04511)

## 2. Multi-Agent Orchestration

**The June 2025 split:** Cognition's "Don't Build Multi-Agents" vs Anthropic's "How we built our multi-agent research system" — one day apart, opposite conclusions. 2026 consensus:

- **Multi-agent wins for parallelizable read-only work** (research, search fan-out): Anthropic Sonnet-4 multi-agent beat single-agent Opus-4 by 90.2% on internal research evals. Cost ~15x baseline chat.
- **Multi-agent loses for write/coordination** (coding): subagents lack shared design context → "Flappy Bird" conflicts. Devin runs single-threaded.
- **Working pattern: orchestrator + read-only workers**. Lead decomposes, workers fan out, orchestrator synthesizes. Workers never mutate shared state.
- **CrewAI / AutoGen GroupChat / LangGraph supervisor**: pattern > framework. Hierarchical > peer-to-peer in production.

**Plaw:** default single-agent. Subagent dispatch only via `Task`-style tool returning synthesized text.

Refs: [Anthropic multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system) · [Cognition "Don't Build Multi-Agents"](https://cognition.ai/blog/dont-build-multi-agents) · [news.smol.ai split coverage](https://news.smol.ai/issues/25-06-13-cognition-vs-anthropic)

## 3. Sub-Agent Isolation

Convergence on **fresh-context dispatch** as the only safe isolation primitive:

- **Claude Code Task tool**: subagent gets system prompt + task, no parent history. Returns single text result. Tool noise stays inside. `isolated:true` flag proposed (issue #20304) for unbiased reviews.
- **Deep Agents (LangChain, Mar 2026)**: same pattern + virtual FS (in-memory, disk, Modal/Daytona) via mount points. Async subagents return task IDs for background runs.
- **Git worktree isolation (Gastown)**: each agent in own worktree, shares `.git`. Caveats: shared ports/DBs need per-worktree offsets. Refinery merges via Bors-style queue.

**Plaw:** start with context isolation via `Task` tool. Optional worktree mode for code-edit agents only.

Refs: [Claude Code subagents](https://code.claude.com/docs/en/sub-agents) · [Deep Agents repo](https://github.com/langchain-ai/deepagents) · [Gastown](https://github.com/gastownhall/gastown)

## 4. Planning

- **LLM-as-planner with TODO-tool (state-of-practice)**: Claude Code TodoWrite, Deep Agents `write_todos`, Devin plan view. Plan is markdown in state; model rewrites each step. Cheap, robust, observable.
- **HTN + LLM hybrid (ChatHTN, ICAPS 2025)**: symbolic HTN with LLM fallback. Cuts LLM calls ~75%. Worth it for hand-authored domains (robotics, ops). Overkill for plaw.
- **Replan-on-failure**: every shipping system replans implicitly via ReAct. Explicit replanners add latency without robustness gains.

**Plaw:** ship `todo_write` tool. Skip HTN.

Refs: [ChatHTN PMLR 2025](https://proceedings.mlr.press/v288/munoz-avila25a.html) · [Deep Agents planning tool](https://blog.langchain.com/deep-agents/)

## 5. Reflection & Self-Correction

- **Reflexion (Shinn 2023)**: canonical retry-with-memory. Useful only with multiple attempts + verifier signal.
- **LLM-as-Judge / Agent-as-Judge (2026 consensus)**: spawn fresh-context judge subagent with rubric. Mitigate position/verbosity/self-preference/authority biases.
- **Constitutional self-critique**: now baked into model training. Don't reimplement at app level.

**Plaw:** invoke `judge` subagent at task-completion checkpoints for high-stakes actions (commits, deletes, API writes).

Refs: [Reflexion paper](https://arxiv.org/abs/2303.11366) · [Agent-as-a-Judge survey](https://arxiv.org/html/2508.02994v1) · [Anthropic CAI](https://www.anthropic.com/research/constitutional-ai-harmlessness-from-ai-feedback)

## 6. Tool Use

- **Native function calling beats structured-output prompting.** Every frontier model has native parallel tool calls.
- **Parallel tool calls**: independent reads in same turn. 4× 300ms = 300ms wall-clock. Default-on in Anthropic/OpenAI; Kimi K2.5 is Anthropic-compatible.
- **Tool selection at scale**: 100+ tools breaks naive prompting. Use vector-search MCP registry, namespacing, lazy descriptions. ToolWeave (2026) does dynamic composition.
- **MCP errors as instructions**: return `isError: true` text content with actionable hints, NOT protocol errors. Model recovers from semantic errors; raw stack traces cascade.
- **Tool composition**: Claude Code keeps tools small + composable. Avoid mega-tools.

**Plaw:** add (a) parallel-call support in WS protocol, (b) structured tool errors with `hint`, (c) optional MCP semantic registry for >50-tool setups.

Refs: [Function calling guide 2026](https://ofox.ai/blog/function-calling-tool-use-complete-guide-2026/) · [Better MCP errors](https://alpic.ai/blog/better-mcp-tool-call-error-responses-ai-recover-gracefully) · [Semantic MCP tool discovery](https://arxiv.org/html/2603.20313)

## 7. Computer Use / GUI Agents

OSWorld-Verified (Apr 2026): Claude Mythos Preview 79.6%, Holo3-122B 78.8%, GPT-5.5 78.7%, Opus 4.7 78.0%, Sonnet 4.5 61.4%.

Two camps:
- **Pure-pixel** (Anthropic, OpenAI CUA/Operator): screenshot → coordinates. Generalizes to any GUI; brittle on small text.
- **Hybrid pixel + a11y/DOM** (Gemini Computer Use): faster on structured apps; fails on canvas/games.

Anthropic's `computer_20251124` ("Computer Use 2.0") adds region-zoom — closes small-text gap.

**Plaw:** plaw has Rust-native browser. Route through Claude's `computer_20251124` for Anthropic models; coordinate-only on Kimi. Defer accessibility-tree extraction.

Refs: [Anthropic computer use tool docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool) · [OSWorld-Verified leaderboard](https://benchlm.ai/benchmarks/osWorldVerified) · [OpenAI CUA](https://openai.com/index/computer-using-agent/)

## 8. Long-Horizon Execution

- **Durable execution (consensus)**: Temporal/DBOS pattern — every step checkpointed, replayable, idempotent. LangGraph PostgresSaver, DBOS, Temporal all ship this.
- **Devin**: cloud sandbox per task, persistent VM, sleep/resume across days.
- **Letta sleep-time compute**: agent reorganizes memory during idle, precomputes "learned context." 5× lower inference compute, 18% accuracy lift. Orthogonal to durable execution.
- **Background subagents** (Deep Agents v0.5): async subagent returns task ID, runs remote.

**Plaw:** desktop single-user — full Temporal overkill. MVP: (a) SQLite per-turn snapshot, (b) resume on restart, (c) idempotent tools (write_file with hash-check). Sleep-time compute = v2 differentiator.

Refs: [DBOS durable execution for agents](https://www.dbos.dev/blog/durable-execution-crashproof-ai-agents) · [Letta sleep-time compute](https://www.letta.com/blog/sleep-time-compute) · [Devin 2025 review](https://cognition.ai/blog/devin-annual-performance-review-2025)

## 9. Error Recovery

Fragile vs robust:
1. **Errors as instructions, not logs.** Map to semantic structs `{code, message, hint, retry_strategy}`. Inject into context.
2. **Bounded retry + tool replacement.** On repeated tool A failure, try tool B. Requires multiple tools per capability.
3. **Circuit breakers** per tool. Error Recovery Rate (ERR) = recovered/failed. Target ERR > 0.7.
4. **Ask-human as a first-class tool.** Claude Code's permission gates are this.
5. **ErrorProbe failure attribution** (multi-agent): backward-trace. Less relevant for single-agent plaw.

**Plaw:** `PlawError` enum with `hint: String`. Add `ask_user` tool for stuck states. Per-tool counters → circuit breaker after N failures.

Refs: [LLM-friendly MCP errors](https://medium.com/@kumaran.isk/llm-friendly-error-handling-designing-mcp-servers-for-ai-df427f6dfd2f) · [ReliabilityBench](https://arxiv.org/pdf/2601.06112) · [ErrorProbe](https://arxiv.org/html/2604.17658v1)

## 10. Evaluation In The Loop

- **Multi-dim quality gates**: PROMOTE / HOLD / ROLLBACK across task-success, latency, safety, evidence coverage (arxiv 2603.15676).
- **Agent-as-Judge > LLM-as-Judge** for multi-step work — judge sees trace, not just output.
- **Bias mitigation**: position, verbosity, self-preference, authority. Randomize order; rubric scoring.
- **Spot-check 5-10%** with humans.

**Plaw:** in-loop self-eval is overkill for chat. For cron/autonomous flows, add post-action judge before irreversible writes. Surface verdict in UI as "AI reviewed: pass/concerns/fail."

Refs: [Quality gates paper](https://arxiv.org/abs/2603.15676) · [Agent-as-Judge](https://arxiv.org/html/2508.02994v1) · [Langfuse LLM-as-Judge](https://langfuse.com/docs/evaluation/evaluation-methods/llm-as-a-judge)

---

## Concrete Plaw Runtime Deltas (Rust)

| Area | Change | Effort |
|------|--------|--------|
| Loop | Keep single-threaded ReAct. Add `todo_write` tool. | S |
| Subagents | Add `task` tool: spawn child agent with fresh context, return text. | M |
| Tool errors | Wrap all tool returns in `{ok, content, is_error, hint}`. | S |
| Parallel tools | Honor model's parallel `tool_use` blocks; execute via `tokio::join!`. | M |
| Checkpoint | SQLite per-turn snapshot of message history + tool state. | M |
| Judge | Optional post-action judge subagent for irreversible writes. | M |
| Computer use | Defer; route through model-native computer tool when Anthropic. | L (defer) |
| Sleep-time | v2: idle-time memory compaction → vector store. | L (defer) |
| Worktree | v2: optional git-worktree mode for code-editing flows. | L (defer) |

## Must-Read Shortlist

1. Anthropic — How we built our multi-agent research system: <https://www.anthropic.com/engineering/multi-agent-research-system>
2. Cognition — Don't Build Multi-Agents: <https://cognition.ai/blog/dont-build-multi-agents>
3. Claude Code agent loop docs: <https://platform.claude.com/docs/en/agent-sdk/agent-loop>
4. Dive into Claude Code (VILA-Lab): <https://github.com/VILA-Lab/Dive-into-Claude-Code>
5. LangChain Deep Agents: <https://github.com/langchain-ai/deepagents>
6. Letta sleep-time compute: <https://www.letta.com/blog/sleep-time-compute>
7. DBOS durable execution for agents: <https://www.dbos.dev/blog/durable-execution-crashproof-ai-agents>
8. Reflexion (Shinn et al.): <https://arxiv.org/abs/2303.11366>
9. LLMCompiler: <https://arxiv.org/abs/2312.04511>
10. OSWorld-Verified leaderboard: <https://benchlm.ai/benchmarks/osWorldVerified>
