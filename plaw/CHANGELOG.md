# Changelog

All notable changes to Plaw will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security
- **Audit #11 Phase 1 — Windows kernel-level isolation, end-to-end** (PRs #77/#82/#87–#91):
  - **Phase 0 (PR #77)**: `WindowsJobObjectSandbox` ships `KILL_ON_JOB_CLOSE` + 4 kernel-enforced
    resource caps (`JOB_OBJECT_LIMIT_ACTIVE_PROCESS` / `JOB_OBJECT_LIMIT_PROCESS_MEMORY` /
    `JOB_OBJECT_LIMIT_PROCESS_TIME` / `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`) +
    UI restrictions (`HANDLES` + `SYSTEMPARAMETERS`).
  - **Phase 0.5 (PR #82)**: `plaw sandbox status` CLI surfaces the active backend, its honest
    capability description, and the per-platform resource caps for operator inspection.
  - **Prereq (PR #87)**: Browser (3 spawn sites) and MCP stdio (1 spawn site) routed through
    the `Sandbox` trait — previously bypassed it on Windows + Linux/macOS.
  - **Phase 1a-1 (PR #88)**: `IntegrityLevel { Default, Medium, Low, Untrusted }` enum +
    `current_process_integrity()` + `validate_lowerable()` observation primitives. Dormant.
  - **Phase 1a-2 (PR #89)**: `spawn_with_lowered_token` + `LoweredChild { id, wait, kill }`
    + `plaw-il-probe` test binary. End-to-end pipeline validated on real Windows host
    (`DuplicateTokenEx` + `SetTokenInformation` + `CreateProcessAsUserW`). Dormant.
  - **Phase 1b (PR #90)**: `Sandbox::spawn_with_integrity(cmd, level) -> SandboxedChild`
    trait extension. Default impl mirrors today's `wrap_command + spawn + after_spawn` flow
    byte-identically; `WindowsJobObjectSandbox` overrides to route non-`Default` IL through
    the Token IL spawn primitives.
  - **Phase 1c (PR #91)**: `[security.sandbox.integrity]` config block exposes per-tool
    Token IL preferences to operators. **Default OFF** per Lens C Gatekeeper failure mode.
    `ShellTool::execute` routes through `spawn_with_integrity` — byte-identical at
    `Default`, returns a clear deferred-feature error at non-`Default` until Phase 1c.2
    ships piped stdio for `CreateProcessAsUserW`. See `docs/config-reference.md` for the
    `[security.sandbox]` and `[security.sandbox.integrity]` reference.
  - **Audit #11 self-review hotfix (PR #93)**: adversarial 4-lens self-review of the
    Phase 1 stack surfaced 1 CRITICAL + 3 HIGH bugs, all addressed:
    - **CRITICAL**: PR #91's trait refactor moved `Sandbox::wrap_command` from BEFORE
      `ShellTool`'s `env_clear + env(SAFE_VARS)` block to AFTER it (inside
      `Sandbox::spawn_with_integrity`'s default impl). Each Linux backend's
      `*cmd = wrapper_cmd` swap then silently DISCARDED the carefully-built `PATH` /
      `PYTHONUTF8` / `NODE_PATH` / `PLAYWRIGHT_BROWSERS_PATH` / etc. Users with
      `[security.sandbox] backend = "bubblewrap"|"firejail"|"docker"` saw "bundled
      python not found"-class breakage. Hotfix: each backend's `wrap_command` now
      captures `get_envs() + get_current_dir()` BEFORE the swap and re-applies after.
      Docker forwards via explicit `-e KEY=VALUE` flags (containers don't propagate
      host env without them). Pinned by 3 regression tests.
    - **HIGH**: `SandboxConfig::default()` emitted an empty `[integrity]` block on
      `toml::to_string` round-trip — polluting user config diffs after upgrade. Fix:
      `skip_serializing_if = "SandboxIntegrityConfig::is_empty"` on the field +
      `Option::is_none` on inner fields.
    - **HIGH**: PR #91 docstring promised `git_operations = "low"` worked but
      `resolve()` only matched `"shell"` — silent operator intent drop. Fix:
      `#[serde(deny_unknown_fields)]` on `SandboxIntegrityConfig` (typo'd or
      unsupported per-tool keys now hard-error at parse) + docstring corrected to
      "Override for `ShellTool` only".
  - **Audit #11 defense-in-depth pins (PR #94)**: tests-only follow-up adding 3
    regression pins surfaced by the same self-review:
    - **H-1**: `IsProcessInJob` kernel-level assertion that Lowered-IL children are
      actually assigned to the Job Object. Without this, a silent `after_spawn`
      failure would leave the child UNRESTRAINED by KILL_ON_JOB_CLOSE + resource
      caps + UI restrictions and the existing variant-only test stayed green.
    - **M-4**: compile-time exhaustive `match` against schema↔runtime enum drift.
      A future 5th runtime `IntegrityLevel` variant without a schema mirror stops
      the build instead of silently mismatching production `match` patterns.
    - **M-5**: parent-IL guard on `spawn_probe_at_medium_writes_medium_sid`.
      Skips with a clear `eprintln!` on elevated or AppContainer CI runners
      instead of false-failing or hanging.
  - **Audit #11 docs (PR #92, #95)**:
    - PR #92: `[security.sandbox]` + `[security.sandbox.integrity]` reference added
      to `docs/config-reference.md` with default-OFF + Phase 1c.2 deferred-feature
      note + per-level wire/behavior/usability table.
    - PR #95: `docs/sandboxing.md` rewritten from stale "Proposal / Roadmap"
      pseudocode to a Phase-0+1-shipped status doc with architecture diagram,
      backend matrix, IL compatibility envelope, and explicit Phase 1c.2 + future
      roadmap deferrals.
- **Legacy XOR cipher migration**: The `enc:` prefix (XOR cipher) is now deprecated. 
  Secrets using this format will be automatically migrated to `enc2:` (ChaCha20-Poly1305 AEAD)
  when decrypted via `decrypt_and_migrate()`. A `tracing::warn!` is emitted when legacy
  values are encountered. The XOR cipher will be removed in a future release.

### Added
- `SecretStore::decrypt_and_migrate()` — Decrypts secrets and returns a migrated `enc2:` 
  value if the input used the legacy `enc:` format
- `SecretStore::needs_migration()` — Check if a value uses the legacy `enc:` format
- `SecretStore::is_secure_encrypted()` — Check if a value uses the secure `enc2:` format
- **Telegram mention_only mode** — New config option `mention_only` for Telegram channel.
  When enabled, bot only responds to messages that @-mention the bot in group chats.
  Direct messages always work regardless of this setting. Default: `false`.

### Deprecated
- `enc:` prefix for encrypted secrets — Use `enc2:` (ChaCha20-Poly1305) instead.
  Legacy values are still decrypted for backward compatibility but should be migrated.

### Fixed
- **Gemini thinking model support** — Responses from thinking models (e.g. `gemini-3-pro-preview`)
  are now handled correctly. The provider skips internal reasoning parts (`thought: true`) and
  signature parts (`thoughtSignature`), extracting only the final answer text. Falls back to
  thinking content when no non-thinking response is available.
- Updated default gateway port to `42617`.
- Removed all user-facing references to port `3000`.
- **Onboarding channel menu dispatch** now uses an enum-backed selector instead of hard-coded
  numeric match arms, preventing duplicated pattern arms and related `unreachable pattern`
  compiler warnings in `src/onboard/wizard.rs`.
- **OpenAI native tool spec parsing** now uses owned serializable/deserializable structs,
  fixing a compile-time type mismatch when validating tool schemas before API calls.

## [0.1.0] - 2026-02-13

### Added
- **Core Architecture**: Trait-based pluggable system for Provider, Channel, Observer, RuntimeAdapter, Tool
- **Provider**: OpenRouter implementation (access Claude, GPT-4, Llama, Gemini via single API)
- **Channels**: CLI channel with interactive and single-message modes
- **Observability**: NoopObserver (zero overhead), LogObserver (tracing), MultiObserver (fan-out)
- **Security**: Workspace sandboxing, command allowlisting, path traversal blocking, autonomy levels (ReadOnly/Supervised/Full), rate limiting
- **Tools**: Shell (sandboxed), FileRead (path-checked), FileWrite (path-checked)
- **Memory (Brain)**: SQLite persistent backend (searchable, survives restarts), Markdown backend (plain files, human-readable)
- **Heartbeat Engine**: Periodic task execution from HEARTBEAT.md
- **Runtime**: Native adapter for Mac/Linux/Raspberry Pi
- **Config**: TOML-based configuration with sensible defaults
- **Onboarding**: Interactive CLI wizard with workspace scaffolding
- **CLI Commands**: agent, gateway, status, cron, channel, tools, onboard
- **CI/CD**: GitHub Actions with cross-platform builds (Linux, macOS Intel/ARM, Windows)
- **Tests**: 159 inline tests covering all modules and edge cases
- **Binary**: 3.1MB optimized release build (includes bundled SQLite)

### Security
- Path traversal attack prevention
- Command injection blocking
- Workspace escape prevention
- Forbidden system path protection (`/etc`, `/root`, `~/.ssh`)

[0.1.0]: https://github.com/theonlyhennygod/plaw/releases/tag/v0.1.0
