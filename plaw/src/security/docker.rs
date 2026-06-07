//! Docker sandbox (container isolation)

// Dormant: paired with security/traits.rs Sandbox trait. No active code
// path constructs a DockerSandbox today (see security/traits.rs rationale).
#![allow(dead_code)]

use crate::security::traits::Sandbox;
use std::process::Command;

/// Docker sandbox backend
#[derive(Debug, Clone)]
pub struct DockerSandbox {
    image: String,
}

impl Default for DockerSandbox {
    fn default() -> Self {
        Self {
            image: "alpine:latest".to_string(),
        }
    }
}

impl DockerSandbox {
    pub fn new() -> std::io::Result<Self> {
        if Self::is_installed() {
            Ok(Self::default())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Docker not found",
            ))
        }
    }

    pub fn with_image(image: String) -> std::io::Result<Self> {
        if Self::is_installed() {
            Ok(Self { image })
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Docker not found",
            ))
        }
    }

    pub fn probe() -> std::io::Result<Self> {
        Self::new()
    }

    fn is_installed() -> bool {
        Command::new("docker")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl Sandbox for DockerSandbox {
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()> {
        let program = cmd.get_program().to_string_lossy().to_string();
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        // Capture caller envs + cwd before rebuilding. PR #91's
        // `Sandbox::spawn_with_integrity` runs `wrap_command` AFTER
        // ShellTool populates env_clear() + env(SAFE_VARS). Unlike
        // bwrap/firejail (which inherit parent env automatically),
        // docker run does NOT propagate host env to the container —
        // each variable needs an explicit `-e KEY=VALUE` flag.
        // Without this, bundled `PATH` / `PYTHONUTF8` / etc. are
        // silently dropped at the container boundary.
        let envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|x| x.to_os_string())))
            .collect();
        let cwd = cmd.get_current_dir().map(|p| p.to_path_buf());

        let mut docker_cmd = Command::new("docker");
        docker_cmd.args([
            "run",
            "--rm",
            "--memory",
            "512m",
            "--cpus",
            "1.0",
            "--network",
            "none",
        ]);
        // Forward each `Some(value)` env to the container via `-e
        // KEY=VALUE`. `None` removes are no-ops for docker because
        // the container starts with empty env anyway. Safety: by the
        // time docker.rs receives `cmd`, ShellTool's `SAFE_ENV_VARS`
        // allowlist has already filtered secrets out.
        for (key, value) in &envs {
            if let Some(value) = value {
                docker_cmd.arg("-e");
                let mut kv = key.clone();
                kv.push("=");
                kv.push(value);
                docker_cmd.arg(kv);
            }
        }
        if let Some(d) = &cwd {
            // `-w` sets the container's working directory.
            docker_cmd.arg("-w");
            docker_cmd.arg(d);
        }
        docker_cmd.arg(&self.image);
        docker_cmd.arg(&program);
        docker_cmd.args(&args);

        // ALSO mirror the envs + cwd on the host docker_cmd, even
        // though the container ignores them. This preserves the
        // bwrap/firejail symmetry and keeps the same `*cmd = X`
        // invariant the trait expects.
        for (k, v) in envs {
            match v {
                Some(value) => {
                    docker_cmd.env(k, value);
                }
                None => {
                    docker_cmd.env_remove(k);
                }
            }
        }
        if let Some(d) = cwd {
            docker_cmd.current_dir(d);
        }

        *cmd = docker_cmd;
        Ok(())
    }

    fn is_available(&self) -> bool {
        Self::is_installed()
    }

    fn name(&self) -> &str {
        "docker"
    }

    fn description(&self) -> &str {
        "Docker container isolation (requires docker)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_sandbox_name() {
        let sandbox = DockerSandbox::default();
        assert_eq!(sandbox.name(), "docker");
    }

    #[test]
    fn docker_sandbox_default_image() {
        let sandbox = DockerSandbox::default();
        assert_eq!(sandbox.image, "alpine:latest");
    }

    #[test]
    fn docker_with_custom_image() {
        let result = DockerSandbox::with_image("ubuntu:latest".to_string());
        match result {
            Ok(sandbox) => assert_eq!(sandbox.image, "ubuntu:latest"),
            Err(_) => assert!(!DockerSandbox::is_installed()),
        }
    }

    // ── §1.1 Sandbox isolation flag tests ──────────────────────

    #[test]
    fn docker_wrap_command_includes_isolation_flags() {
        let sandbox = DockerSandbox::default();
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        sandbox.wrap_command(&mut cmd).unwrap();

        assert_eq!(
            cmd.get_program().to_string_lossy(),
            "docker",
            "wrapped command should use docker as program"
        );

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert!(
            args.contains(&"run".to_string()),
            "must include 'run' subcommand"
        );
        assert!(
            args.contains(&"--rm".to_string()),
            "must include --rm for auto-cleanup"
        );
        assert!(
            args.contains(&"--network".to_string()),
            "must include --network flag"
        );
        assert!(
            args.contains(&"none".to_string()),
            "network must be set to 'none' for isolation"
        );
        assert!(
            args.contains(&"--memory".to_string()),
            "must include --memory limit"
        );
        assert!(
            args.contains(&"512m".to_string()),
            "memory limit must be 512m"
        );
        assert!(
            args.contains(&"--cpus".to_string()),
            "must include --cpus limit"
        );
        assert!(args.contains(&"1.0".to_string()), "CPU limit must be 1.0");
    }

    #[test]
    fn docker_wrap_command_preserves_original_command() {
        let sandbox = DockerSandbox::default();
        let mut cmd = Command::new("ls");
        cmd.arg("-la");
        sandbox.wrap_command(&mut cmd).unwrap();

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert!(
            args.contains(&"alpine:latest".to_string()),
            "must include the container image"
        );
        assert!(
            args.contains(&"ls".to_string()),
            "original program must be passed as argument"
        );
        assert!(
            args.contains(&"-la".to_string()),
            "original args must be preserved"
        );
    }

    #[test]
    fn docker_wrap_command_uses_custom_image() {
        let sandbox = DockerSandbox {
            image: "ubuntu:22.04".to_string(),
        };
        let mut cmd = Command::new("echo");
        sandbox.wrap_command(&mut cmd).unwrap();

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert!(
            args.contains(&"ubuntu:22.04".to_string()),
            "must use the custom image"
        );
    }

    /// PR #91 regression pin — see bubblewrap.rs for the rationale.
    /// Docker is stricter than bwrap/firejail: the wrapper command
    /// MUST forward envs via `-e KEY=VALUE` to reach the container.
    /// This test pins that requirement.
    #[test]
    fn docker_wrap_command_forwards_envs_via_e_flags_and_sets_cwd() {
        let sandbox = DockerSandbox {
            image: "test:latest".to_string(),
        };
        let mut cmd = Command::new("python3");
        cmd.arg("-c").arg("print(42)");
        cmd.env("PATH", "/opt/plaw-bundled/bin:/usr/bin");
        cmd.env("PYTHONUTF8", "1");
        cmd.current_dir("/workspace");

        sandbox.wrap_command(&mut cmd).unwrap();

        // Docker `-e KEY=VALUE` forwarding pins:
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        // -e flags emit KEY=VALUE pairs after each -e
        let mut e_kvs: Vec<String> = Vec::new();
        let mut prev_was_e = false;
        for a in &args {
            if prev_was_e {
                e_kvs.push(a.clone());
                prev_was_e = false;
            } else if a == "-e" {
                prev_was_e = true;
            }
        }
        assert!(
            e_kvs
                .iter()
                .any(|s| s == "PATH=/opt/plaw-bundled/bin:/usr/bin"),
            "docker must forward PATH via `-e`: {e_kvs:?}"
        );
        assert!(
            e_kvs.iter().any(|s| s == "PYTHONUTF8=1"),
            "docker must forward PYTHONUTF8 via `-e`: {e_kvs:?}"
        );
        // `-w` sets container cwd.
        let w_idx = args.iter().position(|a| a == "-w");
        assert!(w_idx.is_some(), "docker must forward cwd via `-w`");
        assert_eq!(
            args.get(w_idx.unwrap() + 1).map(|s| s.as_str()),
            Some("/workspace"),
            "docker `-w` must be set to the original cwd"
        );

        // Host docker_cmd also mirrors envs (preserves symmetry; the
        // container ignores them but they don't hurt).
        let envs_on_host: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|x| x.to_string_lossy().to_string()),
                )
            })
            .collect();
        let path_on_host = envs_on_host
            .iter()
            .find(|(k, _)| k == "PATH")
            .and_then(|(_, v)| v.clone());
        assert_eq!(
            path_on_host,
            Some("/opt/plaw-bundled/bin:/usr/bin".to_string())
        );
    }
}
