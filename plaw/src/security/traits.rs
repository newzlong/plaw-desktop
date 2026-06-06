//! Sandbox trait for pluggable OS-level isolation.
//!
//! This module defines the [`Sandbox`] trait, which abstracts OS-level process
//! isolation backends. Implementations wrap shell commands with platform-specific
//! sandboxing (e.g., seccomp, AppArmor, namespaces) to limit the blast radius
//! of tool execution. [`crate::tools::ShellTool`] applies a sandbox to every
//! command via [`Sandbox::wrap_command`] before spawning; the active backend is
//! selected at startup by [`crate::security::create_sandbox`] from the user's
//! `[security.sandbox]` config (defaults to [`NoopSandbox`] when no platform
//! backend is available — notably the entire Windows path today).

use async_trait::async_trait;
use std::process::Command;

/// PR #90 Phase 1b: Mandatory Integrity Level a [`Sandbox`] can request
/// for a spawned child. Mirrors
/// [`crate::security::windows_token_il::IntegrityLevel`] on Windows so the
/// trait signature parses cross-platform — on non-Windows targets the
/// enum has a single `Default` variant (no-op), so any caller that
/// passes `IntegrityLevel::Default` gets byte-identical behavior to the
/// pre-#90 `wrap_command + spawn + after_spawn` flow.
///
/// Phase 1c (PR #91) wires this through `ShellTool` from the
/// `[security.sandbox.integrity]` config — default `None` per Lens C
/// (Gatekeeper failure mode).
#[cfg(target_os = "windows")]
pub use crate::security::windows_token_il::IntegrityLevel;

/// PR #90 Phase 1b: single-variant stub of `IntegrityLevel` for
/// non-Windows targets. Keeps the [`Sandbox::spawn_with_integrity`]
/// trait signature uniform across platforms — Linux / macOS backends
/// never lower IL (the concept doesn't exist there) so `Default` is
/// the only meaningful value.
#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IntegrityLevel {
    /// "Use whatever the parent already has — no lowering attempted."
    /// On non-Windows this is the ONLY variant.
    #[default]
    Default,
}

/// PR #90 Phase 1b: result of [`Sandbox::spawn_with_integrity`].
///
/// Two variants because the Windows lowered-IL path returns a
/// [`crate::security::windows_token_il::LoweredChild`] (built on raw
/// Win32 handles via `CreateProcessAsUserW`) rather than a
/// `tokio::process::Child` (which has no public constructor for
/// adopting a foreign handle). All other backends produce
/// `SandboxedChild::Tokio` via the default trait impl so the existing
/// `tokio` ecosystem keeps working unchanged.
///
/// Forwarding methods ([`Self::id`], [`Self::wait`], [`Self::kill`])
/// hide the enum variant from most callers — they only need
/// `match`-ing if they want to plug into tokio-specific features (e.g.
/// piped stdout). Phase 1c's `ShellTool` will use only the forwarding
/// API.
#[derive(Debug)]
pub enum SandboxedChild {
    /// Spawn via `tokio::process::Command::spawn()` — the default
    /// impl's output and every non-Windows backend's output today.
    Tokio(tokio::process::Child),
    /// Spawn via
    /// [`crate::security::windows_token_il::spawn_with_lowered_token`]
    /// — Windows-only, populated by `WindowsJobObjectSandbox` when the
    /// caller requested a non-`Default` IL.
    #[cfg(target_os = "windows")]
    Lowered(crate::security::windows_token_il::LoweredChild),
}

impl SandboxedChild {
    /// OS process id of the spawned child. Stable for the lifetime of
    /// the `SandboxedChild`. Returns `None` only when the underlying
    /// `tokio::process::Child` reports `None` (which happens after
    /// `wait()` consumes the child); the Windows lowered-IL path
    /// always has a pid because we capture it from
    /// `PROCESS_INFORMATION.dwProcessId` at spawn time.
    pub fn id(&self) -> Option<u32> {
        match self {
            Self::Tokio(c) => c.id(),
            #[cfg(target_os = "windows")]
            Self::Lowered(c) => Some(c.id()),
        }
    }

    /// Block until the child exits and return its `ExitStatus`.
    /// Consumes `self` so handles are closed deterministically once
    /// the wait completes.
    ///
    /// For the Windows lowered-IL path: the underlying
    /// `LoweredChild::wait()` is SYNC (blocks the current thread on
    /// `WaitForSingleObject`). We wrap it in `tokio::task::spawn_blocking`
    /// so callers awaiting on a multi-threaded runtime don't stall
    /// the worker. The probe-binary integration tests in PR #89
    /// already validated `LoweredChild::wait` end-to-end; this just
    /// adapts it to the async trait surface.
    pub async fn wait(self) -> std::io::Result<std::process::ExitStatus> {
        match self {
            Self::Tokio(mut c) => c.wait().await,
            #[cfg(target_os = "windows")]
            Self::Lowered(c) => tokio::task::spawn_blocking(move || c.wait())
                .await
                .map_err(|e| {
                    std::io::Error::other(format!("LoweredChild wait task panicked: {e}"))
                })?,
        }
    }

