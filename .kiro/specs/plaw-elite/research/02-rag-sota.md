# RAG SOTA — April 2026

Scope: design notes for Plaw's retrieval layer. Local-first (Tauri + Qwen3-Embedding 0.6B via llama.cpp), agentic loop already in place, memory capsules to integrate. Assume reader knows the basics.

---

## 1. Agentic RAG Architectures

The 2025–2026 consensus per the SoK survey (arXiv 2603.07379) and the IJCNLP/ACL 2025 survey on Reasoning Agentic RAG: static "retrieve-then-generate" is dead for non-trivial tasks. Winning patterns split along two axes — *when* to retrieve (gating) and *how* to plan (routing/decomposition).

- **Self-RAG** — model emits reflection tokens (`Retrieve?`, `IsRel`, `IsSup`, `IsUse`) to self-gate retrieval and grade its own outputs. Strong for analytical/long-form tasks; expensive (extra critic calls), dataset-bound (trained on specific reflection vocab).
- **Corrective RAG (CRAG)** — lightweight retrieval evaluator scores hits as Correct / Ambiguous / Incorrect, then triggers fallback (web search) or knowledge refinement. The most production-friendly self-correcting pattern; small evaluator model is cheap. Hybrid retrieval + neural rerank + CRAG correction is the empirical winner on text-and-table benchmarks (Recall@5=0.816, MRR@3=0.605 in arXiv 2604.01733).
- **Adaptive-RAG** — classifier routes queries by complexity to {no-retrieval, single-step, multi-step}. Best cost-discipline approach; needs a complexity classifier (small model or heuristic).

**Verdict for Plaw**: implement CRAG-style evaluator (LLM-as-judge over top-k with single boolean) + Adaptive-RAG router (classify query: code / docs / memory / web). Skip Self-RAG's reflection-token training; it requires a fine-tuned model.

