use super::traits::{Tool, ToolResult};
use crate::runtime::RuntimeAdapter;
use crate::security::SecurityPolicy;
use crate::security::SyscallAnomalyDetector;
use async_trait::async_trait;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Default shell command execution time before kill (5 minutes).
/// Install commands (npm/pnpm/pip/cargo) can easily exceed 60s.
const SHELL_TIMEOUT_SECS: u64 = 300;

/// Maximum allowed timeout that AI can request (10 minutes).
const SHELL_MAX_TIMEOUT_SECS: u64 = 600;

/// Cached login environment, resolved once on first use.
static LOGIN_ENV: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Resolve the full login environment like VSCode does:
/// - Windows: read all env vars from registry (system + user), merge PATH
/// - Unix: spawn a login shell (`$SHELL -l -c env`) to capture full profile
///
/// This ensures shell commands can find tools the user has installed
/// (node, npm, python, java, etc.) even when Plaw is started from
/// a GUI process (Tauri) that inherits an incomplete environment.
fn resolve_login_env() -> &'static HashMap<String, String> {
    LOGIN_ENV.get_or_init(|| {
        let env = resolve_login_env_inner();
        if env.is_empty() {
            // Fallback: use current process env
            std::env::vars().collect()
        } else {
            env
        }
    })
}

fn resolve_login_env_inner() -> HashMap<String, String> {
    // Mobile platforms have no user-installed CLI tools — skip resolution
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        return HashMap::new();
    }

    #[cfg(windows)]
    {
        resolve_windows_registry_env()
    }

    #[cfg(all(not(windows), not(target_os = "android"), not(target_os = "ios")))]
    {
        resolve_unix_login_env()
    }
}

/// Windows: read all environment variables from registry.
/// GUI apps (Tauri/Explorer) often inherit a stale snapshot that misses
/// recently installed tools. The registry is the authoritative source.
#[cfg(windows)]
fn resolve_windows_registry_env() -> HashMap<String, String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let mut env: HashMap<String, String> = HashMap::new();
    let mut sys_path = String::new();
    let mut user_path = String::new();

    // System environment variables
    if let Ok(sys_env) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
    {
        for (name, _) in sys_env.enum_values().filter_map(|r| r.ok()) {
            if let Ok(val) = sys_env.get_value::<String, _>(&name) {
                if name.eq_ignore_ascii_case("Path") {
                    sys_path = val;
                } else {
                    env.insert(name, val);
                }
            }
        }
    }

    // User environment variables (override system for non-PATH vars)
    if let Ok(user_env) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Environment") {
        for (name, _) in user_env.enum_values().filter_map(|r| r.ok()) {
            if let Ok(val) = user_env.get_value::<String, _>(&name) {
                if name.eq_ignore_ascii_case("Path") {
                    user_path = val;
                } else {
                    env.insert(name, val);
                }
            }
        }
    }

    // Merge PATH: system paths first, then user paths, deduplicated
    let mut path_parts: Vec<String> = Vec::new();
    for p in sys_path
        .split(';')
        .chain(user_path.split(';'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !path_parts.iter().any(|x| x.eq_ignore_ascii_case(p)) {
            path_parts.push(p.to_string());
        }
    }
    if !path_parts.is_empty() {
        env.insert("PATH".to_string(), path_parts.join(";"));
    }

    env
}

/// Unix/macOS: spawn a login shell to capture the full environment,
/// including vars set in .bashrc/.zshrc/.profile (nvm, pyenv, etc.).
#[cfg(not(windows))]
fn resolve_unix_login_env() -> HashMap<String, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "env"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut env = HashMap::new();
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if !k.is_empty() {
                        env.insert(k.to_string(), v.to_string());
                    }
                }
            }
            env
        }
        _ => HashMap::new(),
    }
}
/// Maximum output size in bytes (1MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;
/// Environment variables safe to pass to shell commands.
/// Only functional variables are included — never API keys or secrets.
/// Windows requires SystemRoot/WINDIR/TEMP/TMP/USERPROFILE/APPDATA etc.
/// for basic program execution (PowerShell, cmd.exe, crypto subsystem).
const SAFE_ENV_VARS: &[&str] = &[
    // Cross-platform essentials
    "PATH",
    "HOME",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "USER",
    "SHELL",
    "TMPDIR",
    // Windows-essential: without these, PowerShell/cmd fail with cryptic errors
    "SystemRoot",
    "SYSTEMDRIVE",
    "WINDIR",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "CommonProgramFiles",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "CHCP",
    // PATHEXT is critical: without it, Windows/PowerShell cannot resolve
    // `python` → `python.exe` (or any extension-less command name).
    "PATHEXT",
    // Development tools — paths, not secrets
    "JAVA_HOME",
    "JRE_HOME",
    "GOPATH",
    "GOROOT",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "NVM_DIR",
    "NODE_PATH",
    "NPM_CONFIG_PREFIX",
    "PYTHONPATH",
    "PYTHONHOME",
    "VIRTUAL_ENV",
    "CONDA_DEFAULT_ENV",
    "PYTHONIOENCODING",
    "PYTHONUTF8",
    "ANDROID_HOME",
    "ANDROID_SDK_ROOT",
    "DOTNET_ROOT",
    "GRADLE_HOME",
    "MAVEN_HOME",
    "RUBY_HOME",
    "GEM_HOME",
    "GEM_PATH",
    // macOS
    "HOMEBREW_PREFIX",
    "HOMEBREW_CELLAR",
    // XDG (Linux/macOS)
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
];

