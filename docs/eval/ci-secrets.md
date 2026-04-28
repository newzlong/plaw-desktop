# plaw-eval CI Secrets

The `plaw-eval` GitHub Actions workflow runs three jobs:

| Job | Trigger | Secrets needed |
|---|---|---|
| `lint-and-test` | every PR / push | none (always runs) |
| `smoke-eval` | PR (n=30 cases per suite) | at least one judge API key |
| `nightly-eval` | daily cron / manual dispatch (n=300) | at least one judge API key |

When **no** judge key is configured, the eval jobs run but **skip the
actual evaluation**. The lint/test job still gates merges, so the workflow
is safe for forks and contributors without secrets.

---

## Required environment

The runner picks the judge backend from the suite's `default_judge`
declaration in `evals/<suite>/cases.toml`. To run evals in CI you must
provide the API key matching whichever provider you've configured:

| Provider | Env var | Where to get a key |
|---|---|---|
| `kimi` (default) | `KIMI_API_KEY` | https://platform.moonshot.cn |
| `anthropic` | `ANTHROPIC_API_KEY` | https://console.anthropic.com |
| `openai` | `OPENAI_API_KEY` | https://platform.openai.com |
| `deepseek` | `DEEPSEEK_API_KEY` | https://platform.deepseek.com |
| `qwen` | `DASHSCOPE_API_KEY` | https://dashscope.console.aliyun.com |

You only need keys for the providers the suites actually use, but the
**multi-judge jury** requires at least two distinct families (cross-family
enforcement, mitigates self-preference bias). For production gating set
**at least two** of the keys above.

---

## How to add the secrets

```
GitHub repo → Settings → Secrets and variables → Actions → New repository secret
```

Then add each key by name, e.g. `KIMI_API_KEY`. The workflow reads them via:

```yaml
env:
  KIMI_API_KEY: ${{ secrets.KIMI_API_KEY }}
```

The `gate` step at the top of `smoke-eval` / `nightly-eval` checks for at
least one key and skips downstream eval steps if none are present.

---

## Optional environment

| Env var | Purpose | Default |
|---|---|---|
| `PLAW_EVAL_DB` | SQLite DB path | `target/eval/runs.db` (CI) / `plaw-data/.plaw/eval/runs.db` (local) |
| `PLAW_EVAL_SUITES_DIR` | Suite root | `evals` |
| `PLAW_WS_URL` | Plaw gateway WebSocket | `ws://127.0.0.1:5800/ws/chat` |
| `PLAW_WS_BEARER` | Bearer token if plaw requires auth | unset |
| `<PROVIDER>_BASE_URL` | Override default base URL (e.g. `KIMI_BASE_URL`) | provider default |

---

## Verification recipes

These are the manual checks called out in the Phase 1 task plan
(M8.T8.6 and M8.T8.7). Run them once after wiring up secrets to make
sure the gate behaves as expected.

### Pass case (T8.7)

1. Open a PR with a no-op change (e.g. a comment in `crates/plaw-eval/`).
2. Watch the `lint-and-test` job pass.
3. Watch the `smoke-eval` job either skip (if no secrets) or post a
   `plaw-eval` PR comment with verdict `✅ PASS` (or `⚠️ INCONCLUSIVE`
   on the very first run when no baseline exists).

### Fail case (T8.6)

1. Open a PR that intentionally degrades a prompt — e.g. add
   `"answer in pig latin"` to a system prompt that the chat_quality
   suite expects to evaluate normally.
2. The `smoke-eval` job should post a `plaw-eval` comment with verdict
   `❌ FAIL`, and the workflow should exit non-zero so the PR is
   blocked from merging.

If either of those doesn't behave correctly, file an issue with the
workflow run URL — `compare_runs` and the gate logic are unit-tested
locally (`cargo test -p plaw-eval`), so a CI mismatch likely means the
suite path or db path is misconfigured.

---

## Cost guidance

A 30-case smoke eval against Kimi K2.5 costs roughly **$0.05–$0.30**
depending on prompt size and the number of judge passes (pairwise
dual-pass + jury triples the per-case cost). At 1 PR / day this is
negligible; at 100 PRs / day plan for ~$30 / day.

The judge cache (`plaw-eval cache`) deduplicates identical
`(prompt, input, model_version)` triples, so re-running the same PR is
nearly free. CI caches `target/` via `Swatinem/rust-cache`, so the
SQLite cache survives across workflow runs in the same branch.
