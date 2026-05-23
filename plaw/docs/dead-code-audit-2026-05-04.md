# Dead-Code Audit — 2026-05-04

> **Status:** Findings snapshot. The audit was resolved 2026-05-23
> (356 → 0 warnings; crate-level `#![allow(dead_code)]` removed).
> See [`dead-code-hygiene.md`](./dead-code-hygiene.md) for the
> ongoing maintainer guide that codifies what to do when the lint
> now fires at compile time.
> **Scope:** Whole `plaw` crate (`cargo build --lib`).
> **Trigger:** Three dead-code findings surfaced during F-4 / F-7 work
> (`PROGRESS_MIN_INTERVAL_MS`, `build_assistant_history_with_tool_calls`,
> `run_tool_call_loop_with_reply_target`) in modules I had already touched.
> Pattern repetition warranted a one-shot whole-crate scan.
> **Method:** Temporarily comment out `dead_code` in `lib.rs`'s
> top-level `#![allow(...)]`, run `cargo build --lib`, capture warnings,
> restore the allow.
> **Outcome:** **356 dead-code warnings** spanning 25+ files. **No
> deletions in this commit.** Per-cluster investigation deferred to
> follow-up PRs.

This document is a **snapshot of findings**. Each item below requires
investigation before deletion — many `dead_code` warnings in this
crate are false-positive in the sense that:

1. Some items are reachable only through `bin/plaw` CLI subcommands;
   `cargo build --lib` doesn't see those callers.
2. Some are feature-gated and the default build doesn't activate them.
3. Some are part of the **public** crate surface used by downstream
   consumers (workspace crates, integration tests, future bin
   handlers).
4. Some are **genuinely** dead — declared once, never wired.

Only category 4 is safe to delete without further inspection.

## Top-line

```text
356 total dead-code warnings (cargo build --lib, 2026-05-04, branch main)
```

## Per-file clusters

Top 25 files by warning count (`grep -A1 "is never " | sed file-extract | sort | uniq -c`):

| count | file | likely category |
|------:|------|-----------------|
| 84 | `src/onboard/wizard.rs` | CLI-only (interactive setup) |
| 42 | `src/service/mod.rs` | CLI-only (service install/uninstall) |
| 41 | `src/skills/mod.rs` | mixed (public API + helpers) |
| 19 | `src/migration.rs` | CLI-only (first-launch path) |
| 14 | `src/auth/gemini_oauth.rs` | feature-gated (Google auth) |
| 12 | `src/auth/openai_oauth.rs` | feature-gated (OpenAI auth) |
| 10 | `src/security/otp.rs` | feature-gated (2FA) |
|  9 | `src/doctor/mod.rs` | CLI-only (`plaw doctor`) |
|  9 | `src/daemon/mod.rs` | CLI-only (background mode) |
|  9 | `src/channels/mod.rs` | mixed |
|  8 | `src/security/estop.rs` | feature-gated (e-stop hardware) |
|  7 | `src/skills/clawhub.rs` | feature-gated (skill registry) |
|  7 | `src/integrations/mod.rs` | feature-gated (composio etc.) |
|  6 | `src/auth/oauth_common.rs` | feature-gated |
|  5 | `src/hardware/mod.rs` | feature-gated (peripherals) |
|  5 | `src/cron/consolidation.rs` | mixed |
|  4 | `src/security/domain_matcher.rs` | helper |
|  4 | `src/hardware/registry.rs` | feature-gated |
|  3 | `src/security/audit.rs` | helper |
|  3 | `src/providers/compatible.rs` | provider helper |
|  3 | `src/providers/bedrock.rs` | feature-gated (AWS Bedrock) |
|  3 | `src/providers/anthropic.rs` | provider helper |
|  3 | `src/config/schema.rs` | likely public API |
|  2 | `src/tools/composio.rs` | feature-gated |
|  2 | `src/tools/browser.rs` | mixed |

Remaining clusters (≤ 2 warnings each) account for the rest.

## Categorisation heuristics

For each cluster, the recommended check before deletion:

