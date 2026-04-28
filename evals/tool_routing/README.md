# `tool_routing` — Agent picks the correct tool

## Testing thesis

Plaw's value over a plain chat model is its tool ecosystem. A correct
final answer matters less here than *which tool the agent reaches for*.
This suite probes the **selection** decision and the **arguments** the
agent passes — not whether the eventual response reads well.

## What we measure

The metric stack is fundamentally different from `chat_quality`:

- `tool_call_accuracy` (selection F1, arg validity, redundant-call rate)
- **No** G-Eval — final response quality is intentionally out of scope here

## Plaw's tool surface (as of 2026-04)

From [CLAUDE.md](../../CLAUDE.md). Each case's `expected.tool_sequence`
must come from this set:

| Tool | Expected use |
|------|-------------|
| `shell` | Run shell commands |
| `read_file` | Read a file's contents |
| `write_file` | Create or overwrite a file |
| `edit_file` | Modify an existing file in place |
| `list_dir` | List directory contents |
| `search` | Grep across files |
| `web_search` | Search the web (Bing RSS) |
| `web_fetch` | Fetch and convert a URL to Markdown |
| `http_request` | Generic HTTP call (allow-listed domains) |
| `browser_navigate` | Drive a headless browser |
| `browser_click` | Click on a browser element |

## Capability dimensions

| Dimension | Why we test | Seed | Designed target |
|-----------|------------|------|----------------|
| **Single-tool obvious** | Easy baseline; should be ~100% | 3 | 5 |
| **Single-tool ambiguous** | `search` vs `web_search` style choices | 1 | 6 |
| **Multi-tool sequencing** | Order matters, e.g. read-then-edit | 1 | 6 |
| **Refuse to use a tool** | Some asks should NOT trigger any tool | 1 | 4 |
| **Argument extraction** | Right tool, but does it pass right args? | 1 | 5 |
| **Redundant-call avoidance** | Don't call same tool twice with same args | 0 | 3 |
| **Tool unavailable** | Graceful when a needed tool isn't allow-listed | 0 | 3 |

## What the seeds *don't* cover

- Browser automation (single seed only — needs real fixture pages)
- Cases that depend on the actual filesystem or network state at eval time
- Adversarial argument injection (e.g. `..` in path)

## Design checklist

- [ ] At least 30 cases tagged `designed`
- [ ] Each tool from the surface table has ≥ 2 cases
- [ ] At least 5 cases require multi-tool sequencing
- [ ] At least 3 "no tool needed" cases
- [ ] Argument validity is the failure mode for ≥ 5 cases (right tool, wrong args)

## Judge configuration

Score-mode judge on 1-5 scale, but the gate metric is `tool_call_accuracy`,
not the judge score. Judge feedback is informational.

## How to extend

```toml
[[cases]]
id = "tool_routing-<dim>-<NNN>"
tags = ["designed"]
# Why: Tests <selection / args / sequencing> for <tool>
[cases.input]
kind = "agent"
task = "..."
max_steps = 3   # Keep small — this suite tests the FIRST few decisions
[cases.expected]
tool_sequence = ["read_file", "edit_file"]
```
