use std::path::PathBuf;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tokio::sync::Mutex;

const EMBEDDING_PORT: u16 = 18991;
const BINARY_NAME: &str = "llama-server.exe";
const MODEL_NAME: &str = "embeddinggemma-300m-qat-Q8_0.gguf";
const EMBEDDING_DIMENSIONS: u32 = 768;

/// Manages the local llama-server embedding process
pub struct EmbeddingManager {
    child: Option<tokio::process::Child>,
    pub running: bool,
    pub port: u16,
    data_dir: PathBuf,
}

impl EmbeddingManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            child: None,
            running: false,
            port: EMBEDDING_PORT,
            data_dir,
        }
    }

    /// Find the embedding directory containing llama-server and the model
    fn find_embedding_dir(&self) -> Option<PathBuf> {
        // 1. plaw-data/embedding/
        let bundled = self.data_dir.join("embedding");
        if bundled.join(BINARY_NAME).exists() && bundled.join(MODEL_NAME).exists() {
            return Some(bundled);
        }
        // 2. Next to exe: ../embedding/ (for portable installs)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let beside_exe = parent.join("embedding");
                if beside_exe.join(BINARY_NAME).exists() && beside_exe.join(MODEL_NAME).exists() {
                    return Some(beside_exe);
                }
            }
        }
        None
    }

    /// Check if embedding files are available
    pub fn is_available(&self) -> bool {
        self.find_embedding_dir().is_some()
    }

    /// Start the llama-server embedding process
    pub async fn start(&mut self) -> Result<u16, String> {
        if self.running {
            return Ok(self.port);
        }

        let emb_dir = self.find_embedding_dir()
            .ok_or("Embedding files not found (llama-server.exe + model GGUF)")?;

        let binary = emb_dir.join(BINARY_NAME);
        let model = emb_dir.join(MODEL_NAME);

        let mut cmd = tokio::process::Command::new(&binary);
        cmd.arg("--model").arg(&model)
            .arg("--embedding")
            .arg("--pooling").arg("last")
            .arg("--host").arg("127.0.0.1")
            .arg("--port").arg(self.port.to_string())
            .current_dir(&emb_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        // Prevent console window on Windows
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let child = cmd.spawn()
            .map_err(|e| format!("Failed to start embedding server: {e}"))?;

        self.child = Some(child);
        self.running = true;

        // Wait for model to fully load (poll /health up to 30s)
        let ready = self.wait_ready(30).await;
        if !ready {
            if !self.check_alive().await {
                self.running = false;
                self.child = None;
                return Err("Embedding server exited during startup".into());
            }
            eprintln!("[plaw] Embedding server alive but model still loading after 30s");
        }

        // Update config.toml [memory] section
        self.update_config(true);

        Ok(self.port)
    }

    /// Stop the embedding server gracefully
    pub async fn stop(&mut self) -> Result<(), String> {
        if let Some(ref mut child) = self.child {
            // Try taskkill on Windows for clean shutdown
            #[cfg(windows)]
            if let Some(pid) = child.id() {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T"])
                    .creation_flags(0x0800_0000)
                    .output();
            }

            // Wait up to 3s
            let exited = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                child.wait(),
            ).await;

            // Force kill if still alive
            if exited.is_err() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }

        self.child = None;
        self.running = false;

        // Reset config.toml [memory] section
        self.update_config(false);

        Ok(())
    }

    /// Synchronous force kill for app exit cleanup
    pub fn force_kill(&mut self) {
        if let Some(ref child) = self.child {
            #[cfg(windows)]
            if let Some(pid) = child.id() {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x0800_0000)
                    .output();
            }
        }
        self.child = None;
        self.running = false;
    }

    /// Poll /health endpoint until the server returns 200 (model loaded).
    async fn wait_ready(&mut self, timeout_secs: u64) -> bool {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .no_proxy()
            .build()
            .unwrap_or_default();
        let url = format!("http://127.0.0.1:{}/health", self.port);
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(timeout_secs);
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // Check process still alive
            if !self.check_alive_peek() {
                return false;
            }
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    return true;
                }
            }
        }
        false
    }

    /// Non-destructive alive check (doesn't clear child on exit)
    fn check_alive_peek(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }

    /// Check if the process is still alive
    async fn check_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.running = false;
                    self.child = None;
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.running = false;
                    false
                }
            }
        } else {
            false
        }
    }

    /// Update config.toml [memory] section based on embedding server state
    fn update_config(&self, running: bool) {
        let config_path = self.data_dir.join(".plaw").join("config.toml");
        let content = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut val: toml::Value = match content.parse() {
            Ok(v) => v,
            Err(_) => return,
        };
        let table = match val.as_table_mut() {
            Some(t) => t,
            None => return,
        };

        let memory = table
            .entry("memory")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

        if let Some(mem) = memory.as_table_mut() {
            if running {
                mem.insert(
                    "embedding_provider".to_string(),
                    toml::Value::String(format!("custom:http://127.0.0.1:{}", self.port)),
                );
                mem.insert(
                    "embedding_model".to_string(),
                    toml::Value::String("embeddinggemma-300m-qat-Q8_0".to_string()),
                );
                mem.insert(
                    "embedding_dimensions".to_string(),
                    toml::Value::Integer(EMBEDDING_DIMENSIONS as i64),
                );
                mem.insert(
                    "vector_weight".to_string(),
                    toml::Value::Float(0.7),
                );
                mem.insert(
                    "keyword_weight".to_string(),
                    toml::Value::Float(0.3),
                );
            } else {
                mem.insert(
                    "embedding_provider".to_string(),
                    toml::Value::String("none".to_string()),
                );
                mem.insert(
                    "embedding_model".to_string(),
                    toml::Value::String(String::new()),
                );
                mem.insert(
                    "embedding_dimensions".to_string(),
                    toml::Value::Integer(0),
                );
                mem.insert(
                    "vector_weight".to_string(),
                    toml::Value::Float(0.0),
                );
                mem.insert(
                    "keyword_weight".to_string(),
                    toml::Value::Float(1.0),
                );
            }
        }

        if let Ok(s) = toml::to_string_pretty(&val) {
            let _ = std::fs::write(&config_path, s);
        }
    }
}

pub type SharedEmbedding = std::sync::Arc<Mutex<EmbeddingManager>>;