/// Shell command execution tool with sandboxing
pub struct ShellTool {
    security: Arc<SecurityPolicy>,
    runtime: Arc<dyn RuntimeAdapter>,
    sandbox: Arc<dyn crate::security::Sandbox>,
    syscall_detector: Option<Arc<SyscallAnomalyDetector>>,
    /// PR #91 Phase 1c: Token IL applied to spawned children. Default
    /// = no lowering (byte-identical to pre-#91 behavior). Set via
    /// [`Self::with_integrity_level`] from the
    /// `[security.sandbox.integrity]` config — populated only by
    /// `tools::all_tools_impl`; test callers stick with the
    /// constructor default and never need to change.
    integrity_level: crate::security::traits::IntegrityLevel,
}

impl ShellTool {
    pub fn new(
        security: Arc<SecurityPolicy>,
        runtime: Arc<dyn RuntimeAdapter>,
        sandbox: Arc<dyn crate::security::Sandbox>,
    ) -> Self {
        Self::new_with_syscall_detector(security, runtime, sandbox, None)
    }

    pub fn new_with_syscall_detector(
        security: Arc<SecurityPolicy>,
        runtime: Arc<dyn RuntimeAdapter>,
        sandbox: Arc<dyn crate::security::Sandbox>,
        syscall_detector: Option<Arc<SyscallAnomalyDetector>>,
    ) -> Self {
        Self {
            security,
            runtime,
            sandbox,
            syscall_detector,
            integrity_level: crate::security::traits::IntegrityLevel::Default,
        }
    }

    /// PR #91 Phase 1c: builder method opting this `ShellTool` into a
    /// non-`Default` mandatory integrity level for spawned children.
    /// Defaulting via a builder (rather than a 5th positional ctor
    /// parameter) means the existing ~20 `ShellTool::new` test
    /// callers stay unchanged — only the production wiring in
    /// `tools::all_tools_impl` needs to opt in.
    pub fn with_integrity_level(mut self, level: crate::security::traits::IntegrityLevel) -> Self {
        self.integrity_level = level;
        self
    }
}

fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub(super) fn collect_allowed_shell_env_vars(security: &SecurityPolicy) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for key in SAFE_ENV_VARS
        .iter()
        .copied()
        .chain(security.shell_env_passthrough.iter().map(|s| s.as_str()))
    {
        let candidate = key.trim();
        if candidate.is_empty() || !is_valid_env_var_name(candidate) {
            continue;
        }
        if seen.insert(candidate.to_string()) {
            out.push(candidate.to_string());
        }
    }
    out
}

