use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Canonical skills directory: `data_dir/.plaw/workspace/skills/`
/// Shared with Plaw engine — both read and write from this location.
fn skills_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(".plaw").join("workspace").join("skills")
}

/// Parsed skill entry from SKILL.md frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub homepage: String,
    /// Source: "managed"
    #[serde(default)]
    pub source: String,
    /// Directory path on disk
    #[serde(default)]
    pub path: String,
    /// Compatibility tag: "verified", "needs-setup", "incompatible", or "" (unaudited)
    #[serde(default)]
    pub compatibility: String,
    /// Risk tag: "safe", "warning", "danger", or "" (unaudited)
    #[serde(default)]
    pub risk: String,
}

/// Parse SKILL.md YAML frontmatter
fn parse_skill_md(content: &str) -> Option<SkillEntry> {
    let trimmed = content.trim_start();
    if trimmed.starts_with("---") {
        // Has frontmatter block — only use frontmatter parser (no fallback)
        return parse_skill_md_frontmatter(content);
    }
    // No frontmatter: extract from Markdown heading + first paragraph
    parse_skill_md_heading(content)
}

/// Parse YAML frontmatter between `---` delimiters
fn parse_skill_md_frontmatter(content: &str) -> Option<SkillEntry> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    let end_idx = after_first.find("\n---")?;
    let yaml_str = &after_first[..end_idx];

    let mut name = String::new();
    let mut description = String::new();
    let mut homepage = String::new();
    let mut compatibility = String::new();
    let mut risk = String::new();

    for line in yaml_str.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("description:") {
            description = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("homepage:") {
            homepage = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("compatibility:") {
            compatibility = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("risk:") {
            risk = val.trim().trim_matches('"').to_string();
        }
    }

    // If no name in frontmatter, try to extract from first # heading in body
    if name.is_empty() {
        let body = &trimmed[3 + end_idx + 4..]; // skip past closing ---
        for line in body.lines() {
            let l = line.trim();
            if l.is_empty() { continue; }
            if let Some(heading) = l.strip_prefix('#') {
                name = heading.trim_start_matches('#').trim().to_string();
                break;
            }
        }
    }

    if name.is_empty() {
        return None;
    }

    Some(SkillEntry {
        name,
        description,
        homepage,
        compatibility,
        risk,
        source: String::new(),
        path: String::new(),
    })
}

/// Fallback parser: extract name from first `# Heading` and description from first non-heading paragraph
fn parse_skill_md_heading(content: &str) -> Option<SkillEntry> {
    let mut name = String::new();
    let mut description = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "---" {
            continue;
        }
        if name.is_empty() {
            // First non-empty, non-delimiter line should be a heading
            if let Some(heading) = trimmed.strip_prefix('#') {
                name = heading.trim_start_matches('#').trim().to_string();
            } else {
                // Not a heading — use as name anyway
                name = trimmed.to_string();
            }
            continue;
        }
        // Skip sub-headings, code blocks, etc.
        if trimmed.starts_with('#') || trimmed.starts_with("```") || trimmed.starts_with("---") {
            if description.is_empty() {
                continue;
            }
            break;
        }
        if description.is_empty() {
            description = trimmed.to_string();
            break;
        }
    }

    if name.is_empty() {
        return None;
    }

    Some(SkillEntry {
        name,
        description,
        homepage: String::new(),
        compatibility: String::new(),
        risk: String::new(),
        source: String::new(),
        path: String::new(),
    })
}

/// Resolve a skill name (directory slug or display name) to its SKILL.md path
pub fn resolve_skill_md(data_dir: &Path, name: &str) -> Result<std::path::PathBuf, String> {
    let dir = skills_dir(data_dir);
    // Try direct match first (directory slug)
    let direct = dir.join(name).join("SKILL.md");
    if direct.exists() {
        return Ok(direct);
    }
    // Fallback: scan all skill dirs and match by display name
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let md = path.join("SKILL.md");
            if !md.exists() { continue; }
            if let Ok(content) = std::fs::read_to_string(&md) {
                if let Some(skill) = parse_skill_md(&content) {
                    if skill.name == name {
                        return Ok(md);
                    }
                }
            }
        }
    }
    Err(format!("Skill '{}' not found", name))
}

/// Scan a directory for skills (each subdirectory with SKILL.md)
pub fn scan_skills_dir(dir: &Path, source: &str) -> Vec<SkillEntry> {
    let mut skills = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.take(300) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(&skill_md) {
            Ok(c) if c.len() <= 256_000 => c,
            _ => continue,
        };
        if let Some(mut skill) = parse_skill_md(&content) {
            skill.source = source.to_string();
            skill.path = path.display().to_string();
            skills.push(skill);
        }
    }
    skills
}

