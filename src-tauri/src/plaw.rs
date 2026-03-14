use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Process lifecycle state — the single source of truth.
/// Frontend subscribes via `plaw-status` event and reflects this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Stopped,
    Starting,
    Running,
    Healthy,
    Stopping,
    Restarting,
    Crashed,
}

/// Status event pushed to frontend on every state change
#[derive(Clone, serde::Serialize)]
pub struct StatusEvent {
    pub state: ProcessState,
    pub running: bool,
    pub healthy: bool,
    pub port: u16,
    pub started_at: Option<u64>,
    pub crashed: bool,
}

/// Ring buffer for captured log lines
pub struct LogBuffer {
    lines: Vec<LogLine>,
    capacity: usize,
    write_pos: usize,
    count: usize,
}

#[derive(Clone, serde::Serialize)]
pub struct LogLine {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: Vec::with_capacity(capacity),
            capacity,
            write_pos: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, line: LogLine) {
        if self.lines.len() < self.capacity {
            self.lines.push(line);
        } else {
            self.lines[self.write_pos] = line;
        }
        self.write_pos = (self.write_pos + 1) % self.capacity;
        self.count += 1;
    }

    pub fn recent(&self, n: usize) -> Vec<LogLine> {
        let total = self.lines.len();
        if total == 0 {
            return vec![];
        }
        let n = n.min(total);
        let mut result = Vec::with_capacity(n);

        if total < self.capacity {
            // Buffer not full yet, simple slice from end
            let start = total.saturating_sub(n);
            result.extend_from_slice(&self.lines[start..]);
        } else {
            // Buffer is full, read from write_pos backwards
            let start = if n <= self.write_pos {
                self.write_pos - n
            } else {
                self.capacity - (n - self.write_pos)
            };
            if start < self.write_pos {
                result.extend_from_slice(&self.lines[start..self.write_pos]);
            } else {
                result.extend_from_slice(&self.lines[start..]);
                result.extend_from_slice(&self.lines[..self.write_pos]);
            }
        }
        result
    }
}

/// Manages the Plaw child process
pub struct PlawManager {
    pub child: Option<Child>,
    pub port: u16,
    pub state: ProcessState,
    pub running: bool,
    pub healthy: bool,
    pub started_at: Option<u64>,
    pub logs: LogBuffer,
    pub bearer_token: Option<String>,
    data_dir: PathBuf,
}

