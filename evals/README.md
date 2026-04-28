# `evals/` — Plaw Eval Suites

Each subdirectory here is a **suite**: a TOML file + a README explaining
what the suite is testing and why those cases were chosen. `plaw-eval`
loads every suite under this directory.

## Conventions

| Concept | Convention |
|---------|-----------|
| Suite directory | `evals/<suite_name>/` (`snake_case`) |
| Cases file | `cases.toml` (the only required file) |
| Suite docs | `README.md` (intent, sourcing, extension hints) |
| Case ids | `<suite>-<short-slug>-<NNN>` (e.g. `chat_quality-math-001`) |
| Tags | At minimum one of `seed`, `designed`, `flywheel` |

### Tag semantics

- `seed` — engineered placeholder, included so the suite is runnable. Don't gate on these.
- `designed` — chosen on purpose by a human with stated rationale in the suite README. Gate on these.
- `flywheel` — promoted from a production trace via `plaw-eval flywheel promote`. Always gate.
- `smoke` — must finish in < 5s. Used by the `--n 30` PR smoke job.

### Cluster ids

Set `cluster_id` whenever cases share a structural correlation:

- Multi-turn dialogues from the same conversation
- Multiple cases derived from the same source document
- Cases that paraphrase the same underlying question

Without a cluster id, the runner treats every case as independent and
the standard error is too narrow when correlations exist. See
[../docs/eval/suite-design.md](../docs/eval/suite-design.md).

## Suites

| Suite | What it tests | Status |
|-------|--------------|--------|
| `chat_quality` | General single-turn chat quality (factual, math, writing) | Seeds only — needs designed cases |
| `tool_routing` | Agent picks the correct tool for a stated intent | Seeds only — needs designed cases |
| `rag_grounded_qa` | RAG faithfulness & answer relevancy on a fixed corpus | Stubbed — corpus + cases TBD |
| `agent_multi_step` | Multi-step agent tasks with checkable end-state | Seeds only — needs designed cases |
| `error_recovery` | Recovery from injected tool failures / bad outputs | Stubbed — needs fault-injection runtime |

Each suite's README explains its specific testing thesis, what the seeds
*don't* cover, and a checklist for filling in the designed cases.

## Extending a suite

1. Read the suite's `README.md` for its testing thesis.
2. Pick a slot from its "design checklist" that's still unfilled.
3. Add a `[[cases]]` block with `tags = ["designed"]` and a comment
   explaining *why this specific case earned a slot* (one line).
4. Set `cluster_id` if the case shares structure with existing cases.
5. Run `cargo run -p plaw-eval-cli -- list --detail` — the suite must load.
6. Commit with `feat(evals/<suite>): add <topic> case`.

## Authoring philosophy

Eval cases aren't "bug repros" or "user requests we got". They're
deliberate probes for specific failure modes. Three rules:

1. **Each case earns its slot.** If you can't say in one sentence why a
   case is here, delete it. Crowded suites hide regressions.
2. **Distribute, don't pile.** Don't add 10 math cases to
   `chat_quality` — pick 2 hard ones and use the slots on other
   capabilities. Diversity > volume.
3. **Adversarial > friendly.** A suite of "ideal user" cases gives a
   model 95% with a tailwind. Mix in cases where the model has to *not*
   take the obvious wrong path.

See [../docs/eval/suite-design.md](../docs/eval/suite-design.md) for the
methodology these rules are derived from.
