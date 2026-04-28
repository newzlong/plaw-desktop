# `agent_multi_step` — multi-step agent tasks

## Testing thesis

Single-tool selection (covered by `tool_routing`) is necessary but not
sufficient. Real agent failures show up across **sequences of decisions**:
the model picks the right first tool, gets back data it didn't plan
for, and then either adapts cleanly or loses the plot.

This suite measures whether plaw can hold a plan across 3-10 tool calls
with a checkable end-state — not "did it sound competent" but "did the
filesystem / state actually end up in the asked-for shape".

## What we measure

- **Task success** — boolean per case, judged by `final_state` matching
- **Step count** — fewer is better (no padding with unnecessary calls)
- **Plan stability** — did the agent thrash between tools?

The metric stack here intentionally avoids G-Eval. We don't care if
the running commentary is eloquent; we care if the task completed.

## Capability dimensions

| Dimension | Why we test | Seed | Designed target |
|-----------|------------|------|----------------|
| **Simple 2-3 step** | Easy baseline | 2 | 6 |
| **Medium 4-6 step** | Realistic CRUD-style task | 1 | 10 |
| **Complex 7+ step** | Stress test of plan stability | 0 | 6 |
| **Branching** | Decision based on intermediate result | 1 | 4 |
| **Resumable / idempotent** | Verifies agent doesn't double-do work | 1 | 4 |

## What the seeds *don't* cover

- Tasks needing real network state (covered by `tool_routing` smoke only)
- Genuinely complex 7+ step workflows (require careful `final_state`
  authoring; punted to designed pass)
- Tasks where multiple correct paths exist — `final_state` matching is
  too brittle when there are 3 ways to get there. Designed cases need
  to pick narrow tasks where there's *one* obvious shape of done.

## Design checklist

- [ ] At least 30 cases tagged `designed`
- [ ] Each difficulty bucket meets its case count target
- [ ] Every case has either `final_state` JSON OR `tool_sequence` set
  (preferably both)
- [ ] No case relies on real-world side effects beyond a sandboxed temp
  directory
- [ ] At least 3 branching cases test "intermediate result drives next decision"

## Judge configuration

Score-mode is informational here. The actual gate metric is success rate
based on `final_state` matching, computed by the runner not the judge.

## How to extend

```toml
[[cases]]
id = "agent_multi_step-<dim>-<NNN>"
tags = ["designed"]
# Why: Tests <plan stability / branching / resumability> for <scenario>
[cases.input]
kind = "agent"
task = "..."
max_steps = 8
[cases.expected]
tool_sequence = ["list_dir", "read_file", "edit_file", "shell"]
# Or specify the end state directly:
# final_state = { file_exists = "./out.txt", line_count_gte = 5 }
```

## Caveat about `final_state`

The current schema only supports a free-form JSON object. The runner
doesn't yet know how to *check* it (the comparison is left to the
judge). Designed cases should phrase final_state as the judge would
need to evaluate it — see [docs/eval/suite-design.md](../../docs/eval/suite-design.md)
for examples.