impl PlawManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            child: None,
            port: 0,
            state: ProcessState::Stopped,
            running: false,
            healthy: false,
            started_at: None,
            logs: LogBuffer::new(2000),
            bearer_token: None,
            data_dir,
        }
    }

    /// Build a status snapshot for event emission
    pub fn snapshot(&self, crashed: bool) -> StatusEvent {
        StatusEvent {
            state: if crashed { ProcessState::Crashed } else { self.state },
            running: self.running,
            healthy: self.healthy,
            port: self.port,
            started_at: self.started_at,
            crashed,
        }
    }

    /// Find the plaw binary.
    /// Priority: 1) data_dir/bin/ 2) exe同级目录 3) cargo bin 4) PATH
    fn find_binary(&self) -> Option<PathBuf> {
        let names: &[&str] = if cfg!(windows) {
            &["plaw.exe"]
        } else {
            &["plaw"]
        };

        // 1. data_dir/bin/ (portable mode or LOCALAPPDATA fallback)
        for name in names {
            let p = self.data_dir.join("bin").join(name);
            if p.exists() {
                return Some(p);
            }
        }

        // 2. Next to the Tauri exe (handles Program Files install where
        //    data_dir fell back to LOCALAPPDATA but binary is bundled with app)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                for name in names {
                    let p = exe_dir.join(name);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }

        // 3. Cargo bin (dev convenience)
        if let Some(home) = dirs_next::home_dir() {
            for name in names {
                let p = home.join(".cargo").join("bin").join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }

        // 4. Assume it's in PATH
        Some(PathBuf::from("plaw"))
    }

    /// Spawn the Plaw gateway process
    pub async fn start(&mut self, port: u16) -> Result<u16, String> {
        if self.running {
            return Err("Plaw is already running".into());
        }

        self.state = ProcessState::Starting;

        let binary = self.find_binary()
            .ok_or_else(|| {
                self.state = ProcessState::Stopped;
                "Plaw binary not found".to_string()
            })?;

        // Ensure config directory exists
        let config_dir = self.data_dir.join(".plaw");
        let _ = std::fs::create_dir_all(&config_dir);

        let mut cmd = Command::new(&binary);
        cmd.arg("--config-dir").arg(&config_dir)
            .arg("gateway")
            .arg("-p").arg(port.to_string())
            .env("HOME", &self.data_dir)
            .env("USERPROFILE", &self.data_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            ;

        // Inherit system proxy env vars (HTTPS_PROXY etc.) so Plaw follows
        // the user's network environment. If no proxy is set, Plaw connects
        // directly. Users can override via config.toml [proxy] section.
        //
        // Protect local services (embedding server, gateway) from being proxied.
        cmd.env("NO_PROXY", "localhost,127.0.0.1,::1");

        // Prepend bundled tools to PATH so Plaw's shell can find them
        {
            let mut extra_paths = Vec::new();
            let python_dir = self.data_dir.join("python");
            if python_dir.is_dir() {
                extra_paths.push(python_dir.clone());
                // Also add Scripts/ for pip-installed executables
                extra_paths.push(python_dir.join("Scripts"));
            }
            let pandoc_dir = self.data_dir.join("pandoc");
            if pandoc_dir.is_dir() {
                extra_paths.push(pandoc_dir);
            }
            let lo_program = self.data_dir.join("libreoffice").join("libreoffice").join("program");
            if lo_program.is_dir() {
                extra_paths.push(lo_program);
            }
            let poppler_dir = self.data_dir.join("poppler");
            if poppler_dir.is_dir() {
                extra_paths.push(poppler_dir);
            }
            let node_dir = self.data_dir.join("node");
            if node_dir.is_dir() {
                extra_paths.push(node_dir);
            }
            let bin_dir = self.data_dir.join("bin");
            if bin_dir.is_dir() {
                extra_paths.push(bin_dir);
            }
            if !extra_paths.is_empty() {
                let sys_path = std::env::var("PATH").unwrap_or_default();
                let new_path = extra_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .chain(std::iter::once(sys_path))
                    .collect::<Vec<_>>()
                    .join(";");
                cmd.env("PATH", new_path);
            }
        }

        // Set NODE_PATH so Node.js scripts can find bundled npm packages
        let node_modules_dir = self.data_dir.join("node_modules_global").join("node_modules");
        if node_modules_dir.is_dir() {
            cmd.env("NODE_PATH", node_modules_dir.display().to_string());
        }

        // Prevent console window on Windows
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        let child = cmd.spawn()
            .map_err(|e| {
                self.state = ProcessState::Stopped;
                format!("Failed to start Plaw: {e}")
            })?;

        self.child = Some(child);
        self.port = port;
        self.running = true;
        self.state = ProcessState::Running;
        self.started_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        );

        Ok(port)
    }

    /// Stop the running Plaw process gracefully.
    /// 1. Try POST /shutdown (if gateway supports it)
    /// 2. Wait up to 3s for process to exit
    /// 3. Force kill if still alive
    pub async fn stop(&mut self) -> Result<(), String> {
        // Only set Stopping if not already Restarting (preserve restart state)
        if self.state != ProcessState::Restarting {
            self.state = ProcessState::Stopping;
        }

        if let Some(ref mut child) = self.child {
            let port = self.port;

            // Step 1: Try graceful shutdown via API
            if port > 0 {
                let _ = request_shutdown(port, self.bearer_token.as_deref()).await;
            }

            // Step 2: Wait up to 3s for graceful exit
            let exited = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                child.wait(),
            ).await;

            // Step 3: Force kill if still alive
            if exited.is_err() {
                let _ = child.kill().await;
                let _ = child.wait().await; // reap zombie
            }

            self.child = None;
            self.running = false;
            self.healthy = false;
            self.started_at = None;
            // Only set Stopped if not Restarting
            if self.state != ProcessState::Restarting {
                self.state = ProcessState::Stopped;
            }
            Ok(())
        } else {
            self.running = false;
            self.healthy = false;
            self.started_at = None;
            if self.state != ProcessState::Restarting {
                self.state = ProcessState::Stopped;
            }
            Ok(())
        }
    }

    /// Check if the process is still alive
    pub async fn check_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    // Process exited
                    self.running = false;
                    self.child = None;
                    false
                }
                Ok(None) => {
                    // Still running
                    true
                }
                Err(_) => {
                    self.running = false;
                    false
                }
            }
        } else {
            self.running = false;
            false
        }
    }
}

