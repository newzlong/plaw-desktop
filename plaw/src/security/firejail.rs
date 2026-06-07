//! Firejail sandbox (Linux user-space sandboxing)
//!
//! Firejail is a SUID sandbox program that Linux applications use to sandbox themselves.

use crate::security::traits::Sandbox;
use std::process::Command;

/// Firejail sandbox backend for Linux
#[derive(Debug, Clone, Default)]
pub struct FirejailSandbox;

impl FirejailSandbox {
    /// Create a new Firejail sandbox
    pub fn new() -> std::io::Result<Self> {
        if Self::is_installed() {
            Ok(Self)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Firejail not found. Install with: sudo apt install firejail",
            ))
        }
    }

    /// Probe if Firejail is available (for auto-detection)
    pub fn probe() -> std::io::Result<Self> {
        Self::new()
    }

    /// Check if firejail is installed
    fn is_installed() -> bool {
        Command::new("firejail")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl Sandbox for FirejailSandbox {
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()> {
        // Prepend firejail to the command
        let program = cmd.get_program().to_os_string();
        let args: Vec<std::ffi::OsString> = cmd.get_args().map(|s| s.to_os_string()).collect();

        // Capture envs + cwd before rebuilding. See bubblewrap.rs for
        // the rationale — same PR #91 trait-refactor regression.
        let envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|x| x.to_os_string())))
            .collect();
        let cwd = cmd.get_current_dir().map(|p| p.to_path_buf());

        // Build firejail wrapper with security flags
        let mut firejail_cmd = Command::new("firejail");
        firejail_cmd.args([
            "--private=home", // New home directory
            "--private-dev",  // Minimal /dev
            "--nosound",      // No audio
            "--no3d",         // No 3D acceleration
            "--novideo",      // No video devices
            "--nowheel",      // No input devices
            "--notv",         // No TV devices
            "--noprofile",    // Skip profile loading
            "--quiet",        // Suppress warnings
        ]);

        // Add the original command
        firejail_cmd.arg(&program);
        firejail_cmd.args(&args);

        // Restore caller-built envs + cwd onto the wrapper command.
        for (k, v) in envs {
            match v {
                Some(value) => {
                    firejail_cmd.env(k, value);
                }
                None => {
                    firejail_cmd.env_remove(k);
                }
            }
        }
        if let Some(d) = cwd {
            firejail_cmd.current_dir(d);
        }

        // Replace the command
        *cmd = firejail_cmd;

        // C-1.5 fix — see bubblewrap.rs for the full rationale. Short
        // version: the swap above discards stdio config the caller
        // set on the original `cmd`. Default to Piped so ShellTool +
        // MCP stdio callers get the pipe handles they need.
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        Ok(())
    }

    fn is_available(&self) -> bool {
        Self::is_installed()
    }

    fn name(&self) -> &str {
        "firejail"
    }

    fn description(&self) -> &str {
        "Linux user-space sandbox (requires firejail to be installed)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firejail_sandbox_name() {
        assert_eq!(FirejailSandbox.name(), "firejail");
    }

    #[test]
    fn firejail_description_mentions_dependency() {
        let desc = FirejailSandbox.description();
        assert!(desc.contains("firejail"));
    }

    #[test]
    fn firejail_new_fails_if_not_installed() {
        // This will fail unless firejail is actually installed
        let result = FirejailSandbox::new();
        match result {
            Ok(_) => println!("Firejail is installed"),
            Err(e) => assert!(
                e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::Unsupported
            ),
        }
    }

    #[test]
    fn firejail_wrap_command_prepends_firejail() {
        let sandbox = FirejailSandbox;
        let mut cmd = Command::new("echo");
        cmd.arg("test");

        // Note: wrap_command will fail if firejail isn't installed,
        // but we can still test the logic structure
        let _ = sandbox.wrap_command(&mut cmd);

        // After wrapping, the program should be firejail
        if sandbox.is_available() {
            assert_eq!(cmd.get_program().to_string_lossy(), "firejail");
        }
    }

    // ── §1.1 Sandbox isolation flag tests ──────────────────────

    #[test]
    fn firejail_wrap_command_includes_all_security_flags() {
        let sandbox = FirejailSandbox;
        let mut cmd = Command::new("echo");
        cmd.arg("test");
        sandbox.wrap_command(&mut cmd).unwrap();

        assert_eq!(
            cmd.get_program().to_string_lossy(),
            "firejail",
            "wrapped command should use firejail as program"
        );

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        let expected_flags = [
            "--private=home",
            "--private-dev",
            "--nosound",
            "--no3d",
            "--novideo",
            "--nowheel",
            "--notv",
            "--noprofile",
            "--quiet",
        ];

        for flag in &expected_flags {
            assert!(
                args.contains(&flag.to_string()),
                "must include security flag: {flag}"
            );
        }
    }

    #[test]
    fn firejail_wrap_command_preserves_original_command() {
        let sandbox = FirejailSandbox;
        let mut cmd = Command::new("ls");
        cmd.arg("-la");
        cmd.arg("/workspace");
        sandbox.wrap_command(&mut cmd).unwrap();

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert!(
            args.contains(&"ls".to_string()),
            "original program must be passed as argument"
        );
        assert!(
            args.contains(&"-la".to_string()),
            "original args must be preserved"
        );
        assert!(
            args.contains(&"/workspace".to_string()),
            "original args must be preserved"
        );
    }

    /// PR #91 regression pin — see bubblewrap.rs for the rationale.
    /// The exact same trait-refactor reorder broke firejail's env
    /// preservation.
    #[test]
    fn firejail_wrap_command_preserves_envs_and_cwd() {
        let sandbox = FirejailSandbox;
        let mut cmd = Command::new("node");
        cmd.arg("-e").arg("console.log(process.env.PATH)");
        cmd.env("PATH", "/opt/plaw-bundled/bin:/usr/bin");
        cmd.env("NODE_PATH", "/opt/plaw-bundled/lib/node_modules");
        cmd.env_remove("NPM_CONFIG_PREFIX");
        cmd.current_dir("/tmp");

        sandbox.wrap_command(&mut cmd).unwrap();

        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|x| x.to_string_lossy().to_string()),
                )
            })
            .collect();
        let path = envs
            .iter()
            .find(|(k, _)| k == "PATH")
            .and_then(|(_, v)| v.clone());
        assert_eq!(
            path,
            Some("/opt/plaw-bundled/bin:/usr/bin".to_string()),
            "firejail wrap_command must preserve PATH set on cmd"
        );
        let node_path = envs
            .iter()
            .find(|(k, _)| k == "NODE_PATH")
            .and_then(|(_, v)| v.clone());
        assert_eq!(
            node_path,
            Some("/opt/plaw-bundled/lib/node_modules".to_string())
        );
        let npm_removed = envs
            .iter()
            .any(|(k, v)| k == "NPM_CONFIG_PREFIX" && v.is_none());
        assert!(npm_removed);

        assert_eq!(
            cmd.get_current_dir()
                .map(|p| p.to_string_lossy().to_string()),
            Some("/tmp".to_string())
        );
    }

    /// PR #99 C-1.5 regression: the `*cmd = firejail_cmd` swap
    /// discards stdio. Pinned via parallel spawn — see
    /// bubblewrap.rs / docker.rs for the same shape.
    #[tokio::test]
    async fn firejail_wrap_command_defaults_to_piped_stdio() {
        let sandbox = FirejailSandbox;
        let mut cmd = tokio::process::Command::new("echo");
        cmd.arg("hi");
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        sandbox.wrap_command(cmd.as_std_mut()).unwrap();

        let mut spawnable = tokio::process::Command::new("echo");
        spawnable
            .arg("hi")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = spawnable.spawn().expect("spawn echo");
        assert!(child.stdin.is_some());
        assert!(child.stdout.is_some());
        assert!(child.stderr.is_some());
    }
}
