# plaw-eval Troubleshooting

Common issues and how to fix them. Updated as new sharp edges are
found in the wild.

---

## CLI

### `plaw-eval doctor` says "missing" for every API key

Set the env var the doctor printed (e.g. `KIMI_API_KEY`) and re-run.
For CI the keys live in repo secrets — see [`ci-secrets.md`](./ci-secrets.md).

If you've set the env var but doctor still reports missing:

- Check the export actually persists in the shell session
  (`echo $KIMI_API_KEY` should print it).
- On Windows PowerShell, `$env:KIMI_API_KEY = "..."` is per-session.
  Persist via `[Environment]::SetEnvironmentVariable("KIMI_API_KEY", "...", "User")`.

### `plaw-eval run` exits with `either --suite <name> or --all is required`

You ran `plaw-eval run` without selecting any suite. Either pass
`--suite chat_quality` or `--all` to discover everything under
`evals/`.

### `plaw-eval run --all` finds no suites

Check `--suites-dir` (default: `./evals`). Each suite must live in a
sub-directory containing a `cases.toml` — `evals/foo/cases.toml`
works, `evals/foo.toml` does not.

```bash
plaw-eval --suites-dir /path/to/evals doctor
# look for "suites directory : ... ok (N suite(s))"
```

### `plaw-eval list` says `(no runs)` after running an eval

The `--db` path likely differs between the run and the list. The CLI
defaults to `plaw-data/.plaw/eval/runs.db` *under the current
directory*. If you `cd` between commands, set `--db` or
`PLAW_EVAL_DB` to a stable absolute path.

### `plaw-eval compare` exits with non-zero on success

That's intentional — when the gate verdict is `Fail`, the CLI exits
with code 1 so CI pipelines block the merge. If you want a silent
failure-mode for ad-hoc inspection, redirect or check `verdict` in
the JSON report (`--output diff.json`) instead of relying on exit
code.

---

## Runs against a real plaw

### Every case fails with `plaw response timed out`

Plaw isn't actually listening on the URL the runner is dialling.
Check:

- `plaw-eval --ws-url ws://...` matches plaw's `gateway` port.
- Plaw is in foreground/started — `curl http://127.0.0.1:5800/health`
  (whatever plaw exposes) confirms it.
- Bearer token (`--ws-bearer`) is set if plaw's gateway requires one.

### Every case fails with `plaw error: ...`

Plaw is running but rejecting the requests. Look at plaw's own log
output — the error message embedded in the WS frame is the same one
plaw would print to its console / file log. Common root causes:

- Plaw config is missing the upstream provider API key.
- Plaw's tools list rejected the request shape (less likely from a
  bare chat input, common when the eval's `expected.tool_sequence`
  references tools plaw hasn't enabled).

### Mixed results: some cases succeed, some fail

This is the runner working as designed — failures are isolated and
recorded as `error` rows without poisoning the run. Read
`plaw-eval list --detail --limit 1` to see the per-metric breakdown
and look at the failure messages in the case_results table:

```bash
sqlite3 plaw-data/.plaw/eval/runs.db \
    "SELECT case_id, error FROM case_results
     WHERE run_id = '<id>' AND error IS NOT NULL"
```

---

## Judges

### High `PositionInconsistent` rate (>20%)

Indicates the judge has a strong position bias on this suite. Two
fixes:

1. **Switch to a stricter judge model** — Anthropic Claude is
   typically lower position-bias than smaller models. Try
   `claude-sonnet-4.5` for higher-stakes work.
2. **Sharpen the rubric** in G-Eval's `dimension`. Vague rubrics
   make the judge fall back on heuristics like "the first answer
   is usually the better one". Specific criteria force evaluation
   on content.

If neither helps, the case itself is probably ambiguous. See
[`suite-design.md`](./suite-design.md) §9.

### Jury is always Inconclusive

You're running a 2-judge jury and the two judges always disagree.
Either:

- Pick a 3rd judge to break ties (recommended).
- Switch the aggregator to `confidence_weighted` (less conservative
  but more verdicts).
- Look at the per-judge raw output in `metric_scores.raw` — if one
  judge is clearly malfunctioning (e.g. always replying with prose
  instead of `[[1]]/[[2]]/[[T]]`), pull that judge from the jury.

### `judge HTTP 429: rate limit`

The judge endpoint is throttling. `plaw-eval` does not currently
implement client-side rate limiting (M3 has bounded retry but no
adaptive backoff). Mitigations:

