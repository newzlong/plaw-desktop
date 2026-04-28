# `rag_grounded_qa` — RAG faithfulness + answer relevancy

> **Status: stubbed.** This suite needs a fixed corpus + retrieval
> harness that doesn't exist yet. Don't gate on it until the design
> notes below are resolved.

## Testing thesis

When plaw retrieves documents and answers from them, two things can go
wrong independently:

1. **Faithfulness**: the answer makes claims the documents don't support.
2. **Relevancy**: the answer is faithful but doesn't actually address the question.

This suite measures both as separate scalars so we can tell which axis
regressed when a RAG-related PR lands.

## Why it's stubbed

Three open design questions block real cases:

1. **Corpus identity.** Should the corpus be:
   - A frozen snapshot of plaw's own docs? (Easy, narrow)
   - A subset of Wikipedia? (Standard, doesn't reflect real use)
   - A fabricated mini-corpus per case? (Best signal, most work)
2. **Retrieval coupling.** Plaw's RAG layer doesn't yet expose a stable
   "what got retrieved" API to the eval runner. Without that, we can't
   compute precision/recall on retrieval.
3. **Adversarial design.** A serious RAG suite needs OOD cases (info
   not in corpus → expected: graceful refusal) and red-team cases
   (corpus contains misleading-but-relevant doc → expected: don't use
   it). Designing those needs a real corpus first.

## Required before un-stubbing

- [ ] Pick corpus strategy (recommend: per-case mini-corpus, ~5 docs each)
- [ ] Add `retrieved_docs` field to plaw's `tool_result` event payload
- [ ] Implement `metrics::faithfulness` (claim extraction → doc verification)
- [ ] Implement `metrics::answer_relevancy` (embedding sim or judge)
- [ ] Implement `metrics::context_precision` and `metrics::context_recall`

The metric crate has placeholders for these (see [tasks.md M5.T5.3-T5.6](../../.kiro/specs/plaw-elite/phase-1-eval/tasks.md#L126)).

## Capability dimensions (for when we un-stub)

| Dimension | Why we test | Designed target |
|-----------|------------|----------------|
| In-distribution | Standard RAG happy path | 8 |
| Out-of-distribution | Q can't be answered from corpus → refuse | 8 |
| Adversarial | Misleading doc present → ignore it | 6 |
| Multi-hop | Answer requires combining 2+ docs | 4 |
| Citation precision | Answer must name the source doc | 4 |

## Stub case

A single seed case is included so the suite loads and the harness
exercises the `kind = "rag"` codepath. It is **not** a test of plaw's
RAG quality — it's a smoke test of the eval runner.