/// Protected skills that cannot be uninstalled by the user.
/// These skills are expected to exist in the skills directory (user-managed, not embedded in binary).
const PROTECTED_SKILL_NAMES: &[&str] = &[
    "find-skills",
    "skill-creator",
    "audit-skills",
    "fix-skills",
    "pptx",
    "xlsx",
    "docx",
    "pdf",
];

/// List all installed skills from Plaw's skills directory
pub fn list_local_skills(data_dir: &Path) -> Vec<SkillEntry> {
    let dir = skills_dir(data_dir);
    let mut skills = scan_skills_dir(&dir, "managed");
    for skill in &mut skills {
        if PROTECTED_SKILL_NAMES.contains(&skill.name.as_str()) {
            skill.source = "builtin".to_string();
        }
    }
    skills
}

/// Install a skill from a URL (download SKILL.md directory)
pub async fn install_skill_from_url(
    data_dir: &Path,
    url: &str,
    proxy_url: Option<&str>,
) -> Result<String, String> {
    let skills_dir = skills_dir(data_dir);
    std::fs::create_dir_all(&skills_dir)
        .map_err(|e| format!("Failed to create skills dir: {e}"))?;

    // Build HTTP client with optional proxy
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30));
    if let Some(proxy) = proxy_url {
        if !proxy.is_empty() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
    }
    let client = builder.build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    // Fetch the SKILL.md content
    let skill_url = if url.ends_with("SKILL.md") {
        url.to_string()
    } else {
        format!("{}/SKILL.md", url.trim_end_matches('/'))
    };

    let resp = client.get(&skill_url).send().await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let content = resp.text().await
        .map_err(|e| format!("Read failed: {e}"))?;

    // Parse to get the skill name
    let skill = parse_skill_md(&content)
        .ok_or("Invalid SKILL.md: missing name")?;

    // Write to disk
    let skill_dir = skills_dir.join(&skill.name);
    std::fs::create_dir_all(&skill_dir)
        .map_err(|e| format!("Failed to create skill dir: {e}"))?;
    std::fs::write(skill_dir.join("SKILL.md"), &content)
        .map_err(|e| format!("Failed to write SKILL.md: {e}"))?;

    Ok(skill.name)
}

/// Install a skill from a local path (copy directory)
pub fn install_skill_from_path(
    data_dir: &Path,
    source_path: &Path,
) -> Result<String, String> {
    let skill_md = source_path.join("SKILL.md");
    if !skill_md.exists() {
        return Err("No SKILL.md found in the specified directory".to_string());
    }
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;
    let skill = parse_skill_md(&content)
        .ok_or("Invalid SKILL.md: missing name")?;

    let target_dir = skills_dir(data_dir).join(&skill.name);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed to create target dir: {e}"))?;

    // Copy all files from source to target
    copy_dir_contents(source_path, &target_dir)?;

    Ok(skill.name)
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(src)
        .map_err(|e| format!("Read dir failed: {e}"))?
    {
        let entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path)
                .map_err(|e| format!("Create dir failed: {e}"))?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Copy failed: {e}"))?;
        }
    }
    Ok(())
}

/// Uninstall a skill by name (remove from managed dir)
pub fn uninstall_skill(data_dir: &Path, name: &str) -> Result<(), String> {
    if PROTECTED_SKILL_NAMES.contains(&name) {
        return Err("Cannot uninstall protected skill".to_string());
    }
    let skill_dir = skills_dir(data_dir).join(name);
    if !skill_dir.exists() {
        return Err("Skill not found".to_string());
    }
    std::fs::remove_dir_all(&skill_dir)
        .map_err(|e| format!("Failed to remove skill: {e}"))?;
    Ok(())
}

// ========================
// AI Skill Audit
// ========================

/// Result of AI-powered skill audit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditResult {
    pub compatibility: String, // "verified", "needs-setup", "incompatible"
    pub risk: String,          // "safe", "warning", "danger"
    pub reason: String,        // human-readable explanation
    pub dependencies: Vec<String>, // external deps found
}

