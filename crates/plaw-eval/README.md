# plaw-eval

Statistical evaluation foundation for the [plaw](../../plaw) AI agent
runtime. Phase 1 of the **plaw-elite** initiative — full design lives
in [`.kiro/specs/plaw-elite/`](../../.kiro/specs/plaw-elite/).

The library provides:

- Anthropic-grade statistical primitives (95% CI, paired-difference
  analysis, cluster-robust SE, Bradley-Terry MLE).
- A SQLite-backed eval database (suites, runs, case results, judge
  cache, flywheel queue).
- A WebSocket client for driving plaw, with bounded concurrency,
  retries, and Ctrl-C cancellation.
- Multi-judge LLM-as-Judge clients (Anthropic Messages, OpenAI-compat,
  Kimi, DeepSeek, Qwen) with mandatory dual-pass position swap and
  cross-family jury enforcement.
- Quality metrics: G-Eval, keyword coverage, tool-call accuracy.
- Aggregation + run-vs-run gate logic with paired CI and PR-comment
  rendering.

The companion [`plaw-eval-cli`](../plaw-eval-cli) crate exposes the
whole stack as a `plaw-eval` binary.

---

## Quick start

```bash
# Build the CLI
cargo build --release -p plaw-eval-cli

# Set the API key for whichever judge you'll use (see docs/eval/ci-secrets.md
# for the full list).
export KIMI_API_KEY=...

# Sanity-check the environment (DB path, suites dir, API keys, plaw URL).
./target/release/plaw-eval doctor

# Run all suites under ./evals against a plaw on ws://127.0.0.1:5800/ws/chat.
./target/release/plaw-eval run --all --n 30

# List the runs you've accumulated.
./target/release/plaw-eval list

# Compare the latest run against the previous one and emit a PR comment.
./target/release/plaw-eval compare --baseline latest --candidate latest \
    --pr-comment /tmp/comment.md --output /tmp/diff.json
```

For the rationale behind every step, read
[`docs/eval/methodology.md`](../../docs/eval/methodology.md). Don't
skip it — the gate logic only makes sense if you understand the
statistical setup.

---

## Library overview

```rust
use plaw_eval::stats::{t_distribution_ci, paired_difference};
use plaw_eval::storage::EvalRepo;
use plaw_eval::suite::load_suite;
use plaw_eval::runner::{execute, RunnerConfig, PlawClient, aggregate};
use plaw_eval::report::{compare_runs_default, render_pr_comment, extract_failing_rows};
```

Module map:

| Module | Purpose |
|---|---|
| `stats` | Pure statistics: CI, cluster SE, paired diff, B-T MLE, power. |
| `suite` | Suite types + TOML loader. |
| `storage` | SQLite repo for runs, case results, judge cache, flywheel. |
| `runner` | Plaw WS client, bounded-concurrency executor, judge cache. |
| `judges` | Judge clients + dual-pass pairwise + cross-family jury. |
| `metrics` | G-Eval / keyword coverage / tool accuracy + scoring runner. |
| `report` | Aggregation, gate verdict, JSON / Markdown / PR-comment renderers. |
| `flywheel` | Production-trace sampling + review + promotion. |

All public types implement `Debug` and most relevant ones implement
`Serialize`/`Deserialize`. Round-tripping aggregates through JSON for
artifact storage is a first-class flow.

---

## Reading the source

The plaw-eval codebase is small (~6.5 KLOC + ~1100 lines of tests)
and intentionally flat — a competent Rust reader can map the whole
library in 30–60 minutes. Suggested order:

1. `lib.rs` — module shape.
2. `stats/ci.rs` — t-CI, Wilson, bootstrap. Trivial to verify against
   any stats reference.
3. `stats/paired.rs` — the heart of the gate logic.
4. `suite/case.rs` — what a case actually is.
5. `runner/executor.rs` — the run loop.
6. `report/gate.rs` — Pass/Fail/Inconclusive decision.
7. `metrics/runner.rs` + `metrics/g_eval.rs` — scoring.
8. `judges/jury.rs` — cross-family enforcement, aggregation.

Tests live next to their implementations (`#[cfg(test)] mod tests`)
plus three integration tests under [`tests/`](tests/) covering the
M2/M3/M6 acceptance criteria.

---

## Documentation

| Document | Audience | What it covers |
|---|---|---|
| [`docs/eval/methodology.md`](../../docs/eval/methodology.md) | All readers | Why every metric is computed the way it is. The rationale doc. |
| [`docs/eval/suite-design.md`](../../docs/eval/suite-design.md) | Suite authors | What makes a good case, when to use `cluster_id`, sample-size guidance. |
| [`docs/eval/judge-selection.md`](../../docs/eval/judge-selection.md) | Suite authors | Picking judges, jury composition, bias mitigations. |
| [`docs/eval/troubleshooting.md`](../../docs/eval/troubleshooting.md) | Operators | Common errors and how to diagnose. |
| [`docs/eval/ci-secrets.md`](../../docs/eval/ci-secrets.md) | CI maintainers | GitHub Actions secrets, cost guidance, verification recipes. |

---

## Running the test suite

```bash
cargo test -p plaw-eval                   # all unit + integration tests
cargo test -p plaw-eval --lib             # unit only
cargo test -p plaw-eval --test m6_aggregate_compare_integration  # one integration suite
```

The whole suite takes well under a second on a 2024-era laptop. There
are no test fixtures that talk to live LLM endpoints — every judge in
test code is a `MockJudgeClient`, every plaw is a tokio mock WS
server.

CI runs the lint+test pipeline on every PR; see
[`.github/workflows/plaw-eval.yml`](../../.github/workflows/plaw-eval.yml).

---

## Status

Phase 1 milestones (see [`.kiro/specs/plaw-elite/phase-1-eval/tasks.md`](../../.kiro/specs/plaw-elite/phase-1-eval/tasks.md)):

- [x] M0 Scaffolding
- [x] M1 Stats library
- [x] M2 Suite & Storage
- [x] M3 Plaw Client + Runner
- [x] M4 Judges (pairwise + jury, cross-family)
- [x] M5 Metrics (G-Eval, tool, keywords; secondary metrics deferred)
- [x] M6 Aggregation + Reports
- [x] M7 CLI wired
- [x] M8 CI workflow + secrets doc
- [ ] M9 Eval suites (5 × 30+ cases)
- [ ] M10 Flywheel end-to-end
- [x] M11 Documentation (this set)
- [ ] M12 Coverage verification + tag

Deferred: scipy cross-check fixtures, tarpaulin coverage report,
secondary metrics (faithfulness, plan quality, repeatability,
error-recovery), SARIF output, shell completion.
