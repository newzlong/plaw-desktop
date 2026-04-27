# Memory SOTA for Self-Evolving Agents (April 2026)

Audience: plaw architects designing a self-consolidating memory layer in a Tauri/Rust desktop. Focus: concrete techniques, named systems, tradeoffs.

---

## 1. Frameworks — production track record vs research foundation

| System | Stance | Core idea | Verdict |
|---|---|---|---|
| **Letta (MemGPT)** | Production, widely deployed | LLM-as-OS: `core` (RAM, always in-context) + `recall` (conversation cache) + `archival` (vector store), agent calls `core_memory_replace`, `archival_memory_search` as tools | Strongest production track record; only OSS framework with measurable multi-week task improvement. Letta v1 (2026) is git-backed, model-agnostic. |
| **Mem0** | Production, easiest adoption | Dual-store: vector (semantic search) + graph (entity relations); LLM extracts facts, dedupes, classifies (ADD/UPDATE/DELETE/NOOP) | Best "drop-in" UX. Weaker on temporal queries (49% LongMemEval vs Zep 63.8%). |
| **Zep / Graphiti** | Production for temporal reasoning | Bi-temporal knowledge graph: every edge has `valid_from/valid_to` (event time) + `created_at/invalidated_at` (ingest time); edge invalidation supersedes facts | SOTA on DMR (94.8%) and LongMemEval temporal slice. Best research foundation for time-aware memory. |
| **A-MEM** (NeurIPS 2025) | Research, OSS | Zettelkasten — each note auto-links to neighbors at write-time; new notes can rewrite tags/context of older notes ("memory evolution") | Distinctive: write-time graph synthesis, not read-time. Best mental model for self-organizing memory. |
| **HippoRAG 2** (ICLR 2025) | Research, OSS | Neocortex (LLM) + parahippocampal encoder + open KG, retrieval via Personalized PageRank from query-anchor entities; non-parametric continual learning | Best multi-hop QA. Heavy: needs OpenIE + PPR per query. |
| **Generative Agents** (Stanford, 2023) | Foundational | Memory stream + nightly reflection + recency·importance·relevance scoring | Conceptual ancestor of every modern system. Read once, reuse the math. |
| **MemoryBank** | Foundational | Ebbinghaus decay `R = e^(-t/S)`, recall reinforces by `S++, t=0` | Use the formula; the rest is dated. |

**Recommendation for plaw**: emulate Letta's tier model + A-MEM's write-time linking + Zep's bi-temporal edges. Skip a true graph DB v1 — encode edges in SQLite.