/// Read LLM config from config.toml
fn decrypt_secret(data_dir: &Path, value: &str) -> Result<String, String> {
    if let Some(hex_str) = value.strip_prefix("enc2:") {
        let key_path = data_dir.join(".plaw").join(".secret_key");
        let key_hex = std::fs::read_to_string(&key_path)
            .map_err(|e| format!("Failed to read .secret_key: {e}"))?;
        let key_bytes = hex::decode(key_hex.trim())
            .map_err(|e| format!("Failed to hex-decode .secret_key: {e}"))?;
        if key_bytes.len() != 32 {
            return Err(format!("Invalid .secret_key length: {} (expected 32 bytes)", key_bytes.len()));
        }
        let blob = hex::decode(hex_str)
            .map_err(|e| format!("Failed to hex-decode encrypted key: {e}"))?;
        if blob.len() <= 12 {
            return Err("Encrypted value too short".to_string());
        }
        use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Key, Nonce};
        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let key = Key::from_slice(&key_bytes);
        let cipher = ChaCha20Poly1305::new(key);
        let plaintext = cipher.decrypt(nonce, ciphertext)
            .map_err(|_| "Decryption failed — wrong key or tampered data".to_string())?;
        String::from_utf8(plaintext)
            .map_err(|_| "Decrypted key is not valid UTF-8".to_string())
    } else if value.starts_with("enc:") {
        Err("Legacy enc: format not supported in Tauri audit. Please re-save your API key.".to_string())
    } else {
        Ok(value.to_string())
    }
}

