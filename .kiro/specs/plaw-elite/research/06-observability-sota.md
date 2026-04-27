# Plaw Elite — LLM/Agent Observability SOTA (April 2026)

Scope: design plaw's observability layer (Tauri desktop, Rust core, Vue frontend, Kimi K2.5). Local-first; user owns data.

---

## 1. Observability Platforms

**Leader: Langfuse** (acquired by ClickHouse Jan 2026, $15B Series D, 19/50 Fortune 50). MIT-licensed core, OTel-ingest first-class. Stack: PostgreSQL + ClickHouse + Redis + S3. Heavy for desktop; viable as optional self-host endpoint.

**Arize Phoenix** — Apache 2.0, single-Docker option, OTel-native. Best for embedded/lightweight self-host. Strong eval primitives (RAG triad, hallucination evals built in). Closest to "just works locally."

**Helicone** — proxy gateway model (URL swap), excellent for cost/cache view; weak for deep agent trace hierarchies. Skip for plaw — we control the runtime.

Skip for plaw: LangSmith (LangChain-coupled, SaaS-only), Braintrust (eval-CI focused, SaaS), Datadog LLM Obs (enterprise-priced).

**Recommendation**: emit OTel GenAI; ship optional Phoenix-in-a-Docker for "power user mode"; default = local SQLite-backed trace store.

- https://github.com/langfuse/langfuse
- https://github.com/Arize-ai/phoenix
- https://www.firecrawl.dev/blog/best-llm-observability-tools

## 2. OpenTelemetry GenAI Conventions

Status April 2026: GenAI semconv still **experimental** but de-facto standard. Datadog, Langfuse, Phoenix, Honeycomb, Uptrace all consume it natively. `OTEL_SEMCONV_STABILITY_OPT_IN` enables dual-emission during transitions.