pub type SharedManager = Arc<Mutex<PlawManager>>;

/// Spawn a background task to capture stderr and detect crash on EOF.
/// On unexpected exit, attempts auto-restart up to `max_restarts` times
/// with exponential backoff (2s, 4s, 8s).
pub fn spawn_log_reader(
    manager: SharedManager,
    stderr: tokio::process::ChildStderr,
    app_handle: tauri::AppHandle,
) {
    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let log_line = parse_log_line(&line);
            let mut mgr = manager.lock().await;
            mgr.logs.push(log_line);
        }
        // Stream closed — check if this was unexpected
        let mut mgr = manager.lock().await;
        if mgr.running {
            let port = mgr.port;
            mgr.running = false;
            mgr.healthy = false;
            mgr.child = None;
            mgr.started_at = None;
            mgr.state = ProcessState::Crashed;

            // Emit crash event
            let ev = mgr.snapshot(true);
            mgr.logs.push(LogLine {
                timestamp: chrono_now(),
                level: "ERROR".to_string(),
                message: "Plaw process crashed unexpectedly".to_string(),
            });
            drop(mgr);
            let _ = app_handle.emit("plaw-status", &ev);

            // Auto-restart with backoff
            auto_restart(manager.clone(), app_handle.clone(), port).await;
        }
    });
}

/// Attempt to restart Plaw up to 3 times with exponential backoff.
async fn auto_restart(
    manager: SharedManager,
    app_handle: tauri::AppHandle,
    last_port: u16,
) {
    const MAX_RESTARTS: u32 = 3;
    let mut delay_secs = 2u64;

    for attempt in 1..=MAX_RESTARTS {
        {
            let mut mgr = manager.lock().await;
            mgr.logs.push(LogLine {
                timestamp: chrono_now(),
                level: "INFO".to_string(),
                message: format!("Auto-restart attempt {attempt}/{MAX_RESTARTS} in {delay_secs}s..."),
            });
        }

        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;

        // Check if user manually stopped or started in the meantime
        {
            let mgr = manager.lock().await;
            if mgr.running {
                return; // Already running (user started manually)
            }
        }

        // Try to restart with the same port
        let port = if last_port > 0 {
            // Check if port still available
            if std::net::TcpListener::bind(format!("127.0.0.1:{last_port}")).is_ok() {
                last_port
            } else {
                let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                l.local_addr().unwrap().port()
            }
        } else {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };

        let mut mgr = manager.lock().await;
        match mgr.start(port).await {
            Ok(_) => {
                mgr.logs.push(LogLine {
                    timestamp: chrono_now(),
                    level: "INFO".to_string(),
                    message: format!("Auto-restart successful on port {port}"),
                });

                // Capture stdio
                if let Some(ref mut child) = mgr.child {
                    if let Some(stderr) = child.stderr.take() {
                        spawn_log_reader(manager.clone(), stderr, app_handle.clone());
                    }
                    if let Some(stdout) = child.stdout.take() {
                        spawn_stdout_reader(manager.clone(), stdout);
                    }
                }

                let ev = mgr.snapshot(false);
                drop(mgr);
                let _ = app_handle.emit("plaw-status", &ev);

                // Wait for healthy
                for _ in 0..20 {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if health_check(port).await {
                        let mut mgr = manager.lock().await;
                        mgr.healthy = true;
                        let ev = mgr.snapshot(false);
                        drop(mgr);
                        let _ = app_handle.emit("plaw-status", &ev);
                        break;
                    }
                }
                return; // Success
            }
            Err(e) => {
                mgr.logs.push(LogLine {
                    timestamp: chrono_now(),
                    level: "ERROR".to_string(),
                    message: format!("Auto-restart failed: {e}"),
                });
                drop(mgr);
            }
        }

        delay_secs *= 2; // Exponential backoff
    }

    // All retries exhausted
    let mut mgr = manager.lock().await;
    mgr.logs.push(LogLine {
        timestamp: chrono_now(),
        level: "ERROR".to_string(),
        message: format!("Auto-restart failed after {MAX_RESTARTS} attempts. Manual restart required."),
    });
}