fn read_llm_config(data_dir: &Path) -> Result<(String, String, String), String> {
    let config_path = data_dir.join(".plaw").join("config.toml");
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.toml: {e}"))?;
    let val: toml::Value = content.parse()
        .map_err(|e| format!("Failed to parse config.toml: {e}"))?;

    let raw_key = val.get("api_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let provider = val.get("default_provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let model = val.get("default_model").and_then(|v| v.as_str()).unwrap_or("").to_string();

    if raw_key.is_empty() {
        return Err("No api_key in config.toml".to_string());
    }
    let api_key = decrypt_secret(data_dir, &raw_key)?;
    Ok((api_key, provider, model))
}

/// Extract base URL from provider string like "anthropic-custom:https://api.kimi.com/coding"
fn provider_base_url(provider: &str) -> String {
    if let Some(url) = provider.strip_prefix("anthropic-custom:") {
        url.to_string()
    } else {
        match provider {
            "kimi-coder" | "" => "https://api.kimi.com/coding".to_string(),
            "kimi-moonshot" => "https://api.moonshot.cn".to_string(),
            "anthropic" => "https://api.anthropic.com".to_string(),
            _ => provider.to_string(),
        }
    }
}

/// Detect commonly needed tools on the system.
/// Uses platform-appropriate shell to resolve script wrappers (.cmd/.ps1 on Windows).
pub fn detect_system_tools_with_data_dir(data_dir: Option<&std::path::Path>) -> Vec<String> {
    // Mobile platforms have no CLI tools
    if cfg!(target_os = "android") || cfg!(target_os = "ios") {
        return vec!["platform: mobile (no CLI tools)".to_string()];
    }

    let path = match data_dir {
        Some(d) => get_bundled_path(d),
        None => get_full_path().to_string(),
    };

    let checks: &[(&str, &str)] = &[
        ("node", "node --version"),
        ("npm", "npm --version"),
        ("python", "python --version"),
        ("pip", "pip --version"),
        ("docker", "docker --version"),
        ("git", "git --version"),
        ("ffmpeg", "ffmpeg -version"),
        ("yt-dlp", "yt-dlp --version"),
        ("curl", "curl --version"),
        ("cargo", "cargo --version"),
        ("pnpm", "pnpm --version"),
        ("pandoc", "pandoc --version"),
        ("soffice", "soffice --version"),
        ("pdftoppm", "pdftoppm -v"),
    ];

    let mut results = Vec::new();
    results.push(format!("os: {}", std::env::consts::OS));

    for (name, version_cmd) in checks {
        let mut c = shell_command(version_cmd);
        c.env("PATH", &path);
        c.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        match c.output() {
            Ok(output) if output.status.success() => {
                let ver = String::from_utf8_lossy(&output.stdout);
                let first_line = ver.lines().next().unwrap_or("installed").trim().to_string();
                results.push(format!("{name}: {first_line}"));
            }
            _ => {
                // pdftoppm/soffice print version to stderr
                let mut c2 = shell_command(version_cmd);
                c2.env("PATH", &path);
                c2.stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .stdin(std::process::Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    c2.creation_flags(0x08000000);
                }
                match c2.output() {
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stderr.trim().is_empty() {
                            let first = stderr.lines().next().unwrap_or("installed").trim().to_string();
                            results.push(format!("{name}: {first}"));
                        } else {
                            results.push(format!("{name}: NOT INSTALLED"));
                        }
                    }
                    _ => results.push(format!("{name}: NOT INSTALLED")),
                }
            }
        }
    }

    // Chrome detection — platform-specific paths
    let has_chrome = if cfg!(windows) {
        [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ].iter().any(|p| std::path::Path::new(p).exists())
    } else if cfg!(target_os = "macos") {
        std::path::Path::new("/Applications/Google Chrome.app").exists()
    } else {
        ["/usr/bin/google-chrome", "/usr/bin/chromium-browser", "/usr/bin/chromium"]
            .iter().any(|p| std::path::Path::new(p).exists())
    };
    results.push(format!("chrome: {}", if has_chrome { "installed" } else { "NOT INSTALLED" }));

    // Bundled Python packages
    //   Use shell_command() so child shell inherits PATH → finds bundled python.
    if let Some(d) = data_dir {
        let py_pkgs = ["markitdown", "openpyxl", "pptx", "docx", "pypdf",
                        "pdfplumber", "reportlab", "pandas", "defusedxml", "PIL"];
        let mut py_installed = Vec::new();
        for pkg in &py_pkgs {
            let mut cmd = shell_command(&format!("python -c \"import {pkg}\""));
            cmd.env("PATH", &path);
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            if cmd.status().map(|s| s.success()).unwrap_or(false) {
                py_installed.push(*pkg);
            }
        }
        if !py_installed.is_empty() {
            results.push(format!("python-packages: {}", py_installed.join(", ")));
        }

        // Bundled Node.js packages
        //   Use shell_command() so child shell inherits PATH → finds bundled node.
        let node_modules = d.join("node_modules_global").join("node_modules");
        if node_modules.is_dir() {
            let npm_pkgs = ["pptxgenjs", "playwright", "sharp", "react-icons", "docx"];
            let mut npm_installed = Vec::new();
            let node_path_str = node_modules.display().to_string();
            for pkg in &npm_pkgs {
                let mut cmd = shell_command(&format!("node -e \"require('{pkg}')\""));
                cmd.env("PATH", &path);
                cmd.env("NODE_PATH", &node_path_str);
                cmd.stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .stdin(std::process::Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                if cmd.status().map(|s| s.success()).unwrap_or(false) {
                    npm_installed.push(*pkg);
                }
            }
            if !npm_installed.is_empty() {
                results.push(format!("node-packages: {}", npm_installed.join(", ")));
            }
        }
    }

    results
}

/// Resolve the full user PATH from registry (Windows) or login shell (Unix).
/// Tauri GUI apps inherit an incomplete PATH from Explorer; this gets the real one.
fn resolve_full_path() -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Read user PATH from registry via PowerShell (most reliable on Windows)
        let _output = std::process::Command::new("cmd")
            .args(["/c", "echo %PATH%"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .creation_flags(0x08000000)
            .output();

        // Start with process PATH, then extend with common global tool directories
        let mut path = std::env::var("PATH").unwrap_or_default();

        // Add common package manager global bin directories
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let pnpm_dir = format!("{local_app_data}\\pnpm");
            if !path.to_lowercase().contains(&pnpm_dir.to_lowercase()) {
                path = format!("{path};{pnpm_dir}");
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            let npm_dir = format!("{appdata}\\npm");
            if !path.to_lowercase().contains(&npm_dir.to_lowercase()) {
                path = format!("{path};{npm_dir}");
            }
        }
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            let cargo_dir = format!("{userprofile}\\.cargo\\bin");
            if !path.to_lowercase().contains(&cargo_dir.to_lowercase()) {
                path = format!("{path};{cargo_dir}");
            }
            let pip_dir = format!("{userprofile}\\AppData\\Local\\Programs\\Python\\Scripts");
            if !path.to_lowercase().contains(&pip_dir.to_lowercase()) {
                path = format!("{path};{pip_dir}");
            }
        }

        // Also try reading full PATH from registry (system + user)
        let reg_output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command",
                   "[Environment]::GetEnvironmentVariable('Path','User') + ';' + [Environment]::GetEnvironmentVariable('Path','Machine')"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .creation_flags(0x08000000)
            .output();

        if let Ok(out) = reg_output {
            if out.status.success() {
                let reg_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                // Merge registry paths into our path (dedup)
                for dir in reg_path.split(';') {
                    let dir = dir.trim();
                    if !dir.is_empty() && !path.to_lowercase().contains(&dir.to_lowercase()) {
                        path = format!("{path};{dir}");
                    }
                }
            }
        }

        path
    }
    #[cfg(not(windows))]
    {
        // On Unix, try login shell to get full PATH
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let output = std::process::Command::new(&shell)
            .args(["-l", "-c", "echo $PATH"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .output();
        match output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => std::env::var("PATH").unwrap_or_default(),
        }
    }
}

/// Cached full PATH, resolved once.
static FULL_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn get_full_path() -> &'static str {
    FULL_PATH.get_or_init(resolve_full_path)
}