Refs:
- [SoK: Agentic RAG (2026)](https://arxiv.org/abs/2603.07379)
- [Agentic RAG Survey (Singh et al.)](https://arxiv.org/abs/2501.09136)
- [Reasoning RAG: System 1 / System 2 survey](https://arxiv.org/html/2506.10408v1)

---

## 2. Query Understanding & Rewriting

- **HyDE** — "shockingly effective" zero-shot. Generate a fake answer, embed it, retrieve. Wins when query/doc lexical gap is large (NL question vs. technical doc). Fails on entity-heavy lookups (drifts away from exact names) and slows latency by one LLM call.
- **Step-back prompting** — abstract the query upward before retrieving (e.g., "what is X's policy on Y?" → "what are the company's general policies?"). Wins for principle/theory questions. Useless for factoid lookups.
- **Multi-query / RAG-Fusion** — N reformulations + RRF merge. The 2025 MQRF-RAG paper (ACM 3728199.3728221) shows multi-strategy ensembles beat any single rewrite by 7–14% on FreshQA / HotPotQA.
- **Query decomposition** — split compound queries into sub-questions, retrieve each independently. Mandatory for multi-hop ("compare X and Y on Z").

**Verdict for Plaw**: cheap router decides {raw, HyDE, decompose}. Skip step-back unless we add doc-policy tooling. RAG-Fusion as default for ambiguous queries (3 reformulations + RRF).

Refs:
- [Adaptive HyDE for LLM developer support (2025)](https://arxiv.org/html/2507.16754v1)
- [MQRF-RAG: Markov decision process query rewriting](https://dl.acm.org/doi/10.1145/3728199.3728221)
- [Six Query Transformation Architectures](https://www.dmflow.chat/en/blog/rag-query-transformation-guide-6-advanced-architectures)

---

## 3. Retrieval Methods & Embeddings

**SOTA embeddings (April 2026)**:
- **Llama-Embed-Nemotron-8B** — #1 on MMTEB across 250+ languages, open-weight (NVIDIA, Oct 2025). Too heavy for desktop.
- **Qwen3-Embedding-8B** — 70.58 MTEB Multilingual mid-2025, 32K context, 100+ langs. Plaw's chosen 0.6B variant is the same family — solid CN/EN coverage.
- **Microsoft Harrier-OSS-v1** — 270M / 0.6B / 27B sizes, SOTA on MMTEB v2 (March 2026), 32K context. The 0.6B is a direct Qwen3-Embedding-0.6B competitor and worth A/B testing.
- **Gemini Embedding 2** (March 2026) — 5 modalities, 100+ langs, native Matryoshka — irrelevant for local but strongest for cloud fallback.

**Hybrid retrieval is non-negotiable**: BM25 + dense + RRF (k=60) lifts recall@10 from 65–78% to ~91% (Elastic / Qdrant production data). Pure dense misses entity/SKU/identifier queries — exactly what code-and-docs agents hit constantly.

**Late interaction / learned sparse**:
- **ColBERTv2** with PLAID — token-level MaxSim, expensive storage but best zero-shot OOD. **SPLATE** (SIGIR 2024) makes ColBERT CPU-friendly via SPLADE candidate gen + late-interaction rerank — re-ranks 50 docs <10ms.
- **SPLADE++** — learned sparse, BEIR SOTA in-and-out-of-domain. Lives in inverted index (Tantivy-compatible).

**Verdict for Plaw**: Qwen3-Embedding-0.6B (already chosen) + Tantivy BM25 + RRF. Defer ColBERT/SPLADE — desktop storage cost is prohibitive (3–4× index size). Re-evaluate Harrier-0.6B once it has GGUF.

Refs:
- [MTEB Leaderboard](https://huggingface.co/spaces/mteb/leaderboard)
- [Qwen3-Embedding repo](https://github.com/QwenLM/Qwen3-Embedding)
- [SPLATE: Sparse Late Interaction (SIGIR 2024)](https://arxiv.org/abs/2404.13950)
- [Hybrid Search Done Right (Feb 2026)](https://ashutoshkumars1ngh.medium.com/hybrid-search-done-right-fixing-rag-retrieval-failures-using-bm25-hnsw-reciprocal-rank-fusion-a73596652d22)

---

## 4. Reranking

April 2026 ELO leaderboard (Agentset): **ZeroEntropy zerank-2** (1638) > **Cohere Rerank v4 Pro** (1629) > **BGE-reranker-v2-m3** (self-hosted, multilingual). Latency ranges: BGE-base p95=92ms, BGE-large p95=145ms, Jina v3 188ms, Nemotron 243ms.

- **Cross-encoder rerank** — top-50 → top-10. Mandatory for production. CPU-only `bge-reranker-base-v2` ~350ms; GPU drops to 80ms.
- **LLM listwise rerank** (RankGPT, RankLLM) — best quality, prohibitive cost/latency. Use only when tail latency is acceptable.
- **FlashRank** — pure-Rust-friendly small CE. Practical for desktop.

**Verdict for Plaw**: BGE-reranker-v2-m3 (multilingual, matches Qwen ecosystem) via ONNX/ort. Top-50 dense+sparse → top-8 reranked. Skip LLM listwise (Kimi K2.5 cost).

Refs:
- [Reranker Leaderboard (Agentset, 2026)](https://agentset.ai/rerankers)
- [Best Reranker Models Open-Source vs API (Feb 2026)](https://docs.bswen.com/blog/2026-02-25-best-reranker-models/)

---

## 5. Knowledge Representation

- **Contextual Retrieval (Anthropic)** — prepend a chunk-specific 50–100 token context blurb (generated by cheap LLM with prompt cache) before embedding *and* before BM25 indexing. **−49% retrieval failure**, **−67% with rerank**. With Claude prompt caching, indexing cost is ~$1/M doc tokens. **This is the best price/quality chunking technique in 2026.**
- **Late chunking (Jina, arXiv 2409.04701)** — encode the full doc with a long-context embedder once, then mean-pool token spans into chunk vectors. Cheaper than Anthropic's approach (no LLM rewrites) and preserves cross-chunk context. Requires long-context embedder (Qwen3-Embedding 32K is fine).
- **RAPTOR** — recursive cluster + summarize → tree of summaries; query both leaves and summaries. Wins on multi-doc synthesis questions. Indexing is expensive but one-shot.
- **GraphRAG family** — original Microsoft GraphRAG ($33K to index a corpus) is dead in production. **LazyGraphRAG** matches its quality at 0.1% indexing cost and 1/700 query cost; **HippoRAG 2** is ~10× more efficient, neurobiologically-inspired multi-hop; **PathRAG** flow-pruning cuts context −44%. ICLR'26 GraphRAG-Bench: graphs only win on multi-hop / global-summarization queries; vector RAG wins on factoid.
- **Small-to-big** — embed small (sentence/proposition) for precision, retrieve big (parent paragraph/section) for context. Dead-simple, large quality lift, near-zero implementation cost.

**Verdict for Plaw**: 
1. **Late chunking** for code & docs (Qwen3 32K context is sufficient).
2. **Contextual Retrieval** for memory capsules (use Kimi K2.5 with prompt caching to add capsule context).
3. **Small-to-big** retrieval (embed propositions, return parent function/section).
4. Skip GraphRAG initially — code/docs/memory don't need entity graphs. Revisit if cross-capsule reasoning becomes a use case (LazyGraphRAG is the entry point).

Refs:
- [Anthropic Contextual Retrieval](https://www.anthropic.com/news/contextual-retrieval)
- [Late Chunking (Jina, arXiv 2409.04701)](https://arxiv.org/pdf/2409.04701)
- [LazyGraphRAG (Microsoft Research)](https://www.microsoft.com/en-us/research/blog/lazygraphrag-setting-a-new-standard-for-quality-and-cost/)
- [GraphRAG-Bench (ICLR'26)](https://github.com/GraphRAG-Bench/GraphRAG-Benchmark)

---

## 6. Long Context vs RAG

April 2026 production data (TianPan, MindStudio, Claude 1M GA):
- 1M-token request: ~45s latency, $15–30/req on Opus. RAG: ~1s, $0.05–0.15/req. **100–200× cost gap, 30–60× latency gap**.
- Multi-fact recall in 1M context averages ~60% (40% silent miss rate).
- Long context wins **only** when corpus < 100K tokens, stable, and queries are conversational/whole-doc reasoning.

**Winning hybrid** (universal in 2025–2026 production): RAG retrieves 50K–300K relevant tokens → feed to long-context model with prefix caching. Best of both: precision from retrieval, synthesis from long context.

**Verdict for Plaw**: Kimi K2.5 has long context but cost matters. Default to RAG retrieve top-N → ~16K context budget. For "summarize this whole codebase" / "review this file" use cases, switch to long-context mode (skip retrieval, dump file, rely on prompt cache).

Refs:
- [Long-Context vs RAG production framework (April 2026)](https://tianpan.co/blog/2026-04-09-long-context-vs-rag-production-decision-framework)
- [Claude 1M Context Reality Check (TokenMix)](https://tokenmix.ai/blog/1m-token-context-reality-check-2026)

---

## 7. Memory-Aware Retrieval (记忆胶囊)

This is where Plaw's "记忆胶囊" lives. The 2026 design space:

- **Letta (MemGPT)** — OS-style three tiers: core (in-context), recall (searchable history), archival (cold storage). Agent decides retrieval via tool calls. Strong primitive, heavyweight runtime.
- **Mem0** — best for personalization (user prefs, behavioral patterns); commercial-leaning.
- **MemMachine** (arXiv 2604.04853, 2026) — short-term + episodic + profile memory; introduces *contextualized retrieval* that expands nucleus matches with neighboring episode context. Closest to what plaw memory capsules should do.
- **Tripartite typing**: episodic (events), semantic (consolidated facts), procedural (learned workflows). Retrieve different types via different indexes.

**Pattern that works for Claude-Code-style agents**: capsule = (summary, episode chunks, derived facts). Embed summary for coarse retrieval; on-hit, expand to surrounding episode (small-to-big). Promote facts to semantic store after K accesses (consolidation). Letta's filesystem-as-memory benchmark suggests structured FS storage often beats vector-only for agent memory.

**Verdict for Plaw**:
1. Capsule-level vector index (embed summary + key facts).
2. Episode expansion on retrieval (small-to-big).
3. CRAG evaluator decides if memory is sufficient or external retrieval needed.
4. Procedural memory (skills) as separate index, retrieved by task-pattern match.
5. Consolidation job: episodic → semantic after access threshold.

Refs:
- [Letta agent memory architecture](https://www.letta.com/blog/agent-memory)
- [MemMachine (arXiv 2604.04853)](https://arxiv.org/html/2604.04853v1)
- [State of AI Agent Memory 2026 (Mem0)](https://mem0.ai/blog/state-of-ai-agent-memory-2026)

---

## 8. Local-First / Rust Implementation

**Vector store**: 
- **LanceDB** — embedded, columnar, zero-copy, native Rust SDK. Disk-efficient at >RAM scale. Best fit for Tauri sidecar.
- **Qdrant (embedded mode)** — Rust-native, mature filtering, but service-oriented; embedded mode is testing-tier.
- **sqlite-vec** — minimal, single-file, fits Plaw's portable mode philosophy. Limited to flat scan + brute-force or basic ANN; viable for <100K vectors.

**Recommendation**: LanceDB for code/docs index (>100K chunks plausible), sqlite-vec for memory capsules (smaller, benefits from SQL joins with capsule metadata).

**Embedding inference**: 
- **llama.cpp** (current) — Qwen3-Embedding-0.6B reportedly ~5× slower than equivalent-size embedders ([llama.cpp issue #19933](https://github.com/ggml-org/llama.cpp/issues/19933)). Watch for fix or migrate.
- **ort (ONNX Runtime)** — 3–5× faster than Python equivalents, 60–80% less memory. Convert Qwen3-Embedding-0.6B to ONNX and ship via `ort` crate.
- **Candle** — pure Rust, growing model zoo, good for inference deployment. Strong fallback if ONNX conversion fails.

**Sparse / BM25**: **Tantivy** (Rust Lucene) — 15–20× faster indexing than Python alternatives. Vector-search integration in 0.22+. Use Tantivy for BM25 + LanceDB for dense, fuse via RRF in Rust.

**Reranker**: **BGE-reranker-v2-m3** (multilingual, 568M) via `ort`. Top-50 → top-8.

**Verdict — Plaw stack**:
```
Qwen3-Embedding-0.6B (ort, ONNX)  ──┐
Tantivy BM25                        ├─→ RRF (Rust) ─→ BGE-reranker-v2-m3 (ort) ─→ Kimi K2.5
LanceDB (chunks) + sqlite-vec (capsules)
```

Indexing: late chunking (Jina-style) for code/docs; contextual retrieval (Kimi + prompt cache) for capsules; small-to-big expansion at retrieval.

Refs:
- [LanceDB Rust](https://github.com/lancedb/lancedb)
- [Tantivy](https://github.com/quickwit-oss/tantivy)
- [Building Sentence Transformers in Rust (Burn / ort / Candle)](https://dev.to/mayu2008/building-sentence-transformers-in-rust-a-practical-guide-with-burn-onnx-runtime-and-candle-281k)
- [Qwen3-Embedding llama.cpp speed issue](https://github.com/ggml-org/llama.cpp/issues/19933)

---

## TL;DR Design Recommendation

1. **Index**: Tantivy (BM25) + LanceDB (dense). Late chunking for code/docs. Contextual retrieval for memory capsules.
2. **Retrieve**: Adaptive-RAG router → {raw / HyDE / decompose} → hybrid (BM25 ∥ dense, RRF k=60) → BGE rerank → top-8.
3. **Verify**: CRAG-lite evaluator; on Incorrect/Ambiguous, fall back to web search or memory-capsule scan.
4. **Memory**: capsule summary embeds → small-to-big episode expansion → procedural store separate. Consolidate episodic → semantic after K hits.
5. **Long context**: keep RAG default; switch to dump-and-cache for whole-file/whole-codebase tasks.
6. **Inference**: migrate embedding from llama.cpp to ort (ONNX) until upstream speed fix lands.

Avoid building: GraphRAG, ColBERT/SPLADE indexes, Self-RAG fine-tuning, LLM listwise rerank. They are not justified at desktop scale or against Kimi-K2.5 token budget.
