use std::path::Path;

/// Patch config.toml on startup to ensure required defaults for Plaw.
/// - gateway.require_pairing = false (local-only, no auth)
/// - web_search/web_fetch/http_request enabled if missing
pub fn ensure_config_defaults(data_dir: &Path) {
    let config_path = data_dir.join(".plaw").join("config.toml");
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

    let mut changed = false;

    // gateway.require_pairing = false
    {
        let gateway = table
            .entry("gateway")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let Some(gw) = gateway.as_table_mut() {
            if gw.get("require_pairing").and_then(|v| v.as_bool()) != Some(false) {
                gw.insert("require_pairing".to_string(), toml::Value::Boolean(false));
                changed = true;
            }
        }
    }

    let ensure_enabled = |table: &mut toml::map::Map<String, toml::Value>,
                          key: &str,
                          defaults: Vec<(&str, toml::Value)>,
                          changed: &mut bool| {
        let section = table
            .entry(key.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let Some(sec) = section.as_table_mut() {
            if sec.get("enabled").and_then(|v| v.as_bool()) != Some(true) {
                sec.insert("enabled".into(), toml::Value::Boolean(true));
                *changed = true;
            }
            for (k, v) in defaults {
                if !sec.contains_key(k) {
                    sec.insert(k.into(), v);
                    *changed = true;
                }
            }
        }
    };

    // [web_search]
    ensure_enabled(table, "web_search", vec![
        ("provider", toml::Value::String("bing".into())),
        ("max_results", toml::Value::Integer(5)),
        ("timeout_secs", toml::Value::Integer(15)),
    ], &mut changed);
    if let Some(ws) = table.get_mut("web_search").and_then(|v| v.as_table_mut()) {
        if ws.get("provider").and_then(|v| v.as_str()) == Some("duckduckgo") {
            ws.insert("provider".into(), toml::Value::String("bing".into()));
            changed = true;
        }
    }

    // [web_fetch]
    ensure_enabled(table, "web_fetch", vec![
        ("provider", toml::Value::String("fast_html2md".into())),
        ("timeout_secs", toml::Value::Integer(30)),
    ], &mut changed);

    // [http_request]
    ensure_enabled(table, "http_request", vec![
        ("allow_local", toml::Value::Boolean(true)),
        ("timeout_secs", toml::Value::Integer(120)),
    ], &mut changed);

    // autonomy
    {
        let autonomy = table
            .entry("autonomy")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let Some(sec) = autonomy.as_table_mut() {
            if sec.get("level").and_then(|v| v.as_str()) == Some("readonly") {
                sec.insert("level".into(), toml::Value::String("supervised".into()));
                changed = true;
            }
            // Own the level string so the mutating branches below don't
            // collide with an immutable borrow of `sec`.
            let level = sec
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("supervised")
                .to_string();
            let (cmds_empty, has_wildcard) = sec
                .get("allowed_commands")
                .and_then(|v| v.as_array())
                .map(|a| (
                    a.is_empty(),
                    a.iter().any(|v| v.as_str() == Some("*")),
                ))
                .unwrap_or((true, false));

            if level == "full" && !has_wildcard {
                sec.insert("allowed_commands".into(),
                    toml::Value::Array(vec![toml::Value::String("*".into())]));
                changed = true;
            } else if level != "full" && cmds_empty {
                let defaults: Vec<toml::Value> = [
                    "git", "ls", "cat", "grep", "find", "head", "tail", "wc",
                    "echo", "pwd", "date", "cargo", "npm", "pnpm", "node",
                    "python", "python3", "pip", "mkdir", "cp", "mv", "touch",
                    "tar", "unzip", "which", "env",
                    "sort", "uniq", "awk", "sed", "tr", "cut", "xargs",
                    "du", "df", "file", "basename", "dirname", "realpath",
                ].iter().map(|s| toml::Value::String(s.to_string())).collect();
                sec.insert("allowed_commands".into(), toml::Value::Array(defaults));
                changed = true;
            }
            // Default to filesystem confinement; respect an explicit user choice either way.
            if sec.get("workspace_only").is_none() {
                sec.insert("workspace_only".into(), toml::Value::Boolean(true));
                changed = true;
            }

            // Stage 6 collapse migration: the autonomy tier no longer
            // short-circuits the approval gate. To preserve the "no prompts"
            // behavior that pre-Stage-6 `level = "full"` users relied on,
            // seed the literal "*" wildcard into auto_approve once. After
            // that, the user manages opt-in/out from the SecurityConfig UI.
            if level == "full" {
                let mut list: Vec<toml::Value> = sec
                    .get("auto_approve")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let already_wild = list.iter().any(|v| v.as_str() == Some("*"));
                if !already_wild {
                    list.push(toml::Value::String("*".into()));
                    sec.insert("auto_approve".into(), toml::Value::Array(list));
                    changed = true;
                }
            }
        }
    }

    if changed {
        if let Ok(s) = toml::to_string_pretty(&val) {
            let _ = std::fs::write(&config_path, s);
        }
    }
}