Span kinds defined: `gen_ai.client` (LLM call), `gen_ai.agent` (agent step), `gen_ai.tool` (tool exec), `gen_ai.embedding`, `gen_ai.vector_store`. Attributes: `gen_ai.system`, `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, `gen_ai.usage.cache_read_input_tokens`, `gen_ai.response.finish_reasons`. Events for prompts/completions are separated to keep span attributes small.

**OpenLLMetry (Traceloop)** drives the SIG; SDK has Python/JS auto-instrumentation. **No Rust auto-instrumentation exists** — manual spans required. The `tracing-opentelemetry` + `opentelemetry-otlp` stack is solid; pair with `tracing` macros.

Tradeoff: experimental status means attribute renames. Pin a version; gate emission behind a `OTEL_SEMCONV_VERSION` flag.

- https://opentelemetry.io/docs/specs/semconv/gen-ai/
- https://github.com/traceloop/openllmetry
- https://github.com/open-telemetry/opentelemetry-rust

## 3. Tracing Patterns for Agents

**Span hierarchy (canonical)**: session-root → conversation-turn → agent-step → (llm-call | tool-call | retrieval) → sub-agent-step (recursive). Tool inputs/outputs go on span events not attributes (size limits). For Plaw's loop: one root span per user message, child span per agent iteration, child spans per tool execution, llm chunk metrics aggregated at iteration close.

**Context propagation in async Rust**: use `tracing::Instrument::instrument(span)` on every spawned task. W3C tracecontext headers if Plaw ever calls remote sub-agents. For multi-agent (parallel branches), use span links not parent-child to avoid fanout-collapse.

**Key practice**: cap tool I/O at e.g. 8KB per event with elision markers; store full payload keyed by trace+span ID in separate blob store. Otherwise traces become unreadable.

- https://www.braintrust.dev/articles/agent-observability-tracing-tool-calls-memory
- https://uptrace.dev/blog/opentelemetry-ai-systems

## 4. Cost & Token Tracking

**Healthy cache hit rate target**: 70%+ for stable agentic loops; <40% indicates prompt drift or tool-schema instability. Track `cache_read`, `cache_write`, `input`, `output`, `reasoning` as separate counters — naive `total_tokens × list_price` over-reports 35–50% on cache-heavy workloads.

**Attribution dimensions** (tag at span creation, never retroactively): `user_id`, `session_id`, `feature` (chat/skill/cron), `model`, `agent_step_kind`. Plaw is single-user desktop, but `feature` and `skill_name` matter for the user's own budget view.

**Budget enforcement**: pre-flight estimator on input length + reserved output; hard stop on monthly cap; surface "this skill burned 40% of your budget" in UI.

**Token waste detection**: log when (a) tool result > N tokens but model didn't cite it, (b) reasoning tokens > output tokens by 5x, (c) repeated identical tool calls within a session.

- https://www.digitalapplied.com/blog/llm-agent-cost-attribution-guide-production-2026
- https://langfuse.com/docs/observability/features/token-and-cost-tracking

## 5. Quality Drift Detection

**Three layers**: (1) statistical on tokens/length/latency (KS, PSI, Wasserstein), (2) embedding drift on inputs and outputs via MMD or energy distance over sentence-transformer embeddings vs. reference window, (3) semantic eval (LLM-as-judge on sample).

**Format failure rate** is the cheapest, highest-signal metric for plaw: % responses that fail JSON schema, % tool calls with invalid args, % responses missing expected sections. Spike = upstream model change or prompt regression.

**Recommendation for plaw**: keep a 30-day rolling reference window; daily MMD on input embeddings (using a small local model like BGE-M3 or all-MiniLM); alert in UI when >2σ. Skip heavy methods at desktop scale.

- https://www.evidentlyai.com/blog/embedding-drift-detection
- https://insightfinder.com/blog/hidden-cost-llm-drift-detection/

## 6. Hallucination Detection in Production

**HHEM-2.1-Open (Vectara)** — DeBERTa-class classifier, ~600MB RAM, 1.5s for 2k tokens on CPU. **The only production-viable real-time detector for desktop**. Outputs grounded probability per claim. MIT-friendly license, runs offline.

**Lynx (Llama3-70B fine-tune)** — best accuracy on HaluBench, but 70B is infeasible on desktop. Use only for offline batch eval.

**SelfCheckGPT** — sample N completions, measure consistency. Cost prohibitive for production (Nx tokens per response). Reserve for high-stakes flagged spans.

**FaithJudge** (2026 Vectara leaderboard's new principal detector) — few-shot LLM-as-judge. Best alignment with human labels but adds an extra LLM hop.

**Citation grounding** (RAG): extract claims with a small extractor, verify each against retrieved chunks. 2026 finding: citations alone create "illusion of groundedness" — must verify, not trust.

**Plaw plan**: bundle HHEM-2.1-Open as optional sidecar; run on flagged outputs (low confidence, user thumbs-down, or web_search/web_fetch tool used). Don't run on every response.

- https://huggingface.co/vectara/hallucination_evaluation_model
- https://arxiv.org/abs/2407.08488 (Lynx)
- https://github.com/EdinburghNLP/awesome-hallucination-detection

## 7. User Feedback Loops

**Implicit signals (rich, ~50% coverage)**: copy event, re-roll/regenerate, edit-and-resend, abandon (close before completion), follow-up rephrase (similarity to prior turn > 0.85), session length, time-to-next-turn. Each tied to span ID.

**Explicit (<1% coverage)**: thumbs + optional comment, span-level "this tool got it wrong," pairwise A/B when showing alternatives.

**Annotation queue**: low-score traces auto-flagged → user can review in a "review inbox" → labeled → exported as JSONL eval set or fine-tune corpus. For plaw's local-first model, this is purely user-owned.

- https://langfuse.com/docs/observability/features/user-feedback
- https://openreview.net/forum?id=toSLK7ISiE

## 8. Replay & Debugging

**Deterministic replay** = store seed, recorded LLM responses, recorded tool outputs, then re-execute agent loop reading from recording instead of live calls. Catches timing-sensitive bugs that vanish under logging.

**SOTA refs**:
- **agent-replay** (clay-good/agent-replay) — local SQLite, fork+diff runs, auto eval. Closest model to what plaw needs.
- **LangGraph Time Travel** — checkpoint state at each node, branch from any checkpoint.
- **Replay.io MCP** — browser DOM + network record/replay, exposed to Cursor/Claude Code via MCP.

**Trace diff**: compare two runs of same input (different prompt/model versions) span-by-span, surface tool-call divergence and token delta. Critical for prompt iteration.

**Plaw plan**: SQLite trace DB with full LLM I/O; "replay this conversation with new system prompt" button; trace diff view in dev mode.

- https://github.com/clay-good/agent-replay
- https://www.replay.io/

## 9. Privacy & PII (the local-first thesis)

Plaw runs on-device with `plaw-data/` portable dir — observability **must never phone home by default**. Architecture:

- **Default**: SQLite at `plaw-data/.plaw/observability/traces.db`. OTel exporter is `tracing-subscriber` writing structured JSON to local OTLP collector OR direct to SQLite.
- **Opt-in remote**: user provides their own OTLP endpoint (their Langfuse/Phoenix instance). Never plaw-owned cloud.
- **PII redaction**: pre-export filter. **OpenAI Privacy Filter** (open-source, on-device, 96% F1, 128k context, runs in <1GB) — bundle as optional sidecar. Or use Microsoft Presidio (Rust port via FFI possible). Redact before persistence, not before display.
- **Span attribute hygiene**: never put raw user content in span attributes (queryable, indexed); put it in events with size caps.

**GDPR/right-to-delete**: per-trace TTL; "delete all traces from session X" UI; export-all as JSONL.

- https://thenewstack.io/openai-privacy-filter-pii/
- https://ijcjournal.org/InternationalJournalOfComputer/article/view/2458

## 10. Evals Integrated with Monitoring

**Eval-as-monitoring**: production traces become eval datasets continuously. Galileo, Phoenix, Braintrust all do this. Pattern: scheduled job samples N traces/day, runs eval suite (LLM-judge + deterministic checks: format, schema, citation grounding), writes scores back as span attributes, alerts on regression vs. baseline.

**Guard models / runtime safety**: lightweight classifier on input (prompt-injection, jailbreak) and output (toxicity, PII leakage, off-policy). **ProbGuard** (2025) — modular runtime monitor that intervenes in the agent loop when violation probability exceeds threshold by injecting risk context. **Llama Guard 3** for input/output classification (~8B, too heavy for plaw default; viable as opt-in).

**Eval-to-guardrail lifecycle**: same eval used pre-prod becomes runtime guard. E.g. "tool-arg validity" eval → online guard rejecting invalid tool calls before execution.

**Plaw plan**: ship 5 deterministic guards (JSON schema, tool-arg validity, max-tool-iterations, output-length sanity, allowed-domain check for fetch). LLM-judge evals only on sampled traces, off by default to save tokens.

- https://arxiv.org/abs/2508.00500 (ProbGuard)
- https://www.datadoghq.com/blog/llm-guardrails-best-practices/

---

## Rust Ecosystem Cheat-Sheet

| Need | Crate |
|------|-------|
| Span API | `tracing`, `tracing-subscriber` |
| OTel bridge | `tracing-opentelemetry`, `opentelemetry`, `opentelemetry-otlp`, `opentelemetry-sdk` |
| Local SQLite store | `rusqlite` or `sqlx` (write spans as rows) |
| GenAI semconv | manual — no Rust auto-instr; constants from `opentelemetry-semantic-conventions` |
| Embeddings (drift) | `fastembed-rs` (BGE/MiniLM ONNX) |
| Hallucination | HHEM-2.1 via `ort` (ONNX Runtime) sidecar |
| PII redaction | OpenAI Privacy Filter via `ort` or `llama.cpp` FFI |

`llm-observatory-sdk` exists on crates.io but is small/early — read for ideas, don't depend on it.

## Plaw-Specific Recommendations (concrete)

1. **OTel GenAI from day one**: every LLM call, tool call, retrieval emits a span with `gen_ai.*` attributes.
2. **Local SQLite trace DB** as default sink; OTLP exporter optional and opt-in.
3. **Trace viewer in-app** (Vue): timeline, span tree, tool I/O collapsible cards, replay button.
4. **Five deterministic guards** always-on; LLM-judge evals sampled, opt-in.
5. **HHEM sidecar** as optional power-user feature for hallucination flagging.
6. **PII redaction layer** between span creation and persistence; default-on for any user content.
7. **Implicit feedback** (copy/regen/abandon) auto-captured and tied to span IDs — zero user friction.
8. **Replay from SQLite**: re-run a conversation with prompt/model swapped; diff vs. original.
9. **Per-skill cost view** with hard budget cap; user-owned.
10. **Cache hit rate dashboard** (Anthropic-format `cache_read_input_tokens`) — single most important cost lever.
