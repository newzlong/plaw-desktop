# `chat_quality` — General single-turn chat

## Testing thesis

Plaw is a general-purpose AI agent. Before testing tool-using or
multi-step behaviour, we need to know it can hold a competent
single-turn conversation: answer factual questions, perform basic
reasoning, follow simple writing instructions, and decline gracefully
when it doesn't know.

This suite is the first line of defense — if `chat_quality` regresses,
something in the prompt / model / context layer broke regardless of
which Phase 2 subsystem you're working on.

## Capability dimensions

The full suite (M9 target: 30+ designed cases) should cover these
dimensions roughly evenly. Current seed coverage in parens.

| Dimension | Why we test | Seed count | Designed target |
|-----------|------------|------------|----------------|
| **Factual recall** | Hallucination canary on well-known facts | 2 | 5 |
| **Arithmetic / logic** | Cheap signal on basic reasoning | 2 | 5 |
| **Following constraints** | "in one sentence", "use bullets" | 1 | 5 |
| **Honest "I don't know"** | Calibration when info isn't available | 1 | 5 |
| **Style / register** | Formal vs casual, professional tone | 1 | 4 |
| **Concision** | No unsolicited preamble or apology | 1 | 3 |
| **Refusal handling** | Decline harmful requests w/o moralising | 0 | 3 |

## What the seeds *don't* cover

- Long-form generation (≥ 300 tokens output)
- Multi-turn coherence (this suite is single-turn by design — see `agent_multi_step` for multi-turn)
- Code generation quality (out of scope; adversarial code lives in its own future suite)
- Non-English (Chinese cases TBD; current seeds are English-only)

## Design checklist

Before this suite is "done":

- [ ] At least 30 cases tagged `designed`
- [ ] Each capability dimension has its target case count met
- [ ] At least 3 cases are deliberately adversarial (e.g. famous misconceptions)
- [ ] At least 2 cases require the model to refuse a leading prompt
- [ ] Spread of difficulty: 30% easy / 50% medium / 20% hard
- [ ] All `cluster_id`s reviewed — paraphrase pairs share clusters

## Judge configuration

Pairwise dual-pass against the previous run, with `g_eval` as the
metric. Position-bias mitigation is mandatory.

## How to extend

```toml
# Pick an under-filled dimension from the table above
[[cases]]
id = "chat_quality-<dim>-<NNN>"
tags = ["designed"]
# Why this case earns a slot (one line):
# Tests <specific failure mode> — sourced from <where>
[cases.input]
kind = "chat"
messages = [{ role = "user", content = "..." }]
[cases.expected]
answer_keywords = ["..."]
# answer = "..."  # optional reference for G-Eval
```
