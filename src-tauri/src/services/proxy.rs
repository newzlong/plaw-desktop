use std::path::Path;

/// Detect proxy URL with priority: user-configured > env vars > config.toml
pub fn detect_proxy(data_dir: &Path) -> Option<String> {
    // 1. User-configured proxy (from UI settings, saved in settings.json)
    let settings_path = data_dir.join("settings.json");
    if let Ok(content) = std::fs::read_to_string(&settings_path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(proxy) = val.get("market_proxy").and_then(|v| v.as_str()) {
                if !proxy.is_empty() {
                    return Some(proxy.to_string());
                }
            }
        }
    }

    // 2. Environment variables
    let from_env = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .ok()
        .filter(|s| !s.is_empty());

    if from_env.is_some() {
        return from_env;
    }

    // 3. config.toml [proxy] section
    let config_path = data_dir.join(".plaw").join("config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let val: toml::Value = content.parse().ok()?;
    let proxy_section = val.get("proxy")?;

    if proxy_section.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }

    proxy_section
        .get("https_proxy")
        .or_else(|| proxy_section.get("http_proxy"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .filter(|s| s.starts_with("http://") || s.starts_with("https://") || s.starts_with("socks"))
        .map(|s| s.to_string())
}