    /// Force-terminate the child. Consumes `self` so the handles
    /// close. Used for cancellation / cleanup paths.
    #[allow(dead_code)]
    pub async fn kill(self) -> std::io::Result<()> {
        match self {
            Self::Tokio(mut c) => c.kill().await,
            #[cfg(target_os = "windows")]
            Self::Lowered(c) => tokio::task::spawn_blocking(move || c.kill())
                .await
                .map_err(|e| {
                    std::io::Error::other(format!("LoweredChild kill task panicked: {e}"))
                })?,
        }
    }
}

/// Sandbox backend for OS-level process isolation.
///
/// Implement this trait to add a new sandboxing strategy. The runtime queries
/// [`is_available`](Sandbox::is_available) at startup to select the best
/// backend for the current platform, then calls
/// [`wrap_command`](Sandbox::wrap_command) before every shell execution.
///
/// Implementations must be `Send + Sync` because the sandbox may be shared
/// across concurrent tool executions on the Tokio runtime.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Wrap a command with sandbox protection.
    ///
    /// Mutates `cmd` in place to apply isolation constraints (e.g., prepending
    /// a wrapper binary, setting environment variables, adding seccomp filters).
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the sandbox configuration cannot be applied
    /// (e.g., missing wrapper binary, invalid policy file).
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()>;

    /// Post-spawn hook called by [`crate::tools::ShellTool`] immediately after
    /// the child process is spawned, before it is awaited. Receives the child
    /// process ID so the sandbox can assign it to a resource container
    /// (e.g. Windows Job Object, Linux cgroup) by PID lookup.
    ///
    /// Default implementation is a no-op so existing backends
    /// ([`NoopSandbox`], Linux/macOS sandboxes that operate entirely
    /// pre-spawn via [`wrap_command`](Sandbox::wrap_command)) need no change.
    /// The Windows Job Object backend uses this hook to call
    /// `AssignProcessToJobObject` on the freshly-spawned PID — that platform's
    /// isolation mechanism is fundamentally post-spawn-only (an already-running
    /// process is adopted into a job; there is no pre-spawn flag equivalent
    /// to firejail's wrapper-binary approach).
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the post-spawn step fails. Callers should
    /// log and continue rather than killing the child: by the time this hook
    /// fires the process is already executing, and aborting the tool execution
    /// because sandbox attachment failed would be surprising for users who
    /// configured the sandbox optimistically. The active backend's `name()`
    /// in logs lets operators tell whether isolation actually took effect.
    fn after_spawn(&self, _pid: u32) -> std::io::Result<()> {
        Ok(())
    }

    /// PR #90 Phase 1b: spawn `cmd` at the requested Token Integrity
    /// Level and return a [`SandboxedChild`].
    ///
    /// The default impl mirrors today's [`Self::wrap_command`] +
    /// `tokio::process::Command::spawn()` + [`Self::after_spawn`]
    /// choreography and ignores `_level` — keeping every existing
    /// backend (Noop / Firejail / Bubblewrap / Docker / Landlock)
    /// byte-identical to pre-PR #90 behaviour. Only
    /// `WindowsJobObjectSandbox` overrides this method, and only the
    /// non-`Default` branch actually consults `_level` (the `Default`
    /// branch falls through to the wrap+spawn+after_spawn flow exactly
    /// like the default impl).
    ///
    /// `_level == IntegrityLevel::Default` → byte-identical to the
    /// `wrap_command + spawn + after_spawn` sequence shell.rs has run
    /// since PR #77. ShellTool's current call sites (and PR #87's
    /// Browser / MCP-stdio wiring) keep their existing semantics
    /// unchanged.
    ///
    /// Failure of `after_spawn` is fail-soft — matches the
    /// shell.rs:592 / browser.rs / stdio.rs pattern established in
    /// PR #77 + PR #87. The kernel-enforced Job Object caps are
    /// defense-in-depth; the spawn itself succeeds and the child
    /// runs even if the Job Object assignment fails (rare —
    /// usually only on a permissions bug).
    async fn spawn_with_integrity(
        &self,
        mut cmd: tokio::process::Command,
        _level: IntegrityLevel,
    ) -> std::io::Result<SandboxedChild> {
        self.wrap_command(cmd.as_std_mut())?;
        let child = cmd.spawn()?;
        if let Some(pid) = child.id() {
            if let Err(e) = self.after_spawn(pid) {
                tracing::warn!(
                    sandbox = self.name(),
                    pid,
                    error = %e,
                    "Sandbox::spawn_with_integrity default impl: after_spawn failed; continuing"
                );
            }
        }
        Ok(SandboxedChild::Tokio(child))
    }

    /// Check if this sandbox backend is available on the current platform.
    ///
    /// Returns `true` when all required kernel features, binaries, and
    /// permissions are present. The runtime calls this at startup to select
    /// the most capable available backend.
    fn is_available(&self) -> bool;

    /// Return the human-readable name of this sandbox backend.
    ///
    /// Used in logs and diagnostics to identify which isolation strategy is
    /// active (e.g., `"firejail"`, `"bubblewrap"`, `"none"`).
    fn name(&self) -> &str;

    /// Return a brief description of the isolation guarantees this sandbox provides.
    ///
    /// Displayed in status output and health checks so operators can verify
    /// the active security posture.
    fn description(&self) -> &str;
}

