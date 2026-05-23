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
}