/// Build PATH that includes bundled tool directories from plaw-data.
fn get_bundled_path(data_dir: &std::path::Path) -> String {
    let mut extra = Vec::new();
    let candidates = [
        "python", "python/Scripts", "pandoc", "poppler", "node", "bin",
    ];
    for c in &candidates {
        let p = data_dir.join(c);
        if p.is_dir() {
            extra.push(p.display().to_string());
        }
    }
    let lo = data_dir.join("libreoffice").join("libreoffice").join("program");
    if lo.is_dir() {
        extra.push(lo.display().to_string());
    }
    let sys = get_full_path();
    if extra.is_empty() {
        sys.to_string()
    } else {
        format!("{};{sys}", extra.join(";"))
    }
}

/// Check if a CLI tool exists on the system using `where` (Windows) / `which` (Unix).
/// Uses the full PATH from registry + bundled tool directories.
fn check_tool_exists(tool_name: &str, data_dir: &std::path::Path) -> bool {
    // Reject anything that doesn't look like a simple tool name (security)
    if tool_name.is_empty()
        || tool_name.len() > 64
        || tool_name.contains(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != '.')
    {
        return false;
    }

    let path = get_bundled_path(data_dir);

    // 1) Try as CLI tool via `where`/`which`
    let found_cli = {
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/c", &format!("where {tool_name}")]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &format!("which {tool_name}")]);
            c
        };
        cmd.env("PATH", &path);
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        cmd.status().map(|s| s.success()).unwrap_or(false)
    };
    if found_cli {
        return true;
    }

    // 2) Try as Python package: python -c "import <name>"
    //    Use shell_command() so the child shell inherits PATH and finds bundled python.
    let py_name = tool_name.replace('-', "_"); // pip names use hyphens, import names use underscores
    {
        let mut cmd = shell_command(&format!("python -c \"import {py_name}\""));
        cmd.env("PATH", &path);
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        if cmd.status().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
    }

    // 3) Try as Node.js package: node -e "require('<name>')"
    //    Use shell_command() so the child shell inherits PATH and finds bundled node.
    {
        let node_modules = data_dir.join("node_modules_global").join("node_modules");
        let node_path_env = if node_modules.is_dir() {
            node_modules.display().to_string()
        } else {
            String::new()
        };
        let mut cmd = shell_command(&format!("node -e \"require('{tool_name}')\""));
        cmd.env("PATH", &path);
        if !node_path_env.is_empty() {
            cmd.env("NODE_PATH", &node_path_env);
        }
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        if cmd.status().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
    }

    false
}


/// Platform-appropriate shell command: Windows → cmd /c, Unix → sh -c
fn shell_command(cmd_str: &str) -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", cmd_str]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", cmd_str]);
        c
    }
}

const AUDIT_PROMPT: &str = r#"You are a skill compatibility auditor for Lobster Desktop (a desktop AI agent app).

Analyze the following SKILL.md content and classify it. Return ONLY a JSON object (no markdown, no explanation outside JSON).

## Classification Rules

1. **verified** = Works out of the box:
   - Only uses Lobster's built-in tools (shell, file_read, write_file, edit_file, list_dir, memory_store, memory_recall, web_fetch, web_search_tool)
   - No external API keys needed
   - All required external software is ALREADY INSTALLED on this system (check "System Environment" section below)

2. **needs-setup** = Works but requires user to install/configure something NOT YET available:
   - External API keys (e.g., GEMINI_API_KEY, Telegram bot token)
   - External software that is NOT INSTALLED on this system (check "System Environment" section)
   - External services (SMTP, cloud APIs, database servers)
   - IMPORTANT: If a skill needs e.g. Node.js, and Node.js IS installed, that dependency is satisfied — do NOT mark as needs-setup just because the skill mentions Node.js

3. **incompatible** = Cannot work in Lobster environment:
   - Requires infrastructure most users won't have (cloud-specific services)
   - Fundamentally incompatible with the desktop agent model
   - Depends on features that don't exist in Plaw (MCP servers, VS Code extensions, etc.)

## Security Checks

