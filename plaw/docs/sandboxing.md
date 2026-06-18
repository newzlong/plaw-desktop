# Plaw Sandboxing — Status & Roadmap

> **Status: Phase 0 + Phase 1 + Phase 1c.2 shipped**. ShellTool now
> captures output from Token-IL-lowered children.
> Last updated: 2026-06-10.
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
| `low` | `S-1-16-4096` | kernel write-deny on user profile + most FS | ✅ ShellTool captures output (Phase 1c.2); **commands that write outside Low-labeled dirs fail with access-denied** — this is the sandbox working as intended |
| `untrusted` | `S-1-16-0` | most restrictive | ❌ breaks Rust MSVC C runtime DLL load (`STATUS_DLL_INIT_FAILED 0xC0000142`); reserved for enum completeness |

### Lens C Gatekeeper failure mode

The audit #11 design discovery (workflow `wwssfii3c`) surfaced an
empirical compatibility constraint: a **default-on workspace-wide Low
IL** would break first-run `cargo build` / `npm install` (Low-IL
processes can't write to Medium-IL tempdirs), and users would disable
sandboxing entirely. PR #91 ships with `default_level = None` (no
lowering); operators opt-in per-tool only after testing compatibility
with their workload.

## Shipped: Phase 1c.2 (Token IL output capture)

`ShellTool::execute` now **captures stdout/stderr** from a Token-IL-
lowered child. Setting `[security.sandbox.integrity] shell = "low"`
runs shell commands at Low IL with their output captured normally —
the previous deferred-feature bail is gone.

| PR | Phase | What |
|---|---|---|
| [#103](https://github.com/newzlong/plaw-desktop/pull/103) | 1c.2a | `spawn_with_lowered_token_piped` — IOCP-backed named-pipe stdio capture primitive (dormant) |
| [#104](https://github.com/newzlong/plaw-desktop/pull/104) | 1c.2b+c | `LoweredChild::wait_with_output` (concurrent drain) + `SandboxedChild::wait_with_output` forwarding + ShellTool wired through the piped path |

Design (from the 5-lens discovery + 3-adversarial-refute pass, validated
by a standalone IOCP spike before any unsafe was written):

- `tokio::net::windows::named_pipe::NamedPipeServer` (which IS
  `CreateNamedPipeW` + `FILE_FLAG_OVERLAPPED` under the hood) — NOT
  `CreatePipe`, which tokio wraps in `Blocking::new` and deadlocks under
  blocking-pool pressure.
- `STARTUPINFOEXW` + `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` whitelisting
  exactly the 3 stdio handles — not bare `bInheritHandles=TRUE`, which
  would inherit every inheritable handle in the parent.
- `CREATE_NO_WINDOW` to avoid a console on headless sessions.
- `KillOnDrop` guard so a spawn that fails after `CreateProcessAsUserW`
  cannot orphan the child before the Job Object adopts it.
- The "always-piped Lowered breaks MCP/Browser" concern from discovery
  no longer applies: `ShellTool` is the **sole** caller of
  `spawn_with_integrity` (MCP stdio, Browser, and ProcessTool wire
  through `wrap_command` + spawn directly per PR #87), so always-piping
  the Lowered path is correct.

**Cancel behavior:** when ShellTool's `timeout` elapses, the child is
force-killed on BOTH paths. The Tokio path uses tokio's
`kill_on_drop(true)`; the Lowered path holds an internal process-handle
kill guard (`ProcessKillGuard`) in `wait_with_output` that
`TerminateProcess`es the child when the future is dropped — necessary
because the lowered child's wait runs in a detached `spawn_blocking`
task that can't itself be cancelled. The Job Object's
`KILL_ON_JOB_CLOSE` remains a backstop. This is what makes the
"timed out … and was killed" message true.

## Decided against: BrowserTool Token IL (REJECTED, not deferred)

Investigated 2026-06-11 (spike `wad03c4sw`, verified against `browser.rs`).
**Decision: BrowserTool will NOT adopt Token IL lowering.** Containment
for the browser = the existing `WindowsJobObjectSandbox` (PR #77/#87) +
Chromium's own renderer sandbox. This is a settled won't-do, not a
pending spike — don't re-open it as fresh discovery.

What BrowserTool actually spawns (verified):

- **`agent_browser` (default)**: plaw spawns the `agent-browser` Node CLI
  (`browser.rs` spawn sites: the two `--version` probes + the main
  `run_command` exec); that CLI internally launches a
  `chrome-headless-shell` grandchild plaw does not directly control.
  This is the ONLY backend with a plaw-controlled subprocess. Already
  routed through the Sandbox trait (`wrap_command` + `after_spawn` →
  Job Object) per PR #87.
- **`rust_native`**: spawns NOTHING — connects via WebDriver
  (`ClientBuilder::rustls().connect(native_webdriver_url)`, default
  `http://127.0.0.1:9515`) to an externally-started
  `chromedriver`/`geckodriver`. `native_chrome_path` is a
  `goog:chromeOptions.binary` hint, not a spawn. Token IL is a no-op.
- **`computer_use`**: remote HTTP sidecar (`127.0.0.1:8787`). No
  subprocess. Token IL is a no-op.

Why Token IL is the wrong primitive here:

1. **It conflicts with Chromium's sandbox, kernel-enforced.** Chromium's
   Windows sandbox is asymmetric: the broker (browser process) runs at
   Medium IL and spawns renderers that IT lowers to Low/AppContainer.
   Token lowering is monotonic — a process can only mint tokens at or
   below its own IL — and a Low-IL process has `SeImpersonatePrivilege`
   stripped by the kernel, which is exactly the privilege the broker
   needs to create lowered renderer tokens. Pre-lowering the browser to
   Low therefore either breaks renderer spawn (automation fails) or
   forces `--no-sandbox` (which disables Chromium's renderer isolation
   entirely — strictly worse). plaw does NOT pass `--no-sandbox` today,
   so Chromium's sandbox is active; lowering would REMOVE a working
   containment layer to add a broken one.
2. **A Low-IL browser can't even start.** Low IL kernel-denies writes to
   Medium-labeled `%USERPROFILE%` / `%APPDATA%` / `%TEMP%`, where
   Chromium's user-data-dir, disk cache, and cookies DB live → profile
   init fails. Untrusted IL breaks the C-runtime DLL load entirely. A
   with-config path (Low-labeled `--user-data-dir` + `--disable-gpu`)
   is fragile, non-transparent, and still doesn't solve (1).
3. **Only 1 of 3 backends spawns a plaw-controlled process at all** — and
   it's the Chromium-family case where lowering does the most damage.

Why ShellTool was the right (and only) Token IL adopter: a shell is an
IL-unaware leaf process, so Low IL is both safe and useful. Chromium is
not — it wants to BE the lower-IL party, conflicting with Token IL.
"We shipped IL for the shell" does not generalize to the browser.

Reopen trigger (narrow): only if a future backend runs Chromium OUT of
plaw's process tree at Medium IL and exposes an endpoint (the
`rust_native` shape), so plaw could lower its own *client* without
touching the browser tree — and even then the gain is marginal.

## Roadmap: not in scope today

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
