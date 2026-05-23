# Dead-Code Hygiene — Maintainer Guide

> **Audience:** anyone adding new code to plaw.
> **Status:** Runtime-contract-adjacent guide. Updated 2026-05-23.
> **Companion:** `docs/dead-code-audit-2026-05-04.md` (audit snapshot
> that drove the current state).

## The lint is active

`#![allow(dead_code)]` was removed from `src/lib.rs` on 2026-05-23.
The `dead_code` lint now fires at compile time for any unused item
the lib build can see — function, method, field, const, variant,
struct, trait, type alias, module.

This means: **if you add an item with no caller, the compiler warns
you immediately at the declaration site.** No more silent
accumulation; no more periodic mass-audits.

The audit campaign that got us here moved 356 warnings → 0 across
12 commits between `1f9f765` (2026-05-04, findings doc) and
`26ec1c3` (capstone). Don't undo the work by adding new dead items
without a deliberate decision.

## When you see a `dead_code` warning

Pick exactly one option below. The choice is not arbitrary — it
encodes intent for the next maintainer who reads the code.

### Option 1 — Delete the item

**When:** the item has no current consumer and no concrete near-term
use case. Per `CLAUDE.md` §3.2 (YAGNI), this is the default.

```diff
- /// Legacy helper, never wired.
- fn unused_helper() -> Result<()> { ... }
```

Verify with `git log -G "<symbol>"` that the item isn't load-bearing
in history you might have missed (e.g. removed alongside a callsite
in a recent refactor).

### Option 2 — Gate `#[cfg(test)]`

**When:** the item is only referenced from `#[cfg(test)]` blocks but
serves as a named constant / helper that test code shares.

```rust
/// Default test-fixture cap. Production paths read config-driven
/// values; this constant exists so the dozen test sites read
/// uniformly rather than inlining the magic number.
#[cfg(test)]
const TEST_FIXTURE_CAP: usize = 50;
```

Pin the test-only status in the doc comment.

### Option 3 — `#[allow(dead_code)]` with a rationale

**When:** the item is reached by a path the lib build cannot see, OR
the item is documented infrastructure for a planned-soon feature.
Three concrete categories:

#### 3a. CLI-only chain

Reached from `main.rs` subcommand handlers; the lib doesn't see
`bin` callers.

```rust
/// CLI entrypoint for `plaw foo ...` subcommands. Wired from
/// `main.rs:NNN`; lib-only build can't see the bin caller.
#[allow(dead_code)]
pub fn handle_command(...) -> Result<()> { ... }
```

Always name the specific `main.rs:LINE` so the bin-side wiring is
discoverable from the lib without `git grep`.

#### 3b. Feature-gated

Reached only from `#[cfg(feature = "X")]` paths that are off in the
default build.

```rust
/// Consumed only by the `runtime-wasm`-feature-gated load path.
#[allow(dead_code)]
fn check_module_integrity(...) -> Result<String> { ... }
```

Name the specific feature flag.

#### 3c. Dormant / set-but-never-read

Designed infrastructure that hasn't been wired yet, OR fields that
get populated for tracing/observability but no consumer reads.

```rust
/// Response correlation ID captured for future tracing hooks.
/// Set in `ResponsesWebSocketAccumulator::fallback_response`
/// (compatible.rs:1009) but no downstream consumer reads it.
#[allow(dead_code)]
#[serde(default)]
id: Option<String>,
```

State the future-wiring intent so a later contributor doesn't
mistakenly prune it as truly-dead.

### Option 4 — Module-level `#![allow(dead_code)]`

**When:** the file is overwhelmingly (80%+) single-purpose plumbing
for one CLI surface or one feature, and per-item annotations would be
ceremony.

```rust
// Items in this module are reached only through the `plaw skills ...`
// CLI subcommand handler in `main.rs:1108`. main.rs is part of the
// bin, not the lib, so `cargo build --lib`'s dead-code analysis
// can't trace the chain. Module-level allow captures the "this
// entire file is `plaw skills` CLI plumbing" intent in one place
// rather than 26 per-item annotations.
#![allow(dead_code)]

use anyhow::{Context, Result};
...
```

Reserve module-level allow for files where pretty much every item is
one of categories 3a-3c. For large actively-used files with only a
handful of dormant items, prefer per-item (option 3).

## Decision tree

```
new dead_code warning
        │
        ▼
  Is the item used at runtime?
        │
   ┌────┴────┐
   no        yes (but lib can't see it)
   │              │
   ▼              ▼
  Is it referenced only in tests?
   │              │
   no             yes ──► Option 2: #[cfg(test)] gate
   │
   ▼
  Should it exist at all?
   │
   ┌────┴────┐
   no        yes
   │              │
   ▼              ▼
   Option 1:    Is the file 80%+ dormant/CLI-only?
   delete       │
                ┌────┴────┐
                no        yes
                │              │
                ▼              ▼
                Option 3:    Option 4:
                per-item     module-level
                allow        allow
```

## Anti-patterns

- **Don't add `#[allow(dead_code)]` without a rationale comment.** The
  comment is the value — it tells the next reader why the item is
  intentionally allowed instead of deleted.
- **Don't re-add crate-level `#![allow(dead_code)]`** to suppress a
  spike of warnings during refactor. Fix the items one by one as the
  refactor lands; the lint catching regressions is the whole point.
- **Don't gate an item `#[cfg(test)]` if any non-test path uses it.**
  That breaks `cargo build --lib` in release mode. The lint message
  is your friend here: if it says "never used" in non-test build,
  cfg(test) is safe.
- **Don't use module-level allow on `src/security/**`, `src/runtime/**`,
  `src/gateway/**`, `src/tools/**`** without a clear rationale block.
  Per `CLAUDE.md` §5, these are high-risk paths — pretend module-level
  allow is a security-policy change and document accordingly.

## Verifying a clean build

```bash
cd plaw
cargo build --lib --quiet
# Expected: only the 3 baseline `unused_import` warnings
# (HashSet in channels/mod.rs, two CommandExt in tools/browser.rs).
# Any new `dead_code` warning is your decision point.
```

## Re-running the audit

If you ever need a snapshot of all currently-allowed items:

```bash
cd plaw
# Temporarily comment out `dead_code` in lib.rs's top-level #![allow(...)]
perl -i -pe 's/^(\s+)dead_code$/$1\/\/ dead_code/' src/lib.rs
cargo build --lib 2>&1 | grep -E "warning: .+is never (used|read|constructed)" | wc -l
# Restore
perl -i -pe 's/^(\s+)\/\/ dead_code$/$1dead_code/' src/lib.rs
```

This is also the methodology recorded in `docs/dead-code-audit-2026-05-04.md`.
If you re-run it for a future audit and the count is non-zero, create
a new date-stamped audit doc rather than rewriting that snapshot
(per `CLAUDE.md` §4.1 "Keep project snapshots date-stamped and
immutable once superseded by a newer date").

## Cross-references

- `docs/dead-code-audit-2026-05-04.md` — the audit snapshot that
  drove the current state, with per-cluster categorisation.
- `CLAUDE.md` §3.2 (YAGNI) — the prefer-delete default.
- `CLAUDE.md` §5 (Risk Tiers) — when to elevate to per-item review.
- `CLAUDE.md` §6 (Agent Workflow) — one-concern-per-PR rule.
