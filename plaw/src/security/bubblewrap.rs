//! Bubblewrap sandbox (user namespaces for Linux/macOS)

use crate::security::traits::Sandbox;
use std::process::Command;

/// Bubblewrap sandbox backend
#[derive(Debug, Clone, Default)]
pub struct BubblewrapSandbox;

impl BubblewrapSandbox {
    pub fn new() -> std::io::Result<Self> {
        if Self::is_installed() {
            Ok(Self)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Bubblewrap not found",
            ))
        }
    }

    pub fn probe() -> std::io::Result<Self> {
        Self::new()
    }

    fn is_installed() -> bool {
        Command::new("bwrap")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl Sandbox for BubblewrapSandbox {
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()> {
        let program = cmd.get_program().to_os_string();
        let args: Vec<std::ffi::OsString> = cmd.get_args().map(|s| s.to_os_string()).collect();

        // Capture envs + cwd BEFORE rebuilding the command. PR #91's
        // `Sandbox::spawn_with_integrity` runs `wrap_command` AFTER
        // ShellTool populates `env_clear() + env(SAFE_VARS)`; pre-#91
        // the order was reversed and the wrap landed first. The
        // `*cmd = bwrap_cmd` swap below would otherwise discard the
        // carefully-built `PATH` / `PYTHONUTF8` / `PLAYWRIGHT_BROWSERS_PATH`
        // / etc. — bundled-tool spawn breakage for users with
        // `[security.sandbox] backend = "bubblewrap"`.
        let envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|x| x.to_os_string())))
            .collect();
        let cwd = cmd.get_current_dir().map(|p| p.to_path_buf());

        let mut bwrap_cmd = Command::new("bwrap");
        bwrap_cmd.args([
            "--ro-bind",
            "/usr",
            "/usr",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--bind",
            "/tmp",
            "/tmp",
            "--unshare-all",
            "--die-with-parent",
        ]);
        bwrap_cmd.arg(&program);
        bwrap_cmd.args(&args);

        // Restore caller-built envs (preserves env_clear + explicit
        // env() + explicit env_remove() semantics from the original).
        for (k, v) in envs {
            match v {
                Some(value) => {
                    bwrap_cmd.env(k, value);
                }
                None => {
                    bwrap_cmd.env_remove(k);
                }
            }
        }
        if let Some(d) = cwd {
            bwrap_cmd.current_dir(d);
        }

        *cmd = bwrap_cmd;
        Ok(())
    }

    fn is_available(&self) -> bool {
        Self::is_installed()
    }

    fn name(&self) -> &str {
        "bubblewrap"
    }

    fn description(&self) -> &str {
        "User namespace sandbox (requires bwrap)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubblewrap_sandbox_name() {
        let sandbox = BubblewrapSandbox;
        assert_eq!(sandbox.name(), "bubblewrap");
    }

    #[test]
    fn bubblewrap_is_available_only_if_installed() {
        // Result depends on whether bwrap is installed
        let sandbox = BubblewrapSandbox;
        let _available = sandbox.is_available();

        // Either way, the name should still work
        assert_eq!(sandbox.name(), "bubblewrap");
    }

    // ── §1.1 Sandbox isolation flag tests ──────────────────────

    #[test]
    fn bubblewrap_wrap_command_includes_isolation_flags() {
        let sandbox = BubblewrapSandbox;
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        sandbox.wrap_command(&mut cmd).unwrap();

        assert_eq!(
            cmd.get_program().to_string_lossy(),
            "bwrap",
            "wrapped command should use bwrap as program"
        );

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert!(
            args.contains(&"--unshare-all".to_string()),
            "must include --unshare-all for namespace isolation"
        );
        assert!(
            args.contains(&"--die-with-parent".to_string()),
            "must include --die-with-parent to prevent orphan processes"
        );
        assert!(
            !args.contains(&"--share-net".to_string()),
            "must NOT include --share-net (network should be blocked)"
        );
    }

    #[test]
    fn bubblewrap_wrap_command_preserves_original_command() {
        let sandbox = BubblewrapSandbox;
        let mut cmd = Command::new("ls");
        cmd.arg("-la");
        cmd.arg("/tmp");
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
            args.contains(&"/tmp".to_string()),
            "original args must be preserved"
        );
    }

    #[test]
    fn bubblewrap_wrap_command_binds_required_paths() {
        let sandbox = BubblewrapSandbox;
        let mut cmd = Command::new("echo");
        sandbox.wrap_command(&mut cmd).unwrap();

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert!(
            args.contains(&"--ro-bind".to_string()),
            "must include read-only bind for /usr"
        );
        assert!(
            args.contains(&"--dev".to_string()),
            "must include /dev mount"
        );
        assert!(
            args.contains(&"--proc".to_string()),
            "must include /proc mount"
        );
    }

    /// PR #91 regression pin: ShellTool sets `PATH` / `PYTHONUTF8` /
    /// `PLAYWRIGHT_BROWSERS_PATH` etc. on `cmd` BEFORE calling
    /// `spawn_with_integrity` (which routes through `wrap_command` in
    /// the default trait impl). The pre-PR-#91 order was reversed —
    /// `wrap_command` ran first, so envs landed on the wrapper
    /// command after the swap. Post-#91 the wrap discards envs by
    /// the `*cmd = bwrap_cmd` swap. If this test fails, bundled
    /// python / node / pandoc / poppler tools won't be found inside
    /// the sandbox.
    #[test]
    fn bubblewrap_wrap_command_preserves_envs_and_cwd() {
        let sandbox = BubblewrapSandbox;
        let mut cmd = Command::new("python3");
        cmd.arg("-c").arg("import sys; print(sys.path)");
        cmd.env("PATH", "/opt/plaw-bundled/bin:/usr/bin");
        cmd.env("PYTHONUTF8", "1");
        cmd.env("PLAW_TEST_KEY", "marker");
        cmd.env_remove("PYTHONPATH"); // should survive as a removal
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
            "wrap_command must preserve PATH set on the original cmd; \
             losing it means bundled tools aren't found inside bwrap"
        );
        let pyutf8 = envs
            .iter()
            .find(|(k, _)| k == "PYTHONUTF8")
            .and_then(|(_, v)| v.clone());
        assert_eq!(pyutf8, Some("1".to_string()));
        let marker = envs
            .iter()
            .find(|(k, _)| k == "PLAW_TEST_KEY")
            .and_then(|(_, v)| v.clone());
        assert_eq!(marker, Some("marker".to_string()));

        // env_remove("PYTHONPATH") must come through as a None entry.
        let pypath_removed = envs.iter().any(|(k, v)| k == "PYTHONPATH" && v.is_none());
        assert!(
            pypath_removed,
            "env_remove(PYTHONPATH) on original cmd must survive wrap"
        );

        assert_eq!(
            cmd.get_current_dir()
                .map(|p| p.to_string_lossy().to_string()),
            Some("/tmp".to_string()),
            "wrap_command must preserve cwd set on the original cmd"
        );
    }
}
