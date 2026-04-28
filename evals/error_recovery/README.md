# `error_recovery` — fault-injection recovery

> **Status: stubbed.** This suite needs a fault-injection harness that
> doesn't exist yet. Don't gate on it. The README below documents the
> design so it can be filled in when plaw exposes the required hooks.

## Testing thesis

Production agents fail constantly: tools time out, return malformed
output, hit permission errors, encounter rate limits, lose network. A
"smart" answer is worthless if the agent gives up the moment something
breaks.

This suite probes recovery behaviour by **deliberately injecting faults**
into the tool layer and measuring whether the agent (a) recognises the
failure, (b) tries an appropriate workaround, (c) reports honestly when
no recovery is possible.

## Why it's stubbed

We need three things from plaw before this suite is real:

1. **Fault-injection middleware.** A way to wrap a tool's execution so
   that some calls return synthetic errors. Cleanest implementation:
   plaw exposes a `tool_middleware` config block; the eval harness
   installs a middleware that consults a per-case fault map.
2. **Failure-mode taxonomy.** Specific error shapes the agent should
   recognise — timeout / permission / not_found / rate_limited /
   bad_output — emitted in a stable format by every tool.
3. **Recovery oracle.** A way to say "the agent recovered iff X". Could
   be: "agent's final response acknowledges the failure" + "task
   completion via alternative path within step budget".

## Failure modes we want cases for

| Mode | What's injected | Expected recovery |
|------|----------------|-------------------|
| `timeout` | Tool hangs past its budget | Retry once, then degrade or report |
| `permission_denied` | Read fails with EACCES | Don't retry; report cleanly |
| `not_found` | File/URL doesn't exist | Try alternative path; report if no fallback |
| `bad_output` | Tool returns malformed JSON / truncated text | Detect, retry once, fall back |
| `rate_limited` | Tool returns 429 | Backoff + retry |
| `network_error` | Tool returns connection-refused | Same as timeout |

## Capability dimensions

| Dimension | Why we test | Designed target |
|-----------|------------|----------------|
| **Recognise the failure** | Agent acknowledges in next message | 6 |
| **Try a workaround** | Picks an alternative tool / path | 8 |
| **Honest fallback** | Reports when no recovery is possible | 6 |
| **No infinite retry** | Doesn't burn step budget on dead tool | 4 |
| **Don't pretend to succeed** | Critical: no fabricated success | 6 |

## Design checklist

- [ ] Fault-injection middleware shipped in plaw
- [ ] Tool error envelope standardised across all tools
- [ ] At least 30 cases, one per failure-mode × recovery-strategy combo
- [ ] At least 6 "no recovery is correct" cases — agent must NOT pretend
- [ ] Cases for cascading failures (first workaround also fails)

## Stub behaviour

No `cases.toml` ships in this suite yet. Adding one before the
fault-injection harness exists would only test that plaw fails on
missing files, which `tool_routing` already covers. When the harness
lands, this directory gets populated.
