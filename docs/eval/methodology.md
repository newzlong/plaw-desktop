# plaw-eval Methodology

This document explains *why* every metric in plaw-eval is computed the
way it is. The implementations are in `crates/plaw-eval/src/{stats,
metrics, judges, runner}` — this doc is the prose justification.

The bar plaw-eval aims for is the rigour Anthropic published in their
**A Statistical Approach to Language Model Evaluations** (Miller, Nov
2024, [arXiv:2411.00640](https://arxiv.org/abs/2411.00640)). If your
intuition disagrees with anything below, please open an issue rather
than weakening it silently.

---

## 1. Why confidence intervals are non-negotiable

A bare accuracy number — "80%" — is barely more informative than a
horoscope. Two issues:

1. **Sample noise**: with n=30, a single lucky case flips you between
   77% and 80%. With n=300 it's the difference between two genuinely
   different systems and two trivially different ones.
2. **A vs B comparisons**: "the new prompt scores 78%, the old prompt
   scored 80%" tells you nothing about whether you regressed,
   improved, or sat perfectly still in the noise.

plaw-eval reports `mean ± 95% CI` for every metric, every run.
**Always.** A CI tells you the range of plausible values for the
*true* (population) score given the sample you observed. When two
runs' CIs overlap, you cannot honestly claim one beat the other on
that metric — even if the point means look different.

### Implementation

`stats::ci::t_distribution_ci` uses Student's t with n-1 degrees of
freedom (small-sample friendly). For binary success/failure metrics
we use Wilson score (`stats::ci::wilson_score_ci`), which is robust
near 0 and 1 where the normal approximation explodes. For non-normal
quantities (Bradley-Terry coefficients, ranking statistics) we use
1000-sample percentile bootstrap (`stats::ci::bootstrap_ci`).

---

## 2. Cluster-robust standard errors

Many natural eval setups have **correlated cases**:

- Multi-turn conversations (multiple turns share noise from the same
  user persona / topic)
- RAG suites grouping multiple questions per document
- Coding tasks with multiple sub-tasks per scenario
- Anything tagged with the same `cluster_id` in plaw-eval

If you treat 30 turns of one conversation as 30 independent
observations, your computed SE will under-estimate the true variance
by **3× or more** (Cameron & Miller 2015, "A Practitioner's Guide to
Cluster-Robust Inference"). You'll then "detect" improvements that
are actually within noise, and CI will be much narrower than they
should be.

### Implementation

`stats::cluster_se::cluster_robust_se` implements the Cameron-Miller
estimator with the standard `G/(G-1)` finite-cluster correction (the
same correction Stata's `vce(cluster)` defaults to). The
`should_use_cluster_se(n, n_clusters)` heuristic auto-engages
clustering when `n_clusters * 5 < n` — i.e. when each cluster averages
more than five members. Below that, naive SE is fine because
clustering doesn't have enough within-cluster correlation to matter.

### How to use

In `cases.toml`, tag related cases with the same `cluster_id`:

```toml
[[cases]]
id = "rag-doc-A-q1"
cluster_id = "doc-A"
input = { kind = "rag", question = "..." }

[[cases]]
id = "rag-doc-A-q2"
cluster_id = "doc-A"
input = { kind = "rag", question = "..." }
```

`runner::aggregate` will report both naive `stderr` and
`stderr_clustered`. The CI uses whichever is wider (cluster SE when
engaged, naive otherwise) — the conservative choice.

---

## 3. Paired difference analysis

When comparing baseline vs candidate, the **wrong** thing to do is:

> compute mean A separately, compute mean B separately, look at the
> difference and eyeball whether it "looks big".

The right thing is **paired analysis**: for each case ID present in
both runs, compute `candidate_score − baseline_score`, then report the
mean and CI of those *differences*.

Why it matters: case-level noise correlates across A and B (the same
case is "hard" for both versions of the system), so the variance of
the paired differences is much smaller than the variance of either
side alone. Anthropic's paper reports paired analysis can require
**4–10× fewer samples** for the same statistical power.

A run of n=30 paired ≈ a run of n=200 unpaired for the same effect
size at α=0.05. This is the single biggest reason the CI flow uses
paired analysis whenever possible.

### Implementation

`stats::paired::paired_difference(samples_a, samples_b, alpha)` does
exactly this. `report::gate::compare_runs` uses paired diff when case
IDs match between baseline and candidate, and falls back to
independent aggregates when they don't (you should rarely see the
fallback in practice — re-run the same suite, you get matching IDs).

---

## 4. Power analysis: how many cases is enough?

Power analysis answers: "to detect an effect of size δ at α=0.05 with
80% power, given metric standard deviation σ, how big does my sample
need to be?"

Standard formula:

```
n = ((z_{α/2} + z_β) · σ / δ)²
```

Concrete values for plaw-eval's vision targets:

| Effect to detect | σ (typical) | Required n (independent) | Required n (paired) |
|---|---|---|---|
| 5pp | 0.4 | ~503 | ~50 |
| 2pp | 0.4 | ~3140 | ~196 |
| 1pp | 0.4 | ~12560 | ~785 |

This is why the per-PR smoke eval runs n=30 (catches large
regressions, ε=1pp), nightly runs n=300 (catches medium regressions),
and weekly extended runs n=1000+ (catches subtle drift).

### Implementation

`plaw-eval power --effect <pp> --sigma <stdev>` computes the required
n for any combination. Use it before designing a new suite to make
sure your sample budget actually has the resolution you need.

---

## 5. Bradley-Terry MLE for win-rate aggregation

When using pairwise judges across many model variants (typical
plaw-elite scenario when comparing prompt revisions), you accumulate
a sparse `(model_a, model_b, winner)` graph. Naive win-counts are
unstable when models don't all face each other equally.

**Bradley-Terry MLE** estimates a single strength score per model that
maximises the likelihood of the observed comparison outcomes under:

```
P(i beats j) = p_i / (p_i + p_j)
```

This is the same algorithm LMSYS Arena uses to rank chat models. It
handles arbitrary pairing patterns (not all models face all models),
gracefully consumes ties (split as 0.5 wins each), and produces a
total ordering plus per-model bootstrap CI.

### Implementation

`stats::bradley_terry::bradley_terry_mle` solves it via the
Minorization-Maximization (MM) iteration from Hunter (2004). Default
tolerance 1e-8, max 1000 iterations. Companion
`bradley_terry_bootstrap_ci` returns 95% CIs on each entrant's score
by re-fitting on 1000 resampled comparison sets.

---

## 6. Pairwise judges: dual-pass position swap is mandatory

Single-pass pairwise comparison ("here is response A and response B,
which is better?") suffers from severe **position bias**: judges
prefer the response that comes first 60–75% of the time even when
content is identical (Shi et al., 2025; many earlier papers report
similar magnitudes). Single-pass pairwise scores are not measurements
— they're noise dressed in scientific clothing.

**Dual-pass position swap** runs the same comparison twice with the
positions swapped, and only counts a verdict when both passes agree.
When they disagree, the case is flagged as `PositionInconsistent` —
counted toward Tie for scoring purposes but tracked separately as a
bias indicator.

In plaw-eval, dual-pass is the **default and only sanctioned** mode
for production gating. The `JudgeMode::Pairwise` enum's `dual_pass`
field defaults to true, and you should never disable it for any
production run.

### Implementation

`judges::pairwise::compare_dual_pass` runs forward, then swapped, then
reconciles. The reconcile logic in `fn reconcile()` is exhaustive —
every (forward_verdict, swapped_verdict) pair maps to an explicit
`PairwiseDecision` variant.

---

## 7. Multi-judge jury & cross-family enforcement

Single judges have biases that can't be fully measured by their
self-reports. A judge model tends to prefer outputs that resemble its
own writing style ("self-preference bias", Liu 2024) and the magnitude
of this preference is a function of training data, not of the
candidate response's quality.

The fix is a **multi-judge jury** that includes models from at least
two distinct families (Anthropic, OpenAI, Kimi, DeepSeek, Qwen, ...).
plaw-eval's `Jury::new` takes a `min_distinct_families` parameter (2
is the production minimum) and refuses construction if the panel
doesn't meet it.

Why "family" not "model"? Because two models trained on overlapping
datasets at the same lab share the same biases. Kimi-K1.5 and
Kimi-K2.5 are not independent judges of one another's outputs even
though they have different model IDs.

### Aggregation strategies

- **Majority** (default for production): a verdict needs strict
  majority (>n/2 votes); ties or split votes mark the result
  inconclusive. Robust against any single judge being adversarial or
  miscalibrated.
- **ConfidenceWeighted** (Liu et al. LLM-as-a-Fuser): treats
  `PositionInconsistent` as half a vote toward Tie. Slightly more
  permissive, useful for higher-throughput screening.

`Inconclusive` jury verdicts feed the flywheel review queue rather
than blocking the gate — see `flywheel/`.

---

## 8. G-Eval scoring (free-form quality)

G-Eval (Liu et al., EMNLP 2023, [arXiv:2303.16634](https://arxiv.org/abs/2303.16634))
is the canonical scoring-judge baseline. It works by:

1. Auto-generating chain-of-thought evaluation steps for the
   dimension being graded ("coherence", "helpfulness", etc.).
2. Asking the judge to follow those steps and emit a JSON
   `{score, confidence, rationale}`.
3. Normalising the integer score to [0, 1] and weighting by the
   judge's self-reported confidence (low confidence pulls toward the
   midpoint, since a random guess centres there in expectation).

The original paper used token logprobs as the confidence signal. The
Anthropic-compatible APIs plaw uses (Kimi via api.moonshot.cn,
Anthropic Messages) don't expose logprobs, so we substitute a
`confidence` field the judge writes itself. Empirically this is
weaker than logprobs but stronger than ignoring confidence entirely.

### When to use

G-Eval is appropriate for:
- Free-form chat quality
- Helpfulness / coherence / relevance
- Domain-specific quality dimensions you can articulate in prose

It is **not** appropriate for:
- Verifiable outputs (use keyword coverage or structured grader instead)
- Tool-use accuracy (use `tool_*` metrics instead)
- Anything with a known ground truth (don't ask a judge to guess what
  you can compute)

---

## 9. Tool-use metrics

For agent trajectories we decompose tool-use quality into three
sub-metrics following Anthropic's "Demystifying Evals" framing:

| Metric | What it measures | Computed by |
|---|---|---|
| `tool_selection_f1` | Did the agent reach for the right tools? | Set-F1 of called vs expected |
| `tool_arg_validity` | Are the args structurally non-empty? | Fraction of non-null, non-empty args |
| `tool_redundant_rate` | Are calls repeating themselves? | Fraction of `(name, args)` exact-repeats |

Critically, **selection F1 ignores ordering and frequency**. Tools
called multiple times don't penalise selection F1 — that's
`redundant_rate`'s job. Tools called in the wrong order are caught by
`tool_sequence` matching (M5 future work), not selection F1. Splitting
these prevents one bad signal from drowning out the others.

`runner::tool_into_metric_map` inverts redundancy on output (`1 -
redundant_rate`) so larger is always better, matching every other
metric. This makes the gate logic uniform — no special-casing
"smaller-is-better" metrics in the regression check.

---

## 10. Production trace flywheel

The eval suite shouldn't be static. As real users hit edge cases the
authored cases miss, those failures should flow back into the suite.
The pipeline:

```
production trace → sample (1–10%) → async LLM-judge for
weak signals → low-score traces queued for human review →
approved traces promoted to a suite as new cases
```

`flywheel/sampler.rs` does the sampling, `flywheel/reviewer.rs` is
the human-in-the-loop interface, `flywheel/promoter.rs` handles
TOML insertion. Approved cases are tagged `source = "flywheel"` and
`promoted_at = "2026-04-28T..."` so we can audit the suite's
evolution.

The point isn't automation for its own sake — it's that the eval
suite should track real-world failure modes, not just whatever cases
the original author imagined.

---

## 11. Gate logic: lower CI bound, not point mean

The gate fails a PR when:

```
candidate.lower_ci_bound < baseline.mean − epsilon
```

Not when `candidate.mean < baseline.mean`. Reasons:

- Comparing point means rewards lucky noise. A PR that sat exactly on
  the baseline by chance flips green/red depending on which random
  cases were sampled that day.
- The lower CI bound represents the *worst statistically defensible
  reading* of the candidate. If even that is safely above
  `baseline − ε`, you can defend the merge.
- ε defaults to 1pp. Tighten it (smaller ε) for more sensitive
  metrics; loosen it (larger ε) for noisier ones. Keep it constant
  per-suite once chosen — moving epsilon between PRs is a way to
  smuggle regressions in.

When neither baseline nor candidate has data for a metric (or only one
side does), the gate marks that metric `Inconclusive` rather than
silently passing. Inconclusive metrics surface in the PR comment but
don't block merge unless every metric is inconclusive.

---

## 12. What plaw-eval does *not* do

Listed honestly so future work knows where to slot:

- **No sampling stratification by tag**. Today `--n` is a uniform
  random sample over all cases. For suites with imbalanced tags
  (e.g. 90% chat / 10% adversarial) we should stratify. Slated for
  M9+.
- **No multi-arm comparison**. `compare` handles two runs at a time.
  Comparing five prompt variants requires Bradley-Terry MLE on the
  pairwise win matrix — the math is in `stats::bradley_terry`, the
  CLI exposure isn't.
- **No human eval crowd-sourcing**. The flywheel handles single-user
  review. Multi-reviewer agreement (Cohen's κ etc.) would need a
  different schema.
- **No automatic suite repair**. When a metric breaks because plaw
  legitimately changed behaviour and the old golden answer became
  wrong, you have to update the suite by hand. We could imagine an
  LLM-suggested edit flow but it's a long way off.
- **No fine-grained rubrics for G-Eval**. The judge gets a single
  `dimension` string today. Anthropic's eval guidance pushes harder
  on multi-criterion rubrics (DAG-of-checks). Slated for follow-up.

---

## 13. References

- Miller, "A Statistical Approach to Language Model Evaluations",
  Anthropic 2024 — [arXiv:2411.00640](https://arxiv.org/abs/2411.00640)
- Cameron & Miller, "A Practitioner's Guide to Cluster-Robust
  Inference", J. Human Resources 2015
- Liu et al., "G-EVAL: NLG Evaluation using GPT-4 with Better Human
  Alignment", EMNLP 2023 — [arXiv:2303.16634](https://arxiv.org/abs/2303.16634)
- Shi et al., "Judging LLM-as-a-Judge with MT-Bench and Chatbot
  Arena" (extended), 2025 — position-bias measurements
- Hunter, "MM algorithms for generalized Bradley-Terry models", Ann.
  Stat. 2004 — Bradley-Terry MM derivation
- Anthropic, "Demystifying Evals for AI Agents", 2026 —
  trajectory vs outcome metric framing