- **CLI-only candidates** (`onboard/wizard.rs`, `service/mod.rs`,
  `migration.rs`, `doctor/mod.rs`, `daemon/mod.rs`): grep
  `src/main.rs` and `src/bin/` for the symbol; if used by a
  subcommand handler, keep. Otherwise re-evaluate.

- **Feature-gated candidates** (`auth/*`, `security/otp.rs`,
  `security/estop.rs`, `hardware/*`, `providers/bedrock.rs`,
  `tools/composio.rs`, `integrations/*`): check `Cargo.toml`
  `[features]` table and any `#[cfg(feature = "...")]` gates near
  the symbol. If feature-gated and the feature is genuinely off in
  default build, the warning is expected — `#[cfg(feature = "...")]`
  on the gate or `#[allow(dead_code)]` on the item is a clean fix
  (the latter is what `lib.rs` does crate-wide today).

- **Public API candidates** (`skills/mod.rs`, `config/schema.rs`,
  `channels/mod.rs`): grep workspace siblings (`crates/plaw-eval`,
  `bin/`, examples) for the symbol; if it's part of the published
  surface, keep. Otherwise re-evaluate.

- **Mixed clusters**: file-by-file inspection. The 9 warnings in
  `channels/mod.rs` and the 5 in `cron/consolidation.rs` likely
  contain a mix of all three categories above.

## Already-deleted findings (this session)

For reference, the three findings that triggered this audit and were
deleted in their respective commits:

| symbol | file | commit | size |
|--------|------|--------|------|
| `PROGRESS_MIN_INTERVAL_MS` | `agent/loop_.rs` | `bd9ec2d` | 1 line |
| `build_assistant_history_with_tool_calls` | `agent/loop_.rs` | `1bfbd63` | 19 lines |
| `run_tool_call_loop_with_reply_target` | `agent/loop_.rs` | `f3cd8b8` | 47 lines |

Each was confirmed dead by `git log -G "<symbol>"` returning only the
initial commit (i.e. declared at repo bootstrap, never invoked since).

## Recommended follow-up

1. **No bulk deletion.** A blanket `cargo fix --bin plaw` style mass
   removal is unsafe given the CLI-only and feature-gated populations.

2. **One PR per cluster.** Smallest blast radius is per-file deletion,
   gated by per-symbol audit.

3. **Order of attack** (low-risk first):
   - **`agent/loop_/*`** — the area we've been actively maintaining.
     Already at near-zero dead code after the three deletions; any
     remaining (`tools_to_openai_format`, `MAX_PROGRESS_LINES`,
     `progress_log` field, `DEFAULT_MAX_HISTORY_MESSAGES`) needs the
     test-only-vs-genuinely-dead distinction made per item.
   - **`channels/mod.rs`** — 9 warnings, mixed. Worth a focused PR.
   - **Provider helpers** (`compatible.rs`, `anthropic.rs` — 3 each):
     small, isolated, low-risk.
   - **CLI-only modules**: defer until someone touches them; the
     warning is suppressed by the crate-level allow anyway, so cost
     of carrying is just the memory in `cargo build --bin plaw`'s
     symbol table.

4. **Don't lift `#![allow(dead_code)]` permanently** until at least
   the CLI-only and feature-gated populations are resolved — flipping
   it would break the build instantly with 356 warnings.

5. **Future hygiene**: when adding new modules, write the call site
   in the same PR as the helper. The 3 deletions this session were
   helpers added at initial commit with the wiring presumably planned
   "for later" and never landed.

## Audit reproduction

```bash
# 1. Comment out dead_code in lib.rs's top-level #![allow(...)]
# 2. Run:
cargo build --lib 2>&1 | grep "is never " | wc -l
# Expected: 356 (as of 2026-05-04 commit f3cd8b8)
# 3. Restore the allow.
```

Per-file count via:

```bash
cargo build --lib 2>&1 | grep -A1 "is never " | grep -E "src.+\.rs" \
    | sed 's/.*--> //' | sed 's/:[0-9]*:[0-9]*$//' | sort | uniq -c | sort -rn
```
