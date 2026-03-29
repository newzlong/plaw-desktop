use tauri::Emitter;
use crate::AppState;
use crate::skills;
use crate::services::proxy::detect_proxy;

#[tauri::command]
pub fn list_local_skills(state: tauri::State<AppState>) -> Vec<skills::SkillEntry> {
    skills::list_local_skills(&state.data_dir)
}

#[tauri::command]
pub async fn install_skill(
    state: tauri::State<'_, AppState>,
    path_or_url: String,
) -> Result<String, String> {
    let proxy_url = detect_proxy(&state.data_dir);

    let name = if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
        skills::install_skill_from_url(
            &state.data_dir,
            &path_or_url,
            proxy_url.as_deref(),
        ).await?
    } else {
        skills::install_skill_from_path(
            &state.data_dir,
            std::path::Path::new(&path_or_url),
        )?
    };

    let data_dir = state.data_dir.clone();
    let skill_name = name.clone();
    let proxy = detect_proxy(&data_dir);
    tokio::spawn(async move {
        if let Err(e) = auto_audit_skill(&data_dir, &skill_name, proxy.as_deref()).await {
            eprintln!("[plaw] Auto-audit failed for {skill_name}: {e}");
        }
    });

    Ok(name)
}

#[tauri::command]
pub fn uninstall_skill(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    skills::uninstall_skill(&state.data_dir, &name)
}

#[tauri::command]
pub async fn audit_skill(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<skills::AuditResult, String> {
    let skill_md = skills::resolve_skill_md(&state.data_dir, &name)?;
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;

    let proxy_url = detect_proxy(&state.data_dir);
    let result = skills::audit_skill_content(
        &state.data_dir,
        &content,
        proxy_url.as_deref(),
    ).await?;

    let new_content = skills::inject_audit_tags(&content, &result.compatibility, &result.risk);
    std::fs::write(&skill_md, new_content)
        .map_err(|e| format!("Failed to write tags to SKILL.md: {e}"))?;

    Ok(result)
}

#[tauri::command]
pub async fn audit_all_unaudited(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    force: Option<bool>,
) -> Result<u32, String> {
    let force = force.unwrap_or(false);
    let all_skills = skills::list_local_skills(&state.data_dir);
    let data_dir = state.data_dir.clone();
    let proxy = detect_proxy(&data_dir);

    const SKIP_BUILTIN: &[&str] = &[
        "find-skills", "skill-creator", "audit-skills", "fix-skills",
        "pptx", "xlsx", "docx", "pdf",
    ];

    let mut count = 0u32;
    for skill in &all_skills {
        if SKIP_BUILTIN.contains(&skill.name.as_str()) { continue; }
        if !force && !skill.compatibility.is_empty() { continue; }
        count += 1;
        let dd = data_dir.clone();
        let slug = std::path::Path::new(&skill.path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| skill.name.clone());
        let px = proxy.clone();
        let stagger = count;
        let ah = app_handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(stagger as u64 * 500)).await;
            let success = force_audit_skill(&dd, &slug, px.as_deref()).await.is_ok();
            let _ = ah.emit("skill-audited", serde_json::json!({
                "name": slug,
                "success": success,
            }));
        });
    }

    eprintln!("[plaw] Queued {count} skills for audit (force={force})");
    Ok(count)
}

#[derive(serde::Serialize)]
pub struct RegistrySearchResult {
    pub skills: Vec<skills::RegistrySkill>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[tauri::command]
pub async fn search_registry_skills(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<RegistrySearchResult, String> {
    let proxy_url = detect_proxy(&state.data_dir);

    match skills::fetch_github_skills(proxy_url.as_deref()).await {
        Ok(online_skills) => {
            let query_lower = query.to_lowercase();
            let results = online_skills.into_iter()
                .filter(|s| {
                    query_lower.is_empty()
                        || s.name.to_lowercase().contains(&query_lower)
                        || s.description.to_lowercase().contains(&query_lower)
                })
                .collect();
            Ok(RegistrySearchResult { skills: results, source: "online".to_string(), error: None })
        }
        Err(e) => {
            eprintln!("[plaw] GitHub API failed, falling back to local: {e}");
            let results = skills::search_local_skills(&state.data_dir, &query);
            Ok(RegistrySearchResult { skills: results, source: "local".to_string(), error: Some(e) })
        }
    }
}

#[tauri::command]
pub async fn sync_skills_registry(
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    let open_skills_dir = state.data_dir.join("open-skills");

    if open_skills_dir.join(".git").exists() {
        let output = std::process::Command::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(&open_skills_dir)
            .output()
            .map_err(|e| format!("Failed to run git pull: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git pull failed: {stderr}"));
        }
    } else {
        std::fs::create_dir_all(&open_skills_dir)
            .map_err(|e| format!("Failed to create dir: {e}"))?;
        let output = std::process::Command::new("git")
            .args([
                "clone", "--depth", "1",
                "https://github.com/besoeasy/open-skills.git",
                &open_skills_dir.display().to_string(),
            ])
            .output()
            .map_err(|e| format!("Failed to run git clone: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git clone failed: {stderr}"));
        }
    }

    let skills_dir = open_skills_dir.join("skills");
    let count = skills::scan_skills_dir(&skills_dir, "open-skills").len() as u32;
    Ok(count)
}

/// Auto-audit a skill (skips already-tagged)
async fn auto_audit_skill(
    data_dir: &std::path::Path,
    name: &str,
    proxy_url: Option<&str>,
) -> Result<(), String> {
    const SKIP_SKILLS: &[&str] = &[
        "find-skills", "skill-creator", "audit-skills", "fix-skills",
        "pptx", "xlsx", "docx", "pdf",
    ];
    if SKIP_SKILLS.contains(&name) { return Ok(()); }

    let skill_md = skills::resolve_skill_md(data_dir, name)?;
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;
    if content.contains("compatibility:") { return Ok(()); }

    eprintln!("[plaw] Auto-auditing skill: {name}");
    let result = skills::audit_skill_content(data_dir, &content, proxy_url).await?;
    let new_content = skills::inject_audit_tags(&content, &result.compatibility, &result.risk);
    std::fs::write(&skill_md, new_content)
        .map_err(|e| format!("Failed to write audit tags: {e}"))?;
    eprintln!("[plaw] Auto-audit done for {name}: {} / {}", result.compatibility, result.risk);
    Ok(())
}

/// Force-audit a skill (ignores existing tags)
async fn force_audit_skill(
    data_dir: &std::path::Path,
    name: &str,
    proxy_url: Option<&str>,
) -> Result<(), String> {
    let skill_md = skills::resolve_skill_md(data_dir, name)?;
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|e| format!("Failed to read SKILL.md: {e}"))?;
    eprintln!("[plaw] Auditing skill: {name}");
    let result = skills::audit_skill_content(data_dir, &content, proxy_url).await?;
    let new_content = skills::inject_audit_tags(&content, &result.compatibility, &result.risk);
    std::fs::write(&skill_md, new_content)
        .map_err(|e| format!("Failed to write audit tags: {e}"))?;
    eprintln!("[plaw] Audit done for {name}: {} / {}", result.compatibility, result.risk);
    Ok(())
}