pub fn spawn_stdout_reader(
    manager: SharedManager,
    stdout: tokio::process::ChildStdout,
) {
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut paired = false;
        while let Ok(Some(line)) = lines.next_line().await {
            // Try to capture pairing code from: "     │  123456  │"
            if !paired {
                if let Some(code) = extract_pairing_code(&line) {
                    let port = {
                        let mgr = manager.lock().await;
                        mgr.port
                    };
                    if port > 0 {
                        // Wait a moment for the gateway to be ready
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if let Some(token) = auto_pair(port, &code).await {
                            let mut mgr = manager.lock().await;
                            mgr.bearer_token = Some(token);
                            mgr.logs.push(LogLine {
                                timestamp: chrono_now(),
                                level: "INFO".to_string(),
                                message: "Auto-paired with Plaw gateway".to_string(),
                            });
                            paired = true;
                        }
                    }
                }
            }
            let log_line = parse_log_line(&line);
            let mut mgr = manager.lock().await;
            mgr.logs.push(log_line);
        }
        // stdout EOF handled by stderr reader for crash detection
    });
}

/// Extract 6-digit pairing code from gateway stdout
/// Matches: "     │  123456  │"
fn extract_pairing_code(line: &str) -> Option<String> {
    let trimmed = line.trim();
    // Match the box pattern: │  XXXXXX  │
    if trimmed.starts_with('│') && trimmed.ends_with('│') {
        let inner = trimmed.trim_start_matches('│').trim_end_matches('│').trim();
        if inner.len() == 6 && inner.chars().all(|c| c.is_ascii_digit()) {
            return Some(inner.to_string());
        }
    }
    // Also try matching "X-Pairing-Code: XXXXXX"
    if let Some(pos) = line.find("X-Pairing-Code:") {
        let after = line[pos + 16..].trim();
        let code: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if code.len() == 6 {
            return Some(code);
        }
    }
    None
}

/// Automatically pair with the gateway using the captured code
async fn auto_pair(port: u16, code: &str) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}/pair");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .build()
        .ok()?;

    let resp = client
        .post(&url)
        .header("X-Pairing-Code", code)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Parse a log line like "2024-03-08T12:00:00Z INFO gateway: listening on..."
fn parse_log_line(raw: &str) -> LogLine {
    // Try to parse structured log format: TIMESTAMP LEVEL MESSAGE
    let parts: Vec<&str> = raw.splitn(3, ' ').collect();
    if parts.len() >= 3 {
        let maybe_level = parts[1].to_uppercase();
        if matches!(
            maybe_level.as_str(),
            "INFO" | "WARN" | "ERROR" | "DEBUG" | "TRACE"
        ) {
            return LogLine {
                timestamp: parts[0].to_string(),
                level: maybe_level,
                message: parts[2].to_string(),
            };
        }
    }
    // Fallback: treat entire line as INFO message
    LogLine {
        timestamp: chrono_now(),
        level: "INFO".to_string(),
        message: raw.to_string(),
    }
}

