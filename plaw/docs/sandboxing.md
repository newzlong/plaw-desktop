# Plaw Sandboxing — Status & Roadmap

> **Status: Phase 0 + Phase 1 shipped**. Phase 1c.2 in design.
> Last updated: 2026-06-07.
>
> For operator-facing config, see [`config-reference.md`](config-reference.md)
> → `[security.sandbox]` + `[security.sandbox.integrity]`.
> For runtime inspection, see `plaw sandbox status` (PR #82).
> For source code, see `src/security/` + `src/security/traits.rs`.

## Architecture

Plaw exposes a `Sandbox` trait
([`src/security/traits.rs`](../src/security/traits.rs)) that abstracts
OS-level process isolation. `ShellTool` / `BrowserTool` / `MCP stdio`
all spawn their children through this trait. The factory in
[`src/security/detect.rs`](../src/security/detect.rs) picks the best
backend available on the current platform at startup.

```
┌─────────────────────────────────────────────┐
│ Sandbox trait (security/traits.rs)          │
│   wrap_command(cmd: &mut std::process)      │
│   spawn_with_integrity(cmd, level)          │
│   after_spawn(pid)                          │
│   name() / description() / is_available()   │
└──────────────┬──────────────────────────────┘
               │
       ┌───────┴───────┐
       │   factory     │  ← create_sandbox(&config.security)
       │  (detect.rs)  │
       └───────┬───────┘
               │
   ┌───────────┼────────────────────────────────┐
   │           │            │           │        │
   ▼           ▼            ▼           ▼        ▼
NoopSandbox  Firejail   Bubblewrap   Landlock   WindowsJobObject
                                                + optional Token IL
                                                  (per-tool config)
```

## Shipped backends (Phase 0)

| Backend | Platform | Isolation primitives | PR |
|---|---|---|---|
| **noop** | all | application-layer only (allowlists, path blocking) | baseline |
| **firejail** | Linux | namespaces + seccomp via `firejail` wrapper binary | early |
| **bubblewrap** | Linux | namespaces via `bwrap` (rootless-friendly) | early |
| **landlock** | Linux 5.13+ | filesystem ACL via Linux LSM | early |
| **docker** | Linux/macOS/Windows | container isolation (heavyweight) | early |
| **windows-job-object** | Windows | kernel Job Object: `KILL_ON_JOB_CLOSE` + resource caps + UI restrictions | [#77](https://github.com/newzlong/plaw-desktop/pull/77) |

All 6 are honest about their own gaps via `Sandbox::description()`.
Runtime inspection via `plaw sandbox status`.

## Shipped: Windows kernel-level hardening (Phase 0 deep-dive — PR #77)

`WindowsJobObjectSandbox` ships four kernel-enforced resource caps:

| Limit | Default | Config key |
|---|---|---|
| `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` | 256 | `windows_max_processes` |
| `JOB_OBJECT_LIMIT_PROCESS_MEMORY` | 2 GiB | `windows_process_memory_mb` |
| `JOB_OBJECT_LIMIT_PROCESS_TIME` | 600 s CPU time | `windows_process_cpu_time_secs` |
| `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION` | always-on | — |

Plus UI restrictions (`HANDLES` + `SYSTEMPARAMETERS`) and the always-on
`KILL_ON_JOB_CLOSE` policy: every child tied to a plaw-owned job
terminates when plaw exits.

See [`src/security/windows_job.rs`](../src/security/windows_job.rs)
for the implementation + the SAFETY notes covering the three Win32
unsafe call sites.

## Shipped: Token Integrity Level (Phase 1 — PRs #87–#91)

Operators can opt-in per-tool to lower the Mandatory Integrity Level of
spawned children via `[security.sandbox.integrity]`:

```toml
[security.sandbox.integrity]
shell = "low"  # ShellTool spawns at S-1-16-4096 (Low IL)
```

**Default OFF** per [Lens C Gatekeeper failure
mode](#lens-c-gatekeeper-failure-mode). Empty config = byte-identical to
pre-PR-#91 behavior.

The stack landed in 6 PRs:

| PR | Phase | What |
|---|---|---|
| [#82](https://github.com/newzlong/plaw-desktop/pull/82) | 0.5 | `plaw sandbox status` CLI for runtime inspection |
| [#87](https://github.com/newzlong/plaw-desktop/pull/87) | prereq | Routed Browser (3 spawn sites) + MCP stdio through the `Sandbox` trait |
| [#88](https://github.com/newzlong/plaw-desktop/pull/88) | 1a-1 | `IntegrityLevel` enum + `current_process_integrity()` + `validate_lowerable()` observation primitives |
| [#89](https://github.com/newzlong/plaw-desktop/pull/89) | 1a-2 | `spawn_with_lowered_token` + `LoweredChild` + `plaw-il-probe` test binary. **End-to-end pipeline validated on real Windows host** (`DuplicateTokenEx` + `SetTokenInformation` + `CreateProcessAsUserW`). |
| [#90](https://github.com/newzlong/plaw-desktop/pull/90) | 1b | `Sandbox::spawn_with_integrity(cmd, level) -> SandboxedChild` trait extension + `WindowsJobObjectSandbox` override |
| [#91](https://github.com/newzlong/plaw-desktop/pull/91) | 1c | `[security.sandbox.integrity]` config + ShellTool wiring (default OFF) |

Plus self-review follow-ups:

| PR | Scope |
|---|---|
| [#92](https://github.com/newzlong/plaw-desktop/pull/92) | docs — `[security.sandbox]` + `[security.sandbox.integrity]` in `config-reference.md` |
| [#93](https://github.com/newzlong/plaw-desktop/pull/93) | hotfix — Linux env loss from PR #91 trait refactor (CRITICAL); TOML round-trip cleanliness; `deny_unknown_fields` |
| [#94](https://github.com/newzlong/plaw-desktop/pull/94) | tests — `IsProcessInJob` kernel pin + schema↔runtime drift compile pin + parent-IL guard on Medium probe |

### Empirical IL compatibility envelope (PR #89 integration tests)

| Level | SID | Behavior | Usability |
|---|---|---|---|
| `default` | parent's IL | no lowering | ✅ works today |
| `medium` | `S-1-16-8192` | same as parent for unelevated plaw | ✅ works today |
| `low` | `S-1-16-4096` | kernel write-deny on user profile + most FS | ⚠️ needs Phase 1c.2 for ShellTool output capture |
| `untrusted` | `S-1-16-0` | most restrictive | ❌ breaks Rust MSVC C runtime DLL load (`STATUS_DLL_INIT_FAILED 0xC0000142`); reserved for enum completeness |

### Lens C Gatekeeper failure mode

The audit #11 design discovery (workflow `wwssfii3c`) surfaced an
empirical compatibility constraint: a **default-on workspace-wide Low
IL** would break first-run `cargo build` / `npm install` (Low-IL
processes can't write to Medium-IL tempdirs), and users would disable
sandboxing entirely. PR #91 ships with `default_level = None` (no
lowering); operators opt-in per-tool only after testing compatibility
with their workload.

## Roadmap: Phase 1c.2 (designed, deferred)

`ShellTool::execute` currently returns a **clear deferred-feature
error** when `[security.sandbox.integrity] shell = "low"` is set,
because the Phase 1a-2 `spawn_with_lowered_token` primitive does not
yet support piped stdio capture — children inherit the parent console.
Phase 1c.2 will add that capture path.

The hardened design is captured in the
[`project_phase_1c2_locked_findings`](https://github.com/newzlong/plaw-desktop)
memo (internal). Key findings from the 5-lens discovery +
3-adversarial-refute pass:

- Use `CreateNamedPipeW` with `FILE_FLAG_OVERLAPPED` (NOT `CreatePipe`)
  so tokio's IOCP path runs; `CreatePipe` would route through
  `Blocking::new` and deadlock under blocking-pool pressure.
- Use `STARTUPINFOEXW` + `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` instead of
  bare `bInheritHandles=TRUE` — kernel-enforced handle whitelist
  prevents inheriting unrelated sockets / file handles from the parent.
- Use `CREATE_NO_WINDOW` to avoid spawning a console on headless
  Windows sessions.
- `wait_with_output` needs cancel-safety: drop without `kill()` would
  orphan the child via `spawn_blocking`-parked `WaitForSingleObject`.
- Per-caller stdio options through the `Sandbox` trait — always-piped
  Lowered would break MCP-stdio + Browser callers that currently
  inherit parent stdio.

Estimated scope: ~450 LOC across 3 PRs (named-pipe creation + ACL +
tokio NamedPipe adoption + race-safe `DuplicateHandle` + KillOnDrop +
`CREATE_NO_WINDOW` + corrected `IsProcessInJob`-style test design).
Multi-week dedicated work; deferred to a future session.

## Roadmap: not in scope today

- **BrowserTool Token IL**: Chromium expects parent at Medium IL to
  self-lower renderers. Pre-lowering risks breaking the renderer fork
  invariant; needs separate spike before committing.
- **`web_fetch` Token IL**: N/A. `web_fetch` is in-process `reqwest`
  with no subprocess to lower (Lens C correction).
- **macOS sandbox profile**: no roadmap. Plaw on macOS today runs at
  `NoopSandbox` unless `docker` is configured.
- **Linux seccomp profile**: covered transitively by `firejail` and
  `bubblewrap`. No standalone seccomp-only backend planned.

## Reading source

- [`src/security/traits.rs`](../src/security/traits.rs) — `Sandbox`
  trait + `SandboxedChild` + `IntegrityLevel` re-export
- [`src/security/windows_job.rs`](../src/security/windows_job.rs) —
  `WindowsJobObjectSandbox` (PR #77 + #90 override)
- [`src/security/windows_token_il.rs`](../src/security/windows_token_il.rs)
  — Token IL primitives (PR #88 + #89)
- [`src/security/firejail.rs`](../src/security/firejail.rs) /
  [`bubblewrap.rs`](../src/security/bubblewrap.rs) /
  [`docker.rs`](../src/security/docker.rs) /
  [`landlock.rs`](../src/security/landlock.rs) — Linux backends
- [`src/security/detect.rs`](../src/security/detect.rs) — factory
- [`src/main.rs`](../src/main.rs) — `plaw sandbox status` CLI

## Reading config

See [`config-reference.md`](config-reference.md) §`[security.sandbox]`
and §`[security.sandbox.integrity]` for every key with defaults,
purpose, and operator-facing TOML examples.