fn extract_command_argument(args: &serde_json::Value) -> Option<String> {
    if let Some(command) = args
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|cmd| !cmd.is_empty())
    {
        return Some(command.to_string());
    }

    for alias in [
        "cmd",
        "script",
        "shell_command",
        "command_line",
        "bash",
        "sh",
        "input",
    ] {
        if let Some(command) = args
            .get(alias)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|cmd| !cmd.is_empty())
        {
            return Some(command.to_string());
        }
    }

    args.as_str()
        .map(str::trim)
        .filter(|cmd| !cmd.is_empty())
        .map(ToString::to_string)
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the workspace directory"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "approved": {
                    "type": "boolean",
                    "description": "Set true to explicitly approve medium/high-risk commands in supervised mode",
                    "default": false
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 300, max 600). Use higher values for install commands (npm/pnpm/pip/cargo install).",
                    "default": 300,
                    "minimum": 1,
                    "maximum": 600
                }
            },
            "required": ["command"]
        })
    }

    fn idempotency(&self) -> super::traits::Idempotency {
        // Shell commands can do anything — observable result is not safe
        // to assume reproducible across calls.
        super::traits::Idempotency::NonIdempotent
    }

    fn side_effects(&self) -> super::traits::SideEffectClass {
        super::traits::SideEffectClass::LocalExecute
    }

    #[allow(clippy::incompatible_msrv)]
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let command = extract_command_argument(&args)
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;
        let approved = args
            .get("approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let timeout_secs = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(SHELL_TIMEOUT_SECS)
            .min(SHELL_MAX_TIMEOUT_SECS)
            .max(1);

        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        match self.security.validate_command_execution(&command, approved) {
            Ok(_) => {}
            Err(reason) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(reason),
                });
            }
        }

        if let Some(path) = self.security.forbidden_path_argument(&command) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path blocked by security policy: {path}")),
            });
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        // Execute with timeout to prevent hanging commands.
        // Clear the environment to prevent leaking API keys and other secrets
        // (CWE-200), then re-add only safe, functional variables.
        let mut cmd = match self
            .runtime
            .build_shell_command(&command, &self.security.workspace_dir)
        {
            Ok(cmd) => cmd,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to build runtime command: {e}")),
                });
            }
        };

        // PR #91 Phase 1c: sandbox wrapping (firejail / bubblewrap /
        // landlock / Windows Job Object) now runs INSIDE
        // `Sandbox::spawn_with_integrity` below, alongside the spawn
        // itself + the post-spawn Job Object assignment. Calling
        // `wrap_command` here would double-prepend wrapper args for
        // firejail-style backends — the trait owns the choreography
        // now. NoopSandbox's wrap_command (the default until backend
        // wiring is chosen) is a no-op so this change is byte-
        // identical for the common case.

        cmd.env_clear();

        // Source env vars from login environment (registry on Windows,
        // login shell on Unix) instead of the possibly-incomplete GUI
        // process env. Still filtered through the allowlist for security.
        let login_env = resolve_login_env();
        for var in collect_allowed_shell_env_vars(&self.security) {
            // Try login env first (case-insensitive on Windows), fall back to process env
            let val = login_env
                .get(&var)
                .or_else(|| {
                    login_env
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(&var))
                        .map(|(_, v)| v)
                })
                .cloned()
                .or_else(|| std::env::var(&var).ok());
            if let Some(v) = val {
                cmd.env(&var, v);
            }
        }

        // Force UTF-8 encoding for child processes on Windows.
        // Python in pipe mode defaults to the system codepage (GBK on Chinese Windows),
        // causing garbled output when decoded with from_utf8_lossy.
        #[cfg(target_os = "windows")]
        {
            cmd.env("PYTHONUTF8", "1");
            cmd.env("PYTHONIOENCODING", "utf-8");
        }

        // Inject the plaw binary directory AND bundled tool directories into PATH.
        // Plaw binary lives at <data_root>/bin/plaw.exe, so data_root = exe_dir.parent().
        // Bundled tools: python/, python/Scripts/, node/, pandoc/, poppler/, bin/,
        // libreoffice/libreoffice/program/.
        {
            let current_path = login_env
                .get("PATH")
                .or_else(|| login_env.get("Path"))
                .cloned()
                .unwrap_or_default();
            let sep = if cfg!(windows) { ";" } else { ":" };
            let mut extra_dirs: Vec<String> = Vec::new();

            if let Ok(exe) = std::env::current_exe() {
                if let Some(exe_dir) = exe.parent() {
                    let exe_dir_str = exe_dir.to_string_lossy().to_string();
                    if !current_path.contains(&exe_dir_str) {
                        extra_dirs.push(exe_dir_str);
                    }

                    // data_root is the parent of bin/ (exe_dir)
                    if let Some(data_root) = exe_dir.parent() {
                        let candidates = [
                            "pwsh",
                            "python",
                            "python/Scripts",
                            "pandoc",
                            "poppler",
                            "node",
                        ];
                        for c in &candidates {
                            let p = data_root.join(c);
                            if p.is_dir() {
                                let s = p.to_string_lossy().to_string();
                                if !current_path.contains(&s) {
                                    extra_dirs.push(s);
                                }
                            }
                        }
                        // LibreOffice has a nested path
                        let lo = data_root
                            .join("libreoffice")
                            .join("libreoffice")
                            .join("program");
                        if lo.is_dir() {
                            let s = lo.to_string_lossy().to_string();
                            if !current_path.contains(&s) {
                                extra_dirs.push(s);
                            }
                        }
                    }
                }
            }

            // Also try $HOME as data_root fallback (Plaw sets HOME=plaw-data/)
            if let Ok(home) = std::env::var("HOME") {
                let home_path = std::path::Path::new(&home);
                let candidates = [
                    "pwsh",
                    "python",
                    "python/Scripts",
                    "pandoc",
                    "poppler",
                    "node",
                    "bin",
                ];
                for c in &candidates {
                    let p = home_path.join(c);
                    if p.is_dir() {
                        let s = p.to_string_lossy().to_string();
                        if !current_path.contains(&s) && !extra_dirs.contains(&s) {
                            extra_dirs.push(s);
                        }
                    }
                }
                let lo = home_path
                    .join("libreoffice")
                    .join("libreoffice")
                    .join("program");
                if lo.is_dir() {
                    let s = lo.to_string_lossy().to_string();
                    if !current_path.contains(&s) && !extra_dirs.contains(&s) {
                        extra_dirs.push(s);
                    }
                }
            }

            if !extra_dirs.is_empty() {
                cmd.env(
                    "PATH",
                    format!("{}{sep}{current_path}", extra_dirs.join(sep)),
                );
            }

            // Also set NODE_PATH for bundled node_modules if available,
            // and PLAYWRIGHT_BROWSERS_PATH / CHROMIUM_EXECUTABLE_PATH for html2pptx.
            if let Ok(home) = std::env::var("HOME") {
                let home_path = std::path::Path::new(&home);
                let nm = home_path.join("node_modules_global").join("node_modules");
                if nm.is_dir() {
                    cmd.env("NODE_PATH", nm.to_string_lossy().to_string());
                }

                // Playwright: set browsers path + direct executable path for headless shell
                let browsers_dir = home_path.join("browsers");
                if browsers_dir.is_dir() {
                    cmd.env(
                        "PLAYWRIGHT_BROWSERS_PATH",
                        browsers_dir.to_string_lossy().to_string(),
                    );
                    // Find the headless shell binary for direct executablePath usage
                    if let Ok(entries) = std::fs::read_dir(&browsers_dir) {
                        for entry in entries.flatten() {
                            let name = entry.file_name();
                            let name_str = name.to_string_lossy();
                            if name_str.starts_with("chromium_headless_shell-") {
                                #[cfg(windows)]
                                let exe = entry
                                    .path()
                                    .join("chrome-headless-shell-win64")
                                    .join("chrome-headless-shell.exe");
                                #[cfg(not(windows))]
                                let exe = entry
                                    .path()
                                    .join("chrome-headless-shell-linux")
                                    .join("chrome-headless-shell");
                                if exe.is_file() {
                                    cmd.env(
                                        "CHROMIUM_EXECUTABLE_PATH",
                                        exe.to_string_lossy().to_string(),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Spawn (rather than cmd.output()) so we can run the sandbox
        // post-spawn hook between spawn and wait. Windows Job Object needs
        // `AssignProcessToJobObject(job, pid)` after the process exists;
        // there is no pre-spawn equivalent. NoopSandbox and Linux/macOS
        // sandboxes use the default no-op `after_spawn` so this hook is
        // free for them. See `security::traits::Sandbox::after_spawn` docs.
        //
        // `cmd.output()` would auto-pipe stdout/stderr for us, but `spawn()`
        // inherits the parent's streams by default — set Piped explicitly
        // so wait_with_output() can capture the bytes for ToolResult.
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null());

        // PR #91 Phase 1c: route the spawn through the trait method
        // shipped in PR #90. The default impl handles wrap_command +
        // tokio spawn + after_spawn in one call; WindowsJobObjectSandbox
        // additionally routes non-`Default` integrity levels through
        // PR #89's Token IL spawn primitives.
        //
        // At `integrity_level = Default` (everyone today, by default),
        // the call sequence is byte-identical to pre-#91:
        // wrap_command → cmd.spawn → after_spawn(pid).
        //
        // At a non-`Default` level: WindowsJobObjectSandbox returns
        // `SandboxedChild::Lowered`, spawned via PR #103's piped Token
        // IL primitive. Phase 1c.2c: `SandboxedChild::wait_with_output`
        // captures stdout/stderr identically for both the Tokio and the
        // Lowered variant, so there is no longer a deferred-feature bail
        // here.
        //
        // NOTE: at a lowered IL the child runs with kernel-enforced
        // integrity restrictions — e.g. Low IL DENIES writes to
        // Medium-labeled locations (most temp dirs and the workspace).
        // That is the POINT of the sandbox, opt-in via
        // `[security.sandbox.integrity] shell = "..."` (default off), but
        // operators should expect commands that write outside Low-labeled
        // dirs to fail with access-denied. See docs/sandboxing.md.
        let sandboxed = match self
            .sandbox
            .spawn_with_integrity(cmd, self.integrity_level)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Failed to spawn under sandbox '{}': {e}",
                        self.sandbox.name()
                    )),
                });
            }
        };

        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            sandboxed.wait_with_output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

                // Truncate output to prevent OOM
                if stdout.len() > MAX_OUTPUT_BYTES {
                    stdout.truncate(crate::util::floor_utf8_char_boundary(
                        &stdout,
                        MAX_OUTPUT_BYTES,
                    ));
                    stdout.push_str("\n... [output truncated at 1MB]");
                }
                if stderr.len() > MAX_OUTPUT_BYTES {
                    stderr.truncate(crate::util::floor_utf8_char_boundary(
                        &stderr,
                        MAX_OUTPUT_BYTES,
                    ));
                    stderr.push_str("\n... [stderr truncated at 1MB]");
                }

                if let Some(detector) = &self.syscall_detector {
                    let _ = detector.inspect_command_output(
                        &command,
                        &stdout,
                        &stderr,
                        output.status.code(),
                    );
                }

                Ok(ToolResult {
                    success: output.status.success(),
                    output: stdout,
                    error: if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    },
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to execute command: {e}")),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Command timed out after {timeout_secs}s and was killed. For install commands, retry with a higher timeout value."
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuditConfig, SyscallAnomalyConfig};
    use crate::runtime::{NativeRuntime, RuntimeAdapter};
    use crate::security::{AutonomyLevel, SecurityPolicy, SyscallAnomalyDetector};
    use tempfile::TempDir;

    fn test_security(autonomy: AutonomyLevel) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    fn test_runtime() -> Arc<dyn RuntimeAdapter> {
        Arc::new(NativeRuntime::new())
    }

    fn test_sandbox() -> Arc<dyn crate::security::Sandbox> {
        Arc::new(crate::security::NoopSandbox)
    }

    fn test_syscall_detector(tmp: &TempDir) -> Arc<SyscallAnomalyDetector> {
        let log_path = tmp.path().join("shell-syscall-anomalies.log");
        let cfg = SyscallAnomalyConfig {
            baseline_syscalls: vec!["read".into(), "write".into()],
            log_path: log_path.to_string_lossy().to_string(),
            alert_cooldown_secs: 1,
            max_alerts_per_minute: 50,
            ..SyscallAnomalyConfig::default()
        };
        let audit = AuditConfig {
            enabled: false,
            ..AuditConfig::default()
        };
        Arc::new(SyscallAnomalyDetector::new(cfg, tmp.path(), audit))
    }

    #[test]
    fn shell_tool_name() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        assert_eq!(tool.name(), "shell");
    }

    #[tokio::test]
    async fn shell_invokes_sandbox_wrap_command_on_execute() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct CountingSandbox {
            count: Arc<AtomicU32>,
        }

        impl crate::security::Sandbox for CountingSandbox {
            fn wrap_command(&self, _cmd: &mut std::process::Command) -> std::io::Result<()> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            fn is_available(&self) -> bool {
                true
            }
            fn name(&self) -> &str {
                "counting"
            }
            fn description(&self) -> &str {
                "test sandbox that counts wrap_command invocations"
            }
        }

        let count = Arc::new(AtomicU32::new(0));
        let sandbox: Arc<dyn crate::security::Sandbox> = Arc::new(CountingSandbox {
            count: count.clone(),
        });
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            sandbox,
        );

        // Doesn't matter if the command itself succeeds or fails — we only
        // need to confirm execute reached the wrap_command injection point.
        // (`echo` works on both Windows cmd and Unix sh.)
        let _ = tool
            .execute(json!({"command": "echo sandbox-wire-smoke"}))
            .await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "ShellTool::execute must invoke sandbox.wrap_command exactly once"
        );
    }

    #[test]
    fn shell_tool_description() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn shell_tool_schema_has_command() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["command"].is_object());
        assert!(schema["required"]
            .as_array()
            .expect("schema required field should be an array")
            .contains(&json!("command")));
        assert!(schema["properties"]["approved"].is_object());
    }

    #[test]
    fn extract_command_argument_supports_aliases() {
        assert_eq!(
            extract_command_argument(&json!({"cmd": "echo from-cmd"})).as_deref(),
            Some("echo from-cmd")
        );
        assert_eq!(
            extract_command_argument(&json!({"script": "echo from-script"})).as_deref(),
            Some("echo from-script")
        );
        assert_eq!(
            extract_command_argument(&json!("echo from-string")).as_deref(),
            Some("echo from-string")
        );
    }

    #[tokio::test]
    async fn shell_executes_allowed_command() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "echo hello"}))
            .await
            .expect("echo command execution should succeed");
        assert!(result.success);
        assert!(result.output.trim().contains("hello"));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn shell_executes_command_from_cmd_alias() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"cmd": "echo alias"}))
            .await
            .expect("cmd alias execution should succeed");
        assert!(result.success);
        assert!(result.output.trim().contains("alias"));
    }

    #[tokio::test]
    async fn shell_blocks_disallowed_command() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "rm -rf /"}))
            .await
            .expect("disallowed command execution should return a result");
        assert!(!result.success);
        let error = result.error.as_deref().unwrap_or("");
        assert!(error.contains("not allowed") || error.contains("high-risk"));
    }

    #[tokio::test]
    async fn shell_blocks_readonly() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::ReadOnly),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "ls"}))
            .await
            .expect("readonly command execution should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .expect("error field should be present for blocked command")
            .contains("not allowed"));
    }

    #[tokio::test]
    async fn shell_missing_command_param() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("command"));
    }

    #[tokio::test]
    async fn shell_wrong_type_param() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool.execute(json!({"command": 123})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn shell_captures_exit_code() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "ls /nonexistent_dir_xyz"}))
            .await
            .expect("command with nonexistent path should return a result");
        assert!(!result.success);
    }

    #[tokio::test]
    async fn shell_blocks_absolute_path_argument() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "cat /etc/passwd"}))
            .await
            .expect("absolute path argument should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_option_assignment_path_argument() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "grep --file=/etc/passwd root ./src"}))
            .await
            .expect("option-assigned forbidden path should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_short_option_attached_path_argument() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "grep -f/etc/passwd root ./src"}))
            .await
            .expect("short option attached forbidden path should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_tilde_user_path_argument() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "cat ~root/.ssh/id_rsa"}))
            .await
            .expect("tilde-user path should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_input_redirection_path_bypass() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "cat </etc/passwd"}))
            .await
            .expect("input redirection bypass should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("not allowed"));
    }

    fn test_security_with_env_cmd() -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: std::env::temp_dir(),
            allowed_commands: vec!["env".into(), "echo".into()],
            ..SecurityPolicy::default()
        })
    }

    fn test_security_with_env_passthrough(vars: &[&str]) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: std::env::temp_dir(),
            allowed_commands: vec!["env".into()],
            shell_env_passthrough: vars.iter().map(|v| (*v).to_string()).collect(),
            ..SecurityPolicy::default()
        })
    }

    /// RAII guard that restores an environment variable to its original state on drop,
    /// ensuring cleanup even if the test panics.
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(val) => std::env::set_var(self.key, val),
                None => std::env::remove_var(self.key),
            }
        }
    }

    // Unix-only: uses `env` command (cmd.exe / PowerShell don't expose it
    // identically; Windows equivalent would be `set` / `Get-ChildItem env:`).
    // Tracked under F-1.5; production env-leakage protection applies on
    // both platforms regardless of this test's coverage gap.
    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn shell_does_not_leak_api_key() {
        let _g1 = EnvGuard::set("API_KEY", "sk-test-secret-12345");
        let _g2 = EnvGuard::set("PLAW_API_KEY", "sk-test-secret-67890");

        let tool = ShellTool::new(test_security_with_env_cmd(), test_runtime(), test_sandbox());
        let result = tool
            .execute(json!({"command": "env"}))
            .await
            .expect("env command execution should succeed");
        assert!(result.success);
        assert!(
            !result.output.contains("sk-test-secret-12345"),
            "API_KEY leaked to shell command output"
        );
        assert!(
            !result.output.contains("sk-test-secret-67890"),
            "PLAW_API_KEY leaked to shell command output"
        );
    }

    #[cfg(unix)] // uses `env` command — see shell_does_not_leak_api_key
    #[tokio::test]
    async fn shell_preserves_path_and_home_for_env_command() {
        let tool = ShellTool::new(test_security_with_env_cmd(), test_runtime(), test_sandbox());

        let result = tool
            .execute(json!({"command": "env"}))
            .await
            .expect("env command should succeed");
        assert!(result.success);
        assert!(
            result.output.contains("HOME="),
            "HOME should be available in shell environment"
        );
        assert!(
            result.output.contains("PATH="),
            "PATH should be available in shell environment"
        );
    }

    #[tokio::test]
    async fn shell_blocks_plain_variable_expansion() {
        let tool = ShellTool::new(test_security_with_env_cmd(), test_runtime(), test_sandbox());
        let result = tool
            .execute(json!({"command": "echo $HOME"}))
            .await
            .expect("plain variable expansion should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("not allowed"));
    }

    #[cfg(unix)] // uses `env` command — see shell_does_not_leak_api_key
    #[tokio::test(flavor = "current_thread")]
    async fn shell_allows_configured_env_passthrough() {
        let _guard = EnvGuard::set("PLAW_TEST_PASSTHROUGH", "db://unit-test");
        let tool = ShellTool::new(
            test_security_with_env_passthrough(&["PLAW_TEST_PASSTHROUGH"]),
            test_runtime(),
        );

        let result = tool
            .execute(json!({"command": "env"}))
            .await
            .expect("env command execution should succeed");
        assert!(result.success);
        assert!(result
            .output
            .contains("PLAW_TEST_PASSTHROUGH=db://unit-test"));
    }

    #[test]
    fn invalid_shell_env_passthrough_names_are_filtered() {
        let security = SecurityPolicy {
            shell_env_passthrough: vec![
                "VALID_NAME".into(),
                "BAD-NAME".into(),
                "1NOPE".into(),
                "ALSO_VALID".into(),
            ],
            ..SecurityPolicy::default()
        };
        let vars = collect_allowed_shell_env_vars(&security);
        assert!(vars.contains(&"VALID_NAME".to_string()));
        assert!(vars.contains(&"ALSO_VALID".to_string()));
        assert!(!vars.contains(&"BAD-NAME".to_string()));
        assert!(!vars.contains(&"1NOPE".to_string()));
    }

    #[cfg(unix)] // uses `touch` (Unix) — Windows equivalent is `New-Item`
    #[tokio::test]
    async fn shell_requires_approval_for_medium_risk_command() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            allowed_commands: vec!["touch".into()],
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });

        let tool = ShellTool::new(security.clone(), test_runtime(), test_sandbox());
        let denied = tool
            .execute(json!({"command": "touch plaw_shell_approval_test"}))
            .await
            .expect("unapproved command should return a result");
        assert!(!denied.success);
        assert!(denied
            .error
            .as_deref()
            .unwrap_or("")
            .contains("explicit approval"));

        let allowed = tool
            .execute(json!({
                "command": "touch plaw_shell_approval_test",
                "approved": true
            }))
            .await
            .expect("approved command execution should succeed");
        assert!(allowed.success);

        let _ = tokio::fs::remove_file(std::env::temp_dir().join("plaw_shell_approval_test")).await;
    }

    // ── §5.2 Shell timeout enforcement tests ─────────────────

    #[test]
    fn shell_timeout_constant_is_reasonable() {
        assert_eq!(SHELL_TIMEOUT_SECS, 300, "shell timeout must be 300 seconds");
    }

    #[test]
    fn shell_output_limit_is_1mb() {
        assert_eq!(
            MAX_OUTPUT_BYTES, 1_048_576,
            "max output must be 1 MB to prevent OOM"
        );
    }

    // ── §5.3 Non-UTF8 binary output tests ────────────────────

    #[test]
    fn shell_safe_env_vars_excludes_secrets() {
        for var in SAFE_ENV_VARS {
            let lower = var.to_lowercase();
            assert!(
                !lower.contains("key") && !lower.contains("secret") && !lower.contains("token"),
                "SAFE_ENV_VARS must not include sensitive variable: {var}"
            );
        }
    }

    #[test]
    fn shell_safe_env_vars_includes_essentials() {
        assert!(
            SAFE_ENV_VARS.contains(&"PATH"),
            "PATH must be in safe env vars"
        );
        assert!(
            SAFE_ENV_VARS.contains(&"HOME"),
            "HOME must be in safe env vars"
        );
        assert!(
            SAFE_ENV_VARS.contains(&"TERM"),
            "TERM must be in safe env vars"
        );
    }

    #[tokio::test]
    async fn shell_blocks_rate_limited() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            max_actions_per_hour: 0,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = ShellTool::new(security, test_runtime(), test_sandbox());
        let result = tool
            .execute(json!({"command": "echo test"}))
            .await
            .expect("rate-limited command should return a result");
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("Rate limit"));
    }

    #[tokio::test]
    async fn shell_handles_nonexistent_command() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = ShellTool::new(security, test_runtime(), test_sandbox());
        let result = tool
            .execute(json!({"command": "nonexistent_binary_xyz_12345"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn shell_captures_stderr_output() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Full),
            test_runtime(),
            test_sandbox(),
        );
        let result = tool
            .execute(json!({"command": "echo error_msg >&2"}))
            .await
            .unwrap();
        assert!(result.error.as_deref().unwrap_or("").contains("error_msg"));
    }

    #[tokio::test]
    async fn shell_record_action_budget_exhaustion() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            max_actions_per_hour: 1,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = ShellTool::new(security, test_runtime(), test_sandbox());

        let r1 = tool
            .execute(json!({"command": "echo first"}))
            .await
            .unwrap();
        assert!(r1.success);

        let r2 = tool
            .execute(json!({"command": "echo second"}))
            .await
            .unwrap();
        assert!(!r2.success);
        assert!(
            r2.error.as_deref().unwrap_or("").contains("Rate limit")
                || r2.error.as_deref().unwrap_or("").contains("budget")
        );
    }

    #[tokio::test]
    async fn shell_syscall_detector_writes_anomaly_log() {
        let tmp = tempfile::tempdir().expect("temp dir should be created");
        let log_path = tmp.path().join("shell-syscall-anomalies.log");
        let detector = test_syscall_detector(&tmp);
        let tool = ShellTool::new_with_syscall_detector(
            test_security(AutonomyLevel::Full),
            test_runtime(),
            test_sandbox(),
            Some(detector),
        );

        let result = tool
            .execute(json!({"command": "echo seccomp denied syscall=openat"}))
            .await
            .expect("command execution should return result");
        assert!(result.success);
        assert!(result.output.contains("openat"));

        let log = tokio::fs::read_to_string(&log_path)
            .await
            .expect("syscall anomaly log should be written");
        assert!(log.contains("\"kind\":\"unknown_syscall\""));
        assert!(log.contains("\"syscall\":\"openat\""));
    }

    // ─── PR #91 Phase 1c: integrity_level wiring on ShellTool ────

    /// New ShellTool defaults to `IntegrityLevel::Default` —
    /// byte-identical behavior to pre-#91. All ~20 existing
    /// `ShellTool::new` test callers (and the simpler
    /// `default_tools_with_runtime` factory) inherit this.
    #[test]
    fn shell_tool_default_integrity_level_is_default() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        assert_eq!(
            tool.integrity_level,
            crate::security::traits::IntegrityLevel::Default,
            "ShellTool::new must keep the no-IL-lowering default"
        );
    }

    /// Builder method opts in to a specific level. Pins the
    /// `with_integrity_level` surface that `tools::all_tools_impl`
    /// reaches via the config bridge.
    #[test]
    fn shell_tool_with_integrity_level_stores_requested_level() {
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            test_sandbox(),
        );
        // Default → Default — no-op
        let same = tool.with_integrity_level(crate::security::traits::IntegrityLevel::Default);
        assert_eq!(
            same.integrity_level,
            crate::security::traits::IntegrityLevel::Default
        );
        // Default → Medium — explicit no-op (Phase 1c.2 won't change
        // anything because the runtime treats Medium == parent IL on
        // an unelevated Medium plaw process — but pin the storage)
        #[cfg(target_os = "windows")]
        {
            let lowered =
                same.with_integrity_level(crate::security::traits::IntegrityLevel::Medium);
            assert_eq!(
                lowered.integrity_level,
                crate::security::traits::IntegrityLevel::Medium
            );
        }
    }

    /// At Default IL, ShellTool's execute path is byte-identical to
    /// pre-#91 — same wrap_command + spawn + after_spawn flow,
    /// routed through `spawn_with_integrity`'s default trait impl.
    /// We verify by piping through `CountingSandbox` and asserting
    /// the existing wrap_command + after_spawn invariants still
    /// hold.
    #[tokio::test]
    async fn shell_tool_at_default_integrity_preserves_spawn_flow() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct CountingSandbox {
            wrap_count: Arc<AtomicU32>,
            after_count: Arc<AtomicU32>,
        }

        impl crate::security::Sandbox for CountingSandbox {
            fn wrap_command(&self, _cmd: &mut std::process::Command) -> std::io::Result<()> {
                self.wrap_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            fn after_spawn(&self, _pid: u32) -> std::io::Result<()> {
                self.after_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            fn is_available(&self) -> bool {
                true
            }
            fn name(&self) -> &str {
                "counting"
            }
            fn description(&self) -> &str {
                "test"
            }
        }

        let wrap_count = Arc::new(AtomicU32::new(0));
        let after_count = Arc::new(AtomicU32::new(0));
        let sandbox: Arc<dyn crate::security::Sandbox> = Arc::new(CountingSandbox {
            wrap_count: wrap_count.clone(),
            after_count: after_count.clone(),
        });
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            sandbox,
        );
        // Default integrity → spawn_with_integrity default impl →
        // wrap_command + spawn + after_spawn.
        let _ = tool
            .execute(json!({"command": "echo phase-1c-default-flow-smoke"}))
            .await;

        assert_eq!(
            wrap_count.load(Ordering::SeqCst),
            1,
            "wrap_command must fire EXACTLY once via spawn_with_integrity default impl"
        );
        assert_eq!(
            after_count.load(Ordering::SeqCst),
            1,
            "after_spawn must fire EXACTLY once via spawn_with_integrity default impl"
        );
    }

    /// Phase 1c.2c: at a non-Default IL, the WindowsJobObjectSandbox
    /// override returns `SandboxedChild::Lowered` spawned via PR #103's
    /// piped Token IL primitive, and `SandboxedChild::wait_with_output`
    /// drains the IOCP-backed pipes. ShellTool therefore CAPTURES the
    /// lowered child's stdout — the previous deferred-feature bail is
    /// gone. This is the unit-level proof that ShellTool reads lowered
    /// output; the real `WindowsJobObjectSandbox` path is pinned by
    /// `windows_job::tests::spawn_with_integrity_at_low_returns_lowered_child`,
    /// and the end-to-end real-Windows capture by the integration test
    /// `shell_lowered_il_captures_stdout`.
    ///
    /// `MockLoweredSandbox` spawns a real cmd.exe at the requested IL
    /// via the production PIPED primitive so we exercise the actual
    /// capture path without standing up the whole Job Object backend.
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn shell_tool_at_low_integrity_captures_lowered_output() {
        struct MockLoweredSandbox;
        #[async_trait::async_trait]
        impl crate::security::Sandbox for MockLoweredSandbox {
            fn wrap_command(&self, _cmd: &mut std::process::Command) -> std::io::Result<()> {
                Ok(())
            }
            fn is_available(&self) -> bool {
                true
            }
            fn name(&self) -> &str {
                "mock-lowered"
            }
            fn description(&self) -> &str {
                "test mock that always returns piped Lowered for any non-Default level"
            }

            async fn spawn_with_integrity(
                &self,
                _cmd: tokio::process::Command,
                level: crate::security::traits::IntegrityLevel,
            ) -> std::io::Result<crate::security::traits::SandboxedChild> {
                use crate::security::traits::{IntegrityLevel, SandboxedChild};
                if matches!(level, IntegrityLevel::Default) {
                    panic!("test sandbox should only see non-Default IL");
                }
                // Spawn a real cmd.exe at the requested IL via the PIPED
                // primitive. Test runner is at Medium so Low is always
                // reachable. echo is a cmd builtin (no file write) so it
                // runs cleanly even at Low IL.
                let cmd_exe = std::env::var_os("ComSpec")
                    .unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows\System32\cmd.exe"));
                let child = crate::security::windows_token_il::spawn_with_lowered_token_piped(
                    std::path::Path::new(&cmd_exe),
                    &["/C".to_string(), "echo lowered-capture-marker".to_string()],
                    level,
                    None,
                    None,
                )?;
                Ok(SandboxedChild::Lowered(child))
            }
        }

        let sandbox: Arc<dyn crate::security::Sandbox> = Arc::new(MockLoweredSandbox);
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            sandbox,
        )
        .with_integrity_level(crate::security::traits::IntegrityLevel::Low);

        let result = tool
            .execute(json!({"command": "echo ignored-by-mock"}))
            .await
            .expect("execute must Ok-wrap the ToolResult");

        assert!(
            result.success,
            "Lowered IL shell must now succeed (output capture works); error={:?}",
            result.error
        );
        assert!(
            result.output.contains("lowered-capture-marker"),
            "ShellTool must capture the lowered child's stdout via the \
             piped wait_with_output path; got output={:?}",
            result.output
        );
    }

    /// Phase 1c.2c END-TO-END: ShellTool + the REAL
    /// `WindowsJobObjectSandbox` at Low IL captures stdout. This is the
    /// full production path — sandbox builds the Job Object, lowers the
    /// token, pipes stdio, assigns the child to the job, and ShellTool
    /// drains the pipes. The mock test above isolates the capture; this
    /// one proves the whole stack composes. Test runner is at Medium so
    /// Low is reachable; `echo` is a cmd builtin (no file write) so it
    /// runs cleanly at Low IL.
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn shell_lowered_il_captures_stdout() {
        let sandbox: Arc<dyn crate::security::Sandbox> = Arc::new(
            crate::security::windows_job::WindowsJobObjectSandbox::new(
                crate::security::windows_job::WindowsJobLimits::built_in_defaults(),
            )
            .expect("Job Object sandbox must construct on Windows"),
        );
        let tool = ShellTool::new(
            test_security(AutonomyLevel::Supervised),
            test_runtime(),
            sandbox,
        )
        .with_integrity_level(crate::security::traits::IntegrityLevel::Low);

        let result = tool
            .execute(json!({"command": "echo real-sandbox-low-il-marker"}))
            .await
            .expect("execute must Ok-wrap the ToolResult");

        assert!(
            result.success,
            "real Low-IL ShellTool execution must succeed for a no-write \
             builtin; error={:?}",
            result.error
        );
        assert!(
            result.output.contains("real-sandbox-low-il-marker"),
            "the full WindowsJobObjectSandbox → piped Token IL → ShellTool \
             stack must capture stdout; got output={:?}",
            result.output
        );
    }
}