fn chrono_now() -> String {
    // Simple timestamp without chrono dependency
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_secs())
}

/// Request graceful shutdown via POST /shutdown
async fn request_shutdown(port: u16, token: Option<&str>) -> bool {
    let url = format!("http://127.0.0.1:{port}/shutdown");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut req = client.post(&url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    matches!(req.send().await, Ok(r) if r.status().is_success())
}

/// Health check by hitting the /health endpoint
pub async fn health_check(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Background health watcher: checks health every 3s while running,
/// emits `plaw-status` event only when health state changes.
/// Stops automatically when `stop_flag` is set.
pub fn spawn_health_watcher(
    manager: SharedManager,
    app_handle: tauri::AppHandle,
    stop_flag: Arc<AtomicBool>,
) {
    let sse_app = app_handle.clone();
    let sse_mgr = manager.clone();
    let sse_stop = stop_flag.clone();

    tokio::spawn(async move {
        let mut prev_healthy = false;
        let mut sse_spawned = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            let (running, port, token) = {
                let mgr = manager.lock().await;
                (mgr.running, mgr.port, mgr.bearer_token.clone())
            };
            if !running {
                sse_spawned = false;
                if prev_healthy {
                    prev_healthy = false;
                    let mgr = manager.lock().await;
                    let ev = mgr.snapshot(false);
                    let _ = app_handle.emit("plaw-status", &ev);
                }
                continue;
            }
            let now_healthy = health_check(port).await;
            let changed = now_healthy != prev_healthy;
            if changed {
                let mut mgr = manager.lock().await;
                mgr.healthy = now_healthy;
                if now_healthy {
                    mgr.state = ProcessState::Healthy;
                } else if mgr.running {
                    mgr.state = ProcessState::Running;
                }
                let ev = mgr.snapshot(false);
                drop(mgr);
                let _ = app_handle.emit("plaw-status", &ev);
                prev_healthy = now_healthy;
            }
            // Spawn SSE listener once gateway is healthy
            if now_healthy && !sse_spawned {
                sse_spawned = true;
                spawn_sse_listener(
                    port,
                    token,
                    sse_mgr.clone(),
                    sse_app.clone(),
                    sse_stop.clone(),
                );
            }
        }
    });
}

/// Subscribe to Plaw's /api/events SSE stream and forward as Tauri events.
/// Automatically reconnects on disconnect.
fn spawn_sse_listener(
    port: u16,
    token: Option<String>,
    _manager: SharedManager,
    app_handle: tauri::AppHandle,
    stop_flag: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        loop {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            let url = format!("http://127.0.0.1:{port}/api/events");
            let client = match reqwest::Client::builder()
                .no_proxy()
                .build()
            {
                Ok(c) => c,
                Err(_) => break,
            };
            let mut req = client.get(&url);
            if let Some(ref t) = token {
                req = req.header("Authorization", format!("Bearer {t}"));
            }
            let resp = match req.send().await {
                Ok(r) if r.status().is_success() => r,
                _ => {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            // Read SSE stream line by line
            let mut stream = resp.bytes_stream();
            use futures_util::StreamExt;
            let mut buf = String::new();
            while let Some(chunk) = stream.next().await {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
                }
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(_) => break, // connection lost, will reconnect
                };
                buf.push_str(&String::from_utf8_lossy(&bytes));

                // Process complete SSE lines
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim().to_string();
                    buf = buf[pos + 1..].to_string();

                    if let Some(data) = line.strip_prefix("data: ") {
                        // Forward raw JSON to frontend as "plaw-activity" event
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                            let _ = app_handle.emit("plaw-activity", &val);
                        }
                    }
                }
            }
            // Stream ended, wait before reconnecting
            if !stop_flag.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    });
}
