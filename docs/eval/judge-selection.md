# Judge Selection Guide

This is the prose companion to `JudgeSpec` in `cases.toml`. The TLDR:
**use a multi-judge jury with at least two distinct families for any
production gating**. Single judges (and single-family juries) are
fine for local debugging but should not block merges.

---

## 1. The biases you're trying to dodge

Three known biases that warp pairwise judgements:

| Bias | What it does | Mitigation in plaw-eval |
|---|---|---|
| **Position bias** | Judge prefers whichever response comes first 60–75% of the time, even with identical content (Shi et al. 2025). | Mandatory dual-pass position swap in `compare_dual_pass`. Verdicts only count when both passes agree. |
| **Self-preference bias** | A model judges its own outputs higher than an outside critic does (Liu 2024). Magnitude varies by model family but is reliably positive. | Cross-family enforcement in `Jury::new`. Refuses panels with fewer than `min_distinct_families` distinct families. |
| **Verbosity bias** | Judges prefer longer responses irrespective of content (multiple papers). | Partially controlled by tightening `dimension` in G-Eval prompts. Watch output length distribution as a debugging signal. |

There are more (sycophancy bias, format bias, authority bias, ...);
the three above are the ones plaw-eval actively fights with code.

---

## 2. Family vs model

A "family" is a coherent training lineage. Two models from the same
family share a lot of training data and value-shaping, so they make
correlated judgement errors. Treat these as the same family:

- Anthropic: Claude 3.x, Claude Sonnet 4.x, Claude Opus 4.x, Claude
  Haiku 4.x — same family.
- OpenAI: GPT-4, GPT-4o, GPT-4o-mini, GPT-5.x — same family.
- Kimi (Moonshot): Kimi K1.5, K2, K2.5 — same family.
- DeepSeek: DeepSeek-V2, V3, R1 — same family.
- Qwen / Tongyi (Alibaba): Qwen2, Qwen3 — same family.

Independence requires **different families**, not just different
models. Two Claude variants in a jury is functionally a single judge
with extra latency.

---

## 3. Picking a default judge for a suite

For most plaw-eval suites the default should be:

```toml
[default_judge]
model = "kimi-k2.5"
provider = "kimi"
temperature = 0.0
mode = { kind = "pairwise", dual_pass = true }
```

Why Kimi K2.5 as default:

- Plaw uses Kimi K2.5 as its main model. Using the same model as
  default judge is *fine for local sanity checks* (cheaper, faster)
  but **must not be the production gating judge** because that's a
  textbook self-preference setup. Cross-family judges step in for
  gating.
- For local debugging the small bias is acceptable in exchange for
  speed.
- `temperature = 0.0` makes results closer to reproducible (subject
  to serverless non-determinism — see `methodology.md` §1).

For production gating (CI smoke / nightly), upgrade to a jury:

```toml
[default_judge.mode]
kind = "jury"
aggregator = "majority"
models = [
    { model = "claude-sonnet-4.5", provider = "anthropic", temperature = 0.0,
      mode = { kind = "pairwise", dual_pass = true } },
    { model = "gpt-4o-mini", provider = "openai", temperature = 0.0,
      mode = { kind = "pairwise", dual_pass = true } },
    # The kimi entry above is still the suite's `default_judge`; the
    # jury runs alongside it and aggregates — you don't need to repeat
    # kimi in the jury list unless you want it as a third voter.
]
```

Three voters from three families is the production sweet spot.
`majority` aggregator needs ≥2 to agree. Two voters from two families
also works but ties → Inconclusive more often.

---

## 4. Aggregator choice

| Aggregator | When to use |
|---|---|
| `majority` (default) | Production gating. Conservative — splits become inconclusive instead of forcing a verdict. |
| `confidence_weighted` | Higher-throughput screening or flywheel auto-grading. Weights position-inconsistent judgements as 0.5 toward Tie. More verdicts, more noise. |

Don't ship the same suite with `confidence_weighted` to production
gating — the gate's job is to NOT let mushy verdicts past, and the
cost of a true regression sneaking through dwarfs the cost of one
PR being told "the gate was inconclusive, please rerun".

---

## 5. Judge cost & latency

Each pairwise dual-pass judge call is **two LLM completions**. Each
jury member adds another two. So a 3-member jury per case is 6 LLM
calls per case for the pairwise comparison alone, plus whatever each
metric (G-Eval etc.) costs.

Concrete cost per 30-case smoke eval at typical token counts:

| Judge config | Cost per smoke run (USD) | Notes |
|---|---|---|
| Single Kimi K2.5 (dual-pass) | ~$0.05 | Cheapest; only for local |
| Single Claude Sonnet (dual-pass) | ~$0.30 | Mid; can gate small repos |
| 3-judge jury (Kimi+Claude+GPT-4o-mini) | ~$0.50 | Production-grade gate |
| 5-judge jury | ~$1.00 | Higher-stakes evals only |

The judge cache (`plaw-eval cache`) deduplicates identical
`(prompt, input, model_version)` triples, so re-running an unchanged
PR is nearly free. Cost mostly hits the first run after a prompt
update.

---

## 6. Keeping a judge calibrated

Once a year (or after every major model release, whichever sooner):

1. Pull a sample of 50–100 production traces with diverse
   characteristics.
2. Have at least two humans grade them blind.
3. Run your jury over the same set.
4. Compute Spearman correlation jury-vs-human. Target ≥ 0.80.

If the correlation drops below 0.70, the judge has drifted. Investigate
before letting it gate further merges. Common causes:

- Vendor silently updated the judge model under the same model ID.
- The suite's case distribution changed (more edge cases the judge
  hasn't seen).
- Judge prompt got mutated (e.g. someone "improved" the rubric and
  inadvertently changed the implicit standard).

The fix is usually: pin to a more specific model ID (e.g.
`claude-sonnet-4-5-20251022` instead of `claude-sonnet-4.5`) and add
a regression test for the new model when you upgrade.

---

## 7. Debugging a flaky judge

Symptoms:

- Same PR rerun produces different gate verdicts.
- High `PositionInconsistent` rate from `compare_dual_pass`.
- Inconclusive PR comments more than ~10% of runs.

Diagnostics:

1. Check `cache_read_input_tokens` ratio — if it's near 0, the cache
   isn't deduplicating, so the judge is genuinely re-rolling.
2. Look at the raw judge text in the failing case's `metric_scores.raw`
   — does the judge follow the rubric, or wander off?
3. Run the same case 5× with the same judge. If verdicts vary, the
   judge isn't stable on this case shape — refine the case (often
   the *case* is ambiguous, not the judge).

If a single case is flaky across multiple judges, the case itself is
the problem — it asks for a subjective judgement that humans would
disagree on too. Either drop it, sharpen the rubric, or add it to the
flywheel for re-design.

---

## 8. Special judge modes (future work)

These are **not** implemented in Phase 1 but worth noting for the
roadmap:

- **Constitutional rubric judges**: encode multiple safety/quality
  principles as separate Boolean checks and require all to pass.
  Better for compliance gates than scoring judges.
- **Self-consistency** (Wang et al. 2022): sample N completions from
  the same judge prompt and majority-vote among them. Cuts noise but
  N× the cost.
- **Chain-of-Verification (CoVe)**: ask the judge to write the
  rationale, then ask a second pass to verify the rationale is
  internally consistent. Catches a class of judge hallucinations.
- **DAG judges** (DeepEval-style): break "is this response good" into
  a tree of yes/no checks, each cheaper to grade than the parent.
  Slated for `phase-2-architecture/` work.

For Phase 1, pairwise + jury + G-Eval covers the 80% case.