- **safe** = No security concerns
- **warning** = Uses shell commands, file operations, or network in ways that need user awareness
- **danger** = Potential data exfiltration, hardcoded credentials, suspicious network calls, or attempts to modify system config

## Architecture Rules (from IDENTITY.md)

1. Storage must use memory_store/memory_recall (SQLite), NOT filesystem-based memory
2. No .sh shell scripts (Plaw audit blocks them)
3. skills/ directory is ONLY for skill subdirectories with SKILL.md
4. Config path: plaw-data/.plaw/config.toml (read-only, protected)
5. Agent delegation uses delegate(), subagent_spawn(), parallel_delegate()
6. Platform is Windows, commands must work in PowerShell
7. Keep SKILL.md lean (compact mode only injects name+description)

## ZeroClaw/OpenClaw Compatibility

Plaw is an enhanced fork of ZeroClaw. Many skills from ClewHub are written for ZeroClaw/OpenClaw.
When auditing such skills, treat them as Plaw-compatible — the AI runtime auto-maps paths at execution time:
- `.zeroclaw/` → `.plaw/`
- `zeroclaw-data/` → `plaw-data/`
- `CLAUDE.md` (ZeroClaw identity) → `IDENTITY.md`
- Tool names are identical (shell, read_file, write_file, memory_store, etc.)

Do NOT mark a skill as "incompatible" solely because it references ZeroClaw/OpenClaw paths or terminology.
Only flag incompatibility for genuine architectural mismatches (MCP servers, VS Code extensions, etc.).

## Required JSON Output Format

```json
{
  "compatibility": "verified|needs-setup|incompatible",
  "risk": "safe|warning|danger",
  "reason": "Brief explanation in Chinese (1-2 sentences)",
  "dependencies": ["list", "of", "ALL", "required", "CLI", "tools", "or", "packages"]
}
```

IMPORTANT: The "dependencies" array should list ALL external CLI tools or packages that this skill requires to function, regardless of whether they appear in the system environment list above. The system will automatically verify which ones are actually installed. List the CLI command name (e.g. "agent-browser", "ffmpeg", "yt-dlp"), not the package manager command.

"#;

/// Corrected audit result (subset of AuditResult, only tag + reason)
#[derive(Debug, Deserialize)]
struct CorrectedAudit {
    compatibility: String,
    reason: String,
}

/// Second lightweight LLM call: given verified dependency status, re-assess compatibility + reason.
async fn correct_audit_with_verification(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    model: &str,
    all_deps: &[String],
    installed: &[String],
    missing: &[String],
) -> Result<CorrectedAudit, String> {
    let dep_report = if missing.is_empty() {
        format!("所有依赖均已安装：{}", installed.join(", "))
    } else if installed.is_empty() {
        format!("以下依赖未安装：{}", missing.join(", "))
    } else {
        format!(
            "已安装：{}；未安装：{}",
            installed.join(", "),
            missing.join(", ")
        )
    };

    let prompt = format!(
        r#"你是技能兼容性审计员。之前的审计列出了以下依赖：{}
系统实际验证结果：{}

请根据验证结果重新给出兼容性判断。返回 JSON（无 markdown）：
{{"compatibility": "verified|needs-setup|incompatible", "reason": "1-2句中文说明"}}"#,
        all_deps.join(", "),
        dep_report,
    );

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 200,
        "temperature": 0.0,
        "messages": [{"role": "user", "content": prompt}]
    });

    let resp = client.post(url)
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Correction LLM call failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Correction LLM returned HTTP {}", resp.status()));
    }

    let resp_json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse correction response: {e}"))?;

    let text = resp_json.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    let json_str = if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            &text[start..=end]
        } else {
            text
        }
    } else {
        text
    };

    let corrected: CorrectedAudit = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse correction JSON: {e}"))?;

    // Validate tag
    let compatibility = match corrected.compatibility.as_str() {
        "verified" | "needs-setup" | "incompatible" => corrected.compatibility,
        _ => if missing.is_empty() { "verified".to_string() } else { "needs-setup".to_string() },
    };

    Ok(CorrectedAudit { compatibility, reason: corrected.reason })
}