- Drop `--n` or split the suite into multiple smaller runs.
- Upgrade your provider tier.
- Set the judge cache up explicitly so reruns are free
  (`plaw-eval cache stats` to verify it's accumulating hits).

### `unknown judge provider 'foo'`

The provider field in the suite TOML doesn't match a known mapping.
See [`judges/builder.rs`](../../crates/plaw-eval/src/judges/builder.rs)
— supported are `anthropic`, `openai`, `kimi`, `deepseek`, `qwen`
(plus their case-insensitive aliases). Add your provider there if
needed.

---

## Statistics & reports

### CI says "Inconclusive: metric missing from one side"

The metric exists on the candidate (or the baseline) but not on the
other side. Causes:

- You changed `[[metrics]]` between the baseline and candidate. Run
  the baseline again with the new metrics so they're computed there
  too.
- The metric was implemented after the baseline ran. Either re-score
  the baseline (`metrics::runner::score_run`) or wait for the next
  full nightly to refresh it.

### Cluster SE shows `—` for a metric you set `cluster_id` on

The auto-engagement heuristic decided your clusters are too sparse.
The threshold is `n_clusters * 5 < n` — i.e. average 5+ cases per
cluster. If you have 30 cases in 10 clusters (avg 3 per cluster),
clustering doesn't fire. Either consolidate clusters or accept that
naive SE is fine at that density.

### Numbers in the JSON report differ from the CLI Markdown output

They shouldn't. If they do, you're looking at different runs (CLI
reaggregated since you saved the JSON). Always trust the JSON file
that the run produced — the Markdown is a derived view.

### Paired diff is `None` even though both runs hit the same suite

The runs don't share case IDs. Check:

- Did the suite gain or lose cases between baseline and candidate?
  The runner only pairs cases present in **both** runs.
- Did `--n` sample different subsets? If so, run with `--seed 42`
  consistently across baseline and candidate so the same cases get
  picked.

---

## Database & cache

### SQLite says `database is locked`

You have two `plaw-eval` processes hitting the same DB at once. The
runner uses a single `Mutex<Connection>` per process; cross-process
locking would need WAL mode and long-running connections. For now:
serialize CLI invocations against one DB, or point each invocation at
a unique `--db` path.

### `cargo build` complains about `rusqlite` C compiler

`rusqlite` bundles SQLite's C source via the `bundled` feature, so
you need a C toolchain. On Ubuntu CI runners that means installing
`build-essential` (already present on `ubuntu-latest`). On Windows
local dev: install Visual Studio Build Tools or use `cargo build`
inside Git Bash with the MSYS2 toolchain.

### `judge_cache` table grows huge

Run `plaw-eval cache clear --ttl-days 30` periodically. The default
TTL is 0 (clear everything older than the current second), so:

```bash
plaw-eval cache clear --ttl-days 30   # keep last 30 days
```

The cache key includes model version, so a vendor model upgrade
already invalidates affected entries. Manual clearing is mostly for
disk-pressure reasons.

---

## CI specifics

### `lint-and-test` job fails on `cargo fmt --check`

Locally run `cargo fmt --all` and commit the result. The CI uses
`-- --check`, so any formatting diff fails — the autofix isn't
applied in CI.

### `lint-and-test` fails on `clippy -D warnings` after a Rust
toolchain upgrade

Newer rust versions ship newer clippy, which sometimes adds new
warnings. Run `cargo clippy -- -D warnings` locally on the same
toolchain and either fix the new lint or `#[allow(...)]` it with a
comment explaining why.

### Smoke-eval job skips with "no judge API keys configured"

This is the intended fallback when running on a fork or before
secrets are configured. Add at least one of `KIMI_API_KEY`,
`ANTHROPIC_API_KEY`, `OPENAI_API_KEY` in repo secrets. See
[`ci-secrets.md`](./ci-secrets.md).

### PR comment doesn't update on subsequent pushes

Make sure the workflow's `permissions` block includes
`pull-requests: write`. The marocchino sticky-comment action also
requires `header: plaw-eval` to match between runs — don't change
that string.

---

## Reporting bugs / asking for help

When opening an issue, include:

1. `plaw-eval --version` output
2. The output of `plaw-eval doctor`
3. The minimal `cases.toml` that reproduces the bug
4. The full CLI invocation
5. The run ID(s) that exhibit the issue (so we can correlate with
   any uploaded artifact)

If the bug is statistical (numbers look wrong), include the JSON
report and a description of what you expected the numbers to look
like — "wrong" without a target is hard to debug.