/// No-op sandbox that provides no additional OS-level isolation.
///
/// Always reports itself as available. Use this as the fallback when no
/// platform-specific sandbox backend is detected, or in development
/// environments where isolation is not required. Security in this mode
/// relies entirely on application-layer controls.
#[derive(Debug, Clone, Default)]
pub struct NoopSandbox;

impl Sandbox for NoopSandbox {
    fn wrap_command(&self, _cmd: &mut Command) -> std::io::Result<()> {
        // Pass through unchanged
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "none"
    }

    fn description(&self) -> &str {
        "No sandboxing (application-layer security only)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sandbox_name() {
        assert_eq!(NoopSandbox.name(), "none");
    }

    #[test]
    fn noop_sandbox_is_always_available() {
        assert!(NoopSandbox.is_available());
    }

    #[test]
    fn noop_sandbox_wrap_command_is_noop() {
        let mut cmd = Command::new("echo");
        cmd.arg("test");
        let original_program = cmd.get_program().to_string_lossy().to_string();
        let original_args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        let sandbox = NoopSandbox;
        assert!(sandbox.wrap_command(&mut cmd).is_ok());

        // Command should be unchanged
        assert_eq!(cmd.get_program().to_string_lossy(), original_program);
        assert_eq!(
            cmd.get_args()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            original_args
        );
    }

    // ─── PR #90 Phase 1b: SandboxedChild + default spawn_with_integrity ─

    /// The default impl of `spawn_with_integrity` mirrors the
    /// `wrap_command + spawn + after_spawn` flow. Verified via
    /// `NoopSandbox` (the only backend guaranteed to be available
    /// cross-platform): a Default-IL spawn returns
    /// `SandboxedChild::Tokio` (the only variant a non-Windows
    /// backend can populate) and the child exits cleanly.
    #[tokio::test]
    async fn noop_sandbox_spawn_with_integrity_default_returns_tokio_variant() {
        let sandbox = NoopSandbox;
        let mut cmd = tokio::process::Command::new(if cfg!(windows) { "cmd.exe" } else { "true" });
        if cfg!(windows) {
            cmd.arg("/C").arg("exit 0");
        }
        let child = sandbox
            .spawn_with_integrity(cmd, IntegrityLevel::Default)
            .await
            .expect("spawn_with_integrity must succeed for a trivial command");
        // Match the variant explicitly. Phase 1c will use the
        // forwarding API (id/wait/kill) instead of matching, but
        // the variant assertion here pins the default-impl contract.
        match child {
            SandboxedChild::Tokio(_) => {}
            #[cfg(target_os = "windows")]
            SandboxedChild::Lowered(_) => {
                panic!(
                    "NoopSandbox::spawn_with_integrity at Default must return Tokio, not Lowered"
                )
            }
        }
    }

    /// `SandboxedChild::wait` forwards to the underlying child and
    /// returns its ExitStatus. Pins that the async wrapper around
    /// the Windows `LoweredChild::wait` (which runs sync inside
    /// `spawn_blocking`) doesn't change the exit-status semantics
    /// for `Tokio` variants.
    #[tokio::test]
    async fn sandboxed_child_wait_returns_exit_status() {
        let sandbox = NoopSandbox;
        let mut cmd = tokio::process::Command::new(if cfg!(windows) { "cmd.exe" } else { "true" });
        if cfg!(windows) {
            cmd.arg("/C").arg("exit 0");
        }
        let child = sandbox
            .spawn_with_integrity(cmd, IntegrityLevel::Default)
            .await
            .unwrap();
        let pid = child.id();
        assert!(pid.is_some(), "fresh child should report a pid");
        let status = child.wait().await.expect("wait must succeed");
        assert!(status.success(), "exit 0 should report success");
    }
}