/// Call LLM to audit a skill's SKILL.md content
pub async fn audit_skill_content(
    data_dir: &Path,
    skill_content: &str,
    proxy_url: Option<&str>,
) -> Result<AuditResult, String> {
    let (api_key, provider, model) = read_llm_config(data_dir)?;
    let base_url = provider_base_url(&provider);
    let url = format!("{base_url}/v1/messages");

    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30));
    if let Some(proxy) = proxy_url {
        if !proxy.is_empty() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
    }
    let client = builder.build().map_err(|e| format!("HTTP client error: {e}"))?;

    // Detect system environment and build context-aware prompt
    let sys_tools = detect_system_tools_with_data_dir(Some(data_dir));
    let sys_info = sys_tools.join("\n");
    let prompt = format!(
        "{AUDIT_PROMPT}## System Environment (tools currently installed on this machine):\n\n{sys_info}\n\n## SKILL.md Content to Audit:\n\n{skill_content}"
    );

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 512,
        "temperature": 0.0,
        "messages": [{"role": "user", "content": prompt}]
    });

    let resp = client.post(&url)
        .header("Content-Type", "application/json")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!("LLM returned HTTP {status}: {body_text}"));
    }

    let resp_json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse LLM response: {e}"))?;

    // Extract text from Anthropic Messages response
    let text = resp_json.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    // Parse JSON from the response (may be wrapped in ```json ... ```)
    let json_str = if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            &text[start..=end]
        } else {
            text
        }
    } else {
        text
    };

    let audit: AuditResult = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse audit JSON: {e}\nRaw: {text}"))?;

    // Validate values
    let compatibility = match audit.compatibility.as_str() {
        "verified" | "needs-setup" | "incompatible" => audit.compatibility.clone(),
        _ => "needs-setup".to_string(), // default to needs-setup if unclear
    };
    let risk = match audit.risk.as_str() {
        "safe" | "warning" | "danger" => audit.risk.clone(),
        _ => "warning".to_string(),
    };

    // LLM lists ALL required dependencies; we verify which are actually missing.
    let missing_deps: Vec<String> = audit.dependencies
        .iter()
        .filter(|dep| !dep.is_empty() && !check_tool_exists(dep, data_dir))
        .cloned()
        .collect();
    let installed_deps: Vec<String> = audit.dependencies
        .iter()
        .filter(|dep| !dep.is_empty() && check_tool_exists(dep, data_dir))
        .cloned()
        .collect();

    // Check if system verification contradicts LLM's assessment
    let needs_correction = (compatibility == "needs-setup" && missing_deps.is_empty())
        || (compatibility == "verified" && !missing_deps.is_empty());

    if needs_correction {
        // Second lightweight LLM call: feed back verified dep status, let LLM re-assess
        match correct_audit_with_verification(
            &client, &url, &api_key, &model,
            &audit.dependencies, &installed_deps, &missing_deps,
        ).await {
            Ok(corrected) => Ok(AuditResult {
                compatibility: corrected.compatibility,
                risk,
                reason: corrected.reason,
                dependencies: missing_deps,
            }),
            Err(_) => {
                // Fallback: use original LLM output but fix tag only
                let fixed_tag = if missing_deps.is_empty() {
                    "verified".to_string()
                } else {
                    "needs-setup".to_string()
                };
                Ok(AuditResult {
                    compatibility: fixed_tag,
                    risk,
                    reason: audit.reason,
                    dependencies: missing_deps,
                })
            }
        }
    } else {
        Ok(AuditResult {
            compatibility,
            risk,
            reason: audit.reason,
            dependencies: missing_deps,
        })
    }
}

/// Inject or update compatibility and risk tags in SKILL.md content
pub fn inject_audit_tags(content: &str, compat_tag: &str, risk_tag: &str) -> String {
    let trimmed = content.trim_start();
    if trimmed.starts_with("---") {
        let after_first = &trimmed[3..];
        if let Some(end_idx) = after_first.find("\n---") {
            let yaml_block = &after_first[..end_idx];
            let after_block = &after_first[end_idx..];

            let mut new_yaml = String::new();
            let mut found_compat = false;
            let mut found_risk = false;
            for line in yaml_block.lines() {
                if line.trim().starts_with("compatibility:") {
                    new_yaml.push_str(&format!("compatibility: {compat_tag}"));
                    found_compat = true;
                } else if line.trim().starts_with("risk:") {
                    new_yaml.push_str(&format!("risk: {risk_tag}"));
                    found_risk = true;
                } else {
                    new_yaml.push_str(line);
                }
                new_yaml.push('\n');
            }
            if !found_compat {
                new_yaml.push_str(&format!("compatibility: {compat_tag}\n"));
            }
            if !found_risk {
                new_yaml.push_str(&format!("risk: {risk_tag}\n"));
            }
            return format!("---\n{new_yaml}---{}", &after_block[4..]);
        }
    }

    // No frontmatter — prepend one
    format!("---\ncompatibility: {compat_tag}\nrisk: {risk_tag}\n---\n\n{content}")
}