Refs: [Letta](https://github.com/letta-ai/letta) | [Mem0 paper 2504.19413](https://arxiv.org/html/2504.19413v1) | [Zep 2501.13956](https://arxiv.org/abs/2501.13956) | [A-MEM 2502.12110](https://arxiv.org/abs/2502.12110) | [HippoRAG repo](https://github.com/OSU-NLP-Group/HippoRAG)

---

## 2. Memory taxonomy — layers and protocols

Standardized 5-tier from 2025 surveys (sensory rarely useful for chat agents → 4 tiers in practice):

- **Working** = context window. The only memory the model directly reasons over; everything else is retrieved into it.
- **Episodic** = raw event traces (turn-by-turn, tool calls, outcomes). Append-only, timestamped. Plaw's 记忆胶囊 sit here.
- **Semantic** = de-contextualized facts ("user prefers DD/MM/YYYY"). Distilled from episodic.
- **Procedural** = reusable skills/plans. Voyager's code skills indexed by NL description; Letta's "memory blocks" used as tool prompts.

**Read protocol** (per turn): hydrate working = system + core block + top-k episodic (recency·importance·relevance) + top-k semantic (relevance) + relevant procedural (task-similarity).

**Write protocol** (per turn): append episodic; *deferred* extraction → semantic; *background* reflection → semantic + edits to old semantic; skill verification → procedural. Letta exposes write as tool calls; Mem0/Zep do it via background pipeline.

Refs: [Memory in the Age of AI Agents survey 2512.13564](https://arxiv.org/abs/2512.13564) | [Position: Episodic Memory is the Missing Piece 2502.06975](https://arxiv.org/pdf/2502.06975)

---

## 3. Consolidation — how memory learns from itself

Three techniques converge in 2026:

1. **Sleep-time compute** (Berkeley + Letta, Apr 2025). Pre-compute reasoning over context during idle gaps; reduces test-time compute 2.5×–5× at equal accuracy. Anthropic's **Claude Code Auto-Dream** is the first production deployment: 24h trigger reviews every memory file, prunes stale, resolves contradictions, reorganizes index. Run as a low-priority Tauri tokio task gated on user inactivity.
2. **Reflection** (Generative Agents). When sum of recent importance ≥ threshold, prompt the LLM with last N memories: "What 3 high-level insights?" Insights become semantic memories pointing back to evidence (forms a 2-tier tree). Threshold ≈ 150 importance points / ~2× per simulated day.
3. **Hierarchical summarization** (HiAgent ACL 2025, Memory Gisting 2025). Replace older episodic blocks with gists; keep gist→original pointer for "interactive lookup" if reflection demands raw.

Failure mode: reflections compound errors (hallucinated insight cited as fact later). Mitigate with provenance edges ("derived_from: [mem_ids]") and a confidence decay on derived nodes.

Refs: [Sleep-time Compute 2504.13171](https://arxiv.org/abs/2504.13171) (Letta blog) | [Claude Auto-Dream](https://claudefa.st/blog/guide/mechanics/auto-dream) | [Generative Agents 2304.03442](https://arxiv.org/abs/2304.03442) | [HiAgent ACL 2025](https://aclanthology.org/2025.acl-long.1575.pdf)

---

## 4. Retrieval — scoring formulas

Stanford triple is still the baseline:

```
score = α_rec · exp(-Δt/τ) + α_imp · (importance/10) + α_rel · cos_sim(q, m)
```

Stanford set all α=1, τ ≈ 1 sim-hour. For chat agents, importance is LLM-rated 1–10 at write time. **MemoryBank** layers Ebbinghaus on top: `R = exp(-t/S)`, on access `S += 1; t = 0`. Combine: filter by `R > 0.1` then rank by Stanford triple.

Beyond bag-of-vectors:
- **HippoRAG PPR**: extract query entities → seed PPR on KG → top docs by PPR mass. +20% on multi-hop QA over dense retrieval.
- **Zep temporal-aware indexing**: query-time expansion to constrain by `valid_at` window; +7–11% on temporal subset.
- **TiMem temporal tree** (Oct 2025): hierarchical episodes per time bucket → 76.88% LongMemEval-S, 27% smaller footprint.

Failure mode: importance scores are noisy at write time. EverMemOS (2025) uses *reconstructive* retrieval — re-score importance at read time conditioned on query.

Refs: [LongMemEval ICLR 2025](https://arxiv.org/abs/2410.10813) | [EverMemOS / Benchmarking 2510.27246](https://arxiv.org/pdf/2510.27246)

---

## 5. Rewriting & conflict resolution

Three patterns, increasing complexity:

1. **Mem0 ADD/UPDATE/DELETE/NOOP**: LLM judges new fact against neighbors via vector search; emits one of 4 ops. Simple, LLM cost per write, can thrash.
2. **Zep edge invalidation**: never delete; on contradiction, set `invalidated_at` on old edge, insert new edge with `valid_from = now`. Auditable, GDPR-friendly via tombstones.
3. **Recallr-style version chain**: linked list of versions per entity-attribute, dual timestamps (`event_time`, `ingest_time`), supersedes-edge. Classify conflict as `temporal_update | correction | preference_change | contradiction` and apply policy; ambiguous → ask user.

Multi-agent twist: last-writer-wins is unsafe. Use confidence-weighted writes or orchestrator arbitration.

For plaw v1: bi-temporal edges in SQLite (valid_from, valid_to, created_at, invalidated_at), no deletions. Mem0-style classifier as a single LLM call batched per session-end (not per turn — saves tokens).

Refs: [Mem0 paper](https://arxiv.org/html/2504.19413v1) | [Zep paper §3.2](https://arxiv.org/html/2501.13956v1) | [Hindsight is 20/20, 2512.12818](https://arxiv.org/html/2512.12818v1)

---

## 6. Long-term personalization

Two camps:

- **Letta core memory blocks** — persistent named scratchpads (`human`, `persona`) the agent edits via tools; always in-context. Identity = whatever is in the block.
- **PAMU / PersonaAgent (2025)** — explicit user profile object: demographics, preferences, traits. PAMU detects preference drift by short-term vs long-term divergence per dimension; updates only when divergence exceeds threshold.

AlpsBench (2026) finding: models reliably store explicit prefs but fail at *implicit* trait extraction and degrade sharply with distractors. Implication: don't try to auto-infer personality; store explicit user-stated prefs verbatim, infer only frequency-of-correction patterns ("user fixed date format 3×" → semantic rule).

For plaw: a Letta-style core block keyed by user, plus a `preferences` table with `(key, value, source_mem_id, confidence, last_confirmed_at)`. Display & edit in UI — user controls own profile.

Refs: [PAMU 2510.09720](https://arxiv.org/pdf/2510.09720) | [PersonaAgent OpenReview](https://openreview.net/forum?id=fgCOkyJG3f) | [Persistent Memory + User Profiles 2510.07925](https://arxiv.org/abs/2510.07925)

---

## 7. Cross-session continuity

- **Claude Code Session Memory** (v2.0.64+, 2026): background daemon writes/recalls; `claude -c` resumes most recent. Prints "Recalled/Wrote memories" as side-channel.
- **Cursor**: per-project `.cursor/` rules + per-workspace context cache; minimal long-term.
- **Devin**: workspace persistence + structured task journal; checkpoints for resumability.
- **Claude Projects**: shared project knowledge (files + memory) scoped to a project.

Pattern: project-scoped memory > global. Plaw should namespace memory by `(workspace_id, user_id)` and offer `/resume` that rehydrates last N episodic + active task state. Add a `CONTINUE.md`-style explicit handoff file the agent maintains under `plaw-data/<project>/HANDOFF.md` for human-readable continuity.

Refs: [Claude Code Session Memory](https://claudefa.st/blog/guide/mechanics/session-memory) | [Best Practices](https://code.claude.com/docs/en/best-practices)

---

## 8. Forgetting as a feature

Three layers:

1. **Decay-driven pruning**: Ebbinghaus `R < ε` → archive (cold tier) → after T days → hard delete. Reinforce on access.
2. **Strategic forgetting**: drop low-importance episodic after distillation succeeds; keep semantic. Inverse of "compress" — actually delete originals once gist is verified.
3. **Explicit forget (GDPR Art. 17)**: tombstone + cascade. With Zep-style bi-temporal edges, set `invalidated_at = now, reason = 'user_request'`; periodic vacuum hard-deletes after retention window. Without this, append-only stores are GDPR liabilities.

Failure mode: deleting evidence breaks reflection-derived semantic memories. Solution: provenance graph; on evidence delete, mark derived nodes `requires_reverification`.

Refs: [Memory Curation Rule](https://dev.to/askpatrick/the-memory-curation-rule-why-your-ai-agent-needs-to-forget-on-a-schedule-5ec8) | [GDPR Art. 17](https://gdpr-info.eu/art-17-gdpr/)

---

## 9. Multi-agent memory sharing

Four patterns (Confluent / blackboard literature 2025):

- **Blackboard** — shared kv/topic, all agents read/write. LbMAS reaches 81.7% avg over 6 benchmarks vs other multi-agent paradigms; lowest token cost.
- **Event sourcing** — append-only log of facts/intents; agents materialize own views. Auditable, replayable.
- **Read-only shared semantic + private episodic** — most practical; semantic is consensus, episodic is per-agent.
- **Orchestrator-arbitrated writes** — supervisor agent confidence-weights conflicting writes.

For plaw (single-user desktop, occasionally multi-agent like sub-agents in skills): event-sourced episodic log + read-only shared semantic + per-subagent scratchpads. Use SQLite WAL as the event log; subscribers tail on `last_seq_id`.

Refs: [LbMAS 2510.01285](https://arxiv.org/pdf/2510.01285v1) | [Confluent four patterns](https://www.confluent.io/blog/event-driven-multi-agent-systems/)

---

## 10. Storage stack for desktop (Tauri/Rust, self-contained)

| Need | Pick | Why |
|---|---|---|
| KV + relational + event log | **SQLite** (rusqlite) with WAL | Single file, ACID, ships with everything. Use one DB for episodic events + edges + metadata. |
| Vector index | **sqlite-vec** (asg017) | Single-extension load, 1M × 128d feasible, binary quantization, zerocopy Rust binding. Best for ≤1M vectors per user. |
| Heavier vector | **Qdrant Edge** (embedded) | If Qwen3 0.6B embeddings push past sqlite-vec sweet spot. In-process, no daemon, Rust-native. |
| Graph | **encode in SQLite** (`edges(src, dst, type, valid_from, valid_to, created_at, invalidated_at)`) | Embedded Neo4j is too heavy for desktop; PPR + 2-hop traversals are fine in SQL with recursive CTEs at this scale. |
| Cache (working memory) | **in-process** (Rust struct, persisted on tick) | No need for Redis. |

Avoid: shipping a JVM (Neo4j Embedded), running Qdrant as separate service (extra port, signed-binary headache for Tauri NSIS), Postgres+pgvector (overkill, deps).

Killer combo for plaw: **SQLite + sqlite-vec extension + bi-temporal edge table + provenance edges**, all in `plaw-data/<workspace>/memory.db`. One file, portable, GDPR-deletable, backupable.

Refs: [sqlite-vec](https://github.com/asg017/sqlite-vec) | [sqlite-vec Rust guide](https://alexgarcia.xyz/sqlite-vec/rust.html) | [Qdrant](https://github.com/qdrant/qdrant)

---

## Plaw-specific synthesis (5 bullets)

1. **Tiers**: working (in-context) | episodic (SQLite append) | semantic (SQLite + sqlite-vec) | procedural (skills as code, indexed). Letta-shaped, Mem0-economical.
2. **Bi-temporal edges everywhere**. Never delete on conflict — invalidate. Cheap in SQL, GDPR-correct, makes "rewind" trivial.
3. **Sleep-time consolidation** as a Tauri tokio job triggered on idle (>10 min, screen lock, or 24h max). Steps: distill episodic → semantic; reflect; rewrite stale semantic; verify procedural skills; vacuum tombstones past retention.
4. **Retrieval**: hybrid `α·recency + β·importance + γ·cos_sim`, with Ebbinghaus filter, plus optional 1-hop graph expansion via PPR over edges (SQL recursive CTE for ≤2 hops).
5. **Self-evolution loop**: Generative-Agents reflection + A-MEM write-time linking + Zep edge invalidation. Memories link, distill, supersede — never silently overwrite.

## Must-read shortlist

- Letta v1 blog — [letta.com/blog/letta-v1-agent](https://www.letta.com/blog/letta-v1-agent)
- Zep paper — [arxiv.org/abs/2501.13956](https://arxiv.org/abs/2501.13956)
- A-MEM (NeurIPS 2025) — [arxiv.org/abs/2502.12110](https://arxiv.org/abs/2502.12110)
- HippoRAG / HippoRAG 2 — [arxiv.org/abs/2405.14831](https://arxiv.org/abs/2405.14831)
- Sleep-time Compute — [arxiv.org/abs/2504.13171](https://arxiv.org/abs/2504.13171)
- Generative Agents — [arxiv.org/abs/2304.03442](https://arxiv.org/abs/2304.03442)
- LongMemEval — [arxiv.org/abs/2410.10813](https://arxiv.org/abs/2410.10813)
- Memory in the Age of AI Agents survey — [arxiv.org/abs/2512.13564](https://arxiv.org/abs/2512.13564)
