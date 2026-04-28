# Designing plaw-eval Suites

A plaw-eval suite is a TOML file at `evals/<suite_name>/cases.toml`.
This guide is about the parts that aren't obvious from reading the
schema — what makes a *good* case, how to use `cluster_id` correctly,
how to scope a suite, when to split.

For the schema itself, see [`evals/_template/cases.toml`](../../evals/_template/cases.toml).

---

## 1. What a suite is for

A suite measures **one capability dimension** of plaw at scale.
Examples:

- `chat_quality` — single-turn / multi-turn conversational quality
- `tool_routing` — agent picks the right tool for the request
- `rag_grounded_qa` — agent retrieves and cites correctly
- `agent_multi_step` — multi-step task completion
- `error_recovery` — agent handles tool failure gracefully
- `adversarial` — agent resists prompt injection / jailbreaks

A suite does **not** measure the entire system. If your one suite
tries to do everything, your CI gate will be unactionable ("something
got worse, but where?"). Split early.

Rule of thumb: when you can't write a one-sentence description of
what a metric on this suite means in regression terms, the suite is
too broad.

---

## 2. Anatomy of a good case

A good case has three properties:

1. **Verifiable**: there's a defensible right answer or a defensible
   "this is much better than that" judgement.
2. **Discriminating**: a worse model / prompt / config produces a
   visibly worse output.
3. **Stable**: the right answer doesn't drift week-to-week with
   model updates or world events.

Each of these has implications:

### Verifiable

For RAG / factual cases, supply `expected.answer_keywords` so the
deterministic `keyword_coverage` metric can grade without needing a
judge. Keywords beat free-form expected answers because they survive
paraphrase variation.

```toml
[[cases]]
id = "geo-paris-capital"
[cases.input]
kind = "chat"
messages = [{ role = "user", content = "What is the capital of France?" }]
[cases.expected]
answer_keywords = ["Paris"]
```

For free-form cases (creative writing, summarisation, advice), use
`g_eval` with a tight `dimension`. Vague dimensions ("overall
quality") are noise; specific dimensions ("answers the question
without padding") are signal.

### Discriminating

Before merging a case, run it once at full quality and once at known
degraded quality (e.g. older prompt, smaller model). If the metric
score is identical, the case isn't measuring anything — drop it.

A common failure mode: cases that every reasonable model handles
correctly. They contribute zero variance, dilute your sample, and
make CI tighter than your real signal would justify.

### Stable

Avoid cases whose right answer depends on:

- Today's date ("what is the latest version of plaw?")
- Recent events
- Specific external services that change
- The model's own training cutoff

If you must include time-sensitive content, supply the answer in
`expected.answer_keywords` so the keyword check is stable even when
the model's knowledge isn't.

---

## 3. The `cluster_id` field

`cluster_id` tells the aggregator that cases sharing this label are
**not statistically independent**. The runner switches to
cluster-robust SE when this is set on enough cases.

### When to set it

- Multi-turn conversations where each turn becomes its own case →
  cluster all turns of one conversation together.
- Multiple questions asked of one document in a RAG suite → cluster
  by document.
- Cases derived from the same template (e.g. "what is the capital of
  X" with X varying) → cluster by template.

### When NOT to set it

- 30 totally unrelated cases? Leave `cluster_id` unset. They're
  independent by construction; cluster SE would only add noise.
- Just because cases share a tag doesn't make them clustered. Tags
  are for organisation; clusters are for statistics.

### What good clustering looks like

Plaw-eval auto-engages clustering when `n_clusters * 5 < n` — i.e.
average ≥ 5 cases per cluster. So if you only have 2 cases in a
"cluster" the heuristic correctly skips it. If you have 30 cases
spread evenly across 6 clusters, clustering kicks in.

Practical example for a 30-case RAG suite covering 6 documents:

```toml
[[cases]]
id = "doc-rust-book-q1"
cluster_id = "rust-book"
# ...
[[cases]]
id = "doc-rust-book-q2"
cluster_id = "rust-book"
# ...
[[cases]]
id = "doc-tokio-tutorial-q1"
cluster_id = "tokio-tutorial"
# ...
```

Six clusters × five cases each = 30 total → cluster SE engaged
automatically.

---

## 4. Sample size: how many cases?

Use `plaw-eval power --effect <pp> --sigma <stdev>` to compute the
required n for the smallest effect you care about detecting. The
results from §4 of `methodology.md`:

| To detect | Need (paired) |
|---|---|
| 5pp | ~50 |
| 2pp | ~196 |
| 1pp | ~785 |

Phase 1 ships with 30-case suites because the smoke eval should be
cheap (per-PR cost) and only needs to catch large regressions. The
nightly run with n=300 catches the 2pp band. For tighter sensitivity
you'd need a weekly extended run.

A suite of 30 cases is enough to **detect 5pp regressions reliably**.
It is NOT enough to detect 1pp drift. Don't gate ε at 0.005 against
a 30-case run — the gate will flap on noise.

---

## 5. Tags

`tags` is free-form metadata. Use it for:

- Filtering during local debugging (`plaw-eval run --suite X` with
  filter — todo, M11+)
- Grouping cases by failure mode after the fact
- Documenting case origin (`tags = ["source-flywheel"]`)

Don't use it for clustering — that's `cluster_id`'s job.

---

## 6. Inputs

Three input variants today:

```toml
# Chat — single-turn or multi-turn message history.
[cases.input]
kind = "chat"
messages = [
    { role = "system", content = "Be brief." },
    { role = "user",   content = "Hello?" },
]

# Agent — a task to drive plaw with a step budget.
[cases.input]
kind = "agent"
task = "Open the file README.md and summarise its main sections."
max_steps = 5

# RAG — a question and an optional ground-truth doc.
[cases.input]
kind = "rag"
question = "What is the capital of France?"
ground_truth_doc = "Paris is the capital and most populous city of France."
```

The `question_text()` helper in `metrics::runner` extracts the
user-facing prompt for graders. For chat it returns the **last** user
message; for agent the task; for RAG the question. Don't bury context
in earlier turns expecting the grader to see it — it won't.

---

## 7. Expected outputs

Optional grading hints. Use whichever apply, leave the rest empty:

```toml
[cases.expected]
answer = "Paris is the capital of France."        # for free-form graders
answer_keywords = ["Paris"]                       # for keyword_coverage
tool_sequence = ["read_file", "list_dir"]         # for tool-call accuracy
final_state = { file_exists = "/tmp/output.txt" } # for agent_multi_step (M5+)
```

`answer_keywords` is the workhorse — deterministic, multilingual,
robust to phrasing. Add it whenever you can articulate what *must* be
in the response.

---

## 8. Default judge & metric specs

Each suite picks one default judge. The `pairwise` mode is right for
A/B comparison work; `score` works when you need an absolute scale
(rare in practice). For multi-judge cross-family work, declare a
`jury`:

```toml
[default_judge]
model = "kimi-k2.5"
provider = "kimi"
[default_judge.mode]
kind = "jury"
aggregator = "majority"
models = [
    { model = "claude-sonnet-4.5", provider = "anthropic", temperature = 0.0,
      mode = { kind = "pairwise", dual_pass = true } },
    { model = "gpt-4o-mini", provider = "openai", temperature = 0.0,
      mode = { kind = "pairwise", dual_pass = true } },
]
```

Cross-family is mandatory for jury — see [`judge-selection.md`](./judge-selection.md).

Metrics declared on the suite apply to every case:

```toml
[[metrics]]
name = "g_eval"
[metrics.params]
dimension = "answers the question precisely without padding"
scale = 5

[[metrics]]
name = "keyword_coverage"
# No params; uses defaults (case_insensitive=true, whole_word=true).
```

Cases that lack the data a metric needs (e.g. no `answer_keywords` for
`keyword_coverage`) are gracefully skipped — `metrics::runner::compute_metric`
returns `Ok(None)` instead of erroring.

---

## 9. Reviewing your own suite

Before merging a new suite, ask:

- **Does each case fail for an interpretable reason?** Run the suite
  against a deliberately-broken plaw config and check the failure
  modes match what you expected. Cases that fail for "wrong" reasons
  (network errors, parser bugs, irrelevant degradation) are wasting
  sample budget.
- **Do the per-metric scores spread out?** A metric that scores 1.0
  on every case isn't measuring anything. Look at the `quick_summary`
  output and confirm there's variance.
- **Is `n_clusters` right?** If you set `cluster_id`s, run aggregate
  and confirm the auto-detection fired (see `n_clusters` in the JSON
  report).
- **Would you stake an actual gate decision on this?** If the answer
  is "no, the suite is too noisy / too narrow / too easy", iterate
  before merging. A bad suite is worse than no suite — it produces
  false confidence.

---

## 10. Adding a new metric

Most cases should be servable by `g_eval` + `keyword_coverage` +
the structural tool metrics. If you find yourself reaching for a new
metric:

1. Try harder to express the requirement in `g_eval`'s `dimension`
   first. "Specifies a year between 1900 and 2100" is a fine G-Eval
   prompt.
2. If the metric is deterministic (string match, JSON validity, math),
   add it under `crates/plaw-eval/src/metrics/<name>.rs` and wire it
   into `metrics::runner::compute_metric`.
3. If the metric needs a different judge protocol (chain-of-verification,
   self-consistency, multi-rubric DAG), open a design ADR before
   implementing — these are non-trivial and change suite semantics.