/// Registry skill entry (from ClawHub or similar)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySkill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

const GITHUB_REPO: &str = "besoeasy/open-skills";
const GITHUB_RAW_BASE: &str = "https://raw.githubusercontent.com/besoeasy/open-skills/main/skills";

/// Fetch skills listing from GitHub API, then fetch each SKILL.md for descriptions.
/// Uses GitHub Contents API: GET /repos/{owner}/{repo}/contents/skills
pub async fn fetch_github_skills(proxy_url: Option<&str>) -> Result<Vec<RegistrySkill>, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("lobster-desktop/0.1");
    if let Some(proxy) = proxy_url {
        if !proxy.is_empty() {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }
    }
    let client = builder.build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    // Step 1: List skill directories
    let api_url = format!("https://api.github.com/repos/{GITHUB_REPO}/contents/skills");
    let resp = client.get(&api_url)
        .header("Accept", "application/vnd.github.v3+json")
        .send().await
        .map_err(|e| format!("GitHub API request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let entries: Vec<serde_json::Value> = resp.json().await
        .map_err(|e| format!("GitHub API parse error: {e}"))?;

    // Filter directories only
    let dir_names: Vec<String> = entries.iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("dir"))
        .filter_map(|e| e.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    // Step 2: Fetch SKILL.md for each (in parallel, max 10 concurrent)
    let mut skills = Vec::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(10));
    let mut handles = Vec::new();

    for name in dir_names {
        let client = client.clone();
        let sem = semaphore.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            let url = format!("{GITHUB_RAW_BASE}/{name}/SKILL.md");
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(content) = resp.text().await {
                        if let Some(skill) = parse_skill_md(&content) {
                            return Some(RegistrySkill {
                                name: skill.name,
                                description: skill.description,
                                url: format!("{GITHUB_RAW_BASE}/{name}"),
                                version: String::new(),
                                author: "open-skills".to_string(),
                                tags: vec![],
                            });
                        }
                    }
                    // Fallback: return with just the name
                    Some(RegistrySkill {
                        name: name.clone(),
                        description: String::new(),
                        url: format!("{GITHUB_RAW_BASE}/{name}"),
                        version: String::new(),
                        author: "open-skills".to_string(),
                        tags: vec![],
                    })
                }
                _ => None,
            }
        }));
    }

    for handle in handles {
        if let Ok(Some(skill)) = handle.await {
            skills.push(skill);
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Local fallback: scan installed skills when GitHub is unreachable
pub fn search_local_skills(data_dir: &Path, query: &str) -> Vec<RegistrySkill> {
    let dir = skills_dir(data_dir);
    let all: Vec<RegistrySkill> = scan_skills_dir(&dir, "managed")
        .into_iter()
        .map(|skill| RegistrySkill {
            name: skill.name,
            description: skill.description,
            url: skill.path,
            version: String::new(),
            author: "installed".to_string(),
            tags: vec![],
        })
        .collect();

    let query_lower = query.to_lowercase();
    all.into_iter()
        .filter(|s| {
            query_lower.is_empty()
                || s.name.to_lowercase().contains(&query_lower)
                || s.description.to_lowercase().contains(&query_lower)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_managed_skills_dir() {
        let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()
            .join("plaw-data");
        let skills = list_local_skills(&data_dir);

        println!("  Found {} managed skills:", skills.len());
        for s in &skills {
            println!("    - {} [{}]", s.name, s.description.chars().take(60).collect::<String>());
        }
        assert!(!skills.is_empty(), "Should find at least 1 managed skill");
    }

    #[test]
    fn test_parse_invalid_skill() {
        // Empty content
        assert!(parse_skill_md("").is_none());
        // Only whitespace
        assert!(parse_skill_md("   \n  \n").is_none());
        // Empty name in frontmatter
        assert!(parse_skill_md("---\ndescription: test\n---\n").is_none());
        // No closing --- in frontmatter — has "---" prefix so uses frontmatter path only
        assert!(parse_skill_md("---\nname: test\n").is_none());
    }

    #[test]
    fn test_parse_skill_md_heading_fallback() {
        // Heading-only SKILL.md (no frontmatter)
        let entry = parse_skill_md("# My Cool Skill\n\nDoes cool things.\n").unwrap();
        assert_eq!(entry.name, "My Cool Skill");
        assert_eq!(entry.description, "Does cool things.");

        // Multi-level heading
        let entry = parse_skill_md("## Sub Skill\n\nSub description\n").unwrap();
        assert_eq!(entry.name, "Sub Skill");
        assert_eq!(entry.description, "Sub description");
    }
}
