use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// A knowledge entry parsed from a Markdown file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub source: String,
    /// First ~200 chars of content (preview)
    #[serde(default)]
    pub preview: String,
    /// Full file path
    #[serde(default)]
    pub path: String,
}

/// Parse YAML frontmatter from a Markdown file
fn parse_frontmatter(content: &str) -> Option<(String, Vec<String>, String, String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after = &trimmed[3..];
    let end = after.find("\n---")?;
    let yaml = &after[..end];

    let mut title = String::new();
    let mut tags = Vec::new();
    let mut created = String::new();
    let mut updated = String::new();
    let mut source = String::new();

    for line in yaml.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("title:") {
            title = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("created:") {
            created = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("updated:") {
            updated = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("source:") {
            source = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("tags:") {
            // Parse inline array: [tag1, tag2]
            let val = val.trim();
            if val.starts_with('[') && val.ends_with(']') {
                let inner = &val[1..val.len() - 1];
                tags = inner
                    .split(',')
                    .map(|t| t.trim().trim_matches('"').to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
            }
        }
    }

    Some((title, tags, created, updated, source))
}

/// Extract body content (after frontmatter)
fn extract_body(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after = &trimmed[3..];
    if let Some(end) = after.find("\n---") {
        let rest = &after[end + 4..];
        rest.trim_start_matches('\n').trim_start_matches('\r')
    } else {
        content
    }
}

/// Get the knowledge directory path
pub fn knowledge_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(".plaw").join("knowledge")
}

/// List all knowledge entries
pub fn list_entries(data_dir: &Path) -> Vec<KnowledgeEntry> {
    let dir = knowledge_dir(data_dir);
    let mut entries = Vec::new();

    let read_dir = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(_) => return entries,
    };

    for item in read_dir.take(5000) {
        let item = match item {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = item.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "md" | "txt") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) if c.len() <= 512_000 => c,
            _ => continue,
        };

        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let (title, tags, created, updated, source) =
            parse_frontmatter(&content).unwrap_or_default();

        let body = extract_body(&content);
        let preview: String = body.chars().take(200).collect();

        entries.push(KnowledgeEntry {
            id,
            title: if title.is_empty() {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled")
                    .to_string()
            } else {
                title
            },
            tags,
            created,
            updated,
            source,
            preview,
            path: path.display().to_string(),
        });
    }

    // Sort by updated date descending
    entries.sort_by(|a, b| b.updated.cmp(&a.updated));
    entries
}

/// Search knowledge entries by keyword
pub fn search_entries(data_dir: &Path, query: &str) -> Vec<KnowledgeEntry> {
    let all = list_entries(data_dir);
    if query.is_empty() {
        return all;
    }
    let q = query.to_lowercase();
    all.into_iter()
        .filter(|e| {
            e.title.to_lowercase().contains(&q)
                || e.preview.to_lowercase().contains(&q)
                || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
        .collect()
}

/// Read a single knowledge entry by ID (filename without extension)
pub fn read_entry(data_dir: &Path, id: &str) -> Result<(KnowledgeEntry, String), String> {
    let dir = knowledge_dir(data_dir);
    let path = dir.join(format!("{id}.md"));
    if !path.exists() {
        let path_txt = dir.join(format!("{id}.txt"));
        if !path_txt.exists() {
            return Err("Entry not found".to_string());
        }
        return read_entry_from_path(&path_txt, id);
    }
    read_entry_from_path(&path, id)
}

fn read_entry_from_path(path: &Path, id: &str) -> Result<(KnowledgeEntry, String), String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read: {e}"))?;

    let (title, tags, created, updated, source) =
        parse_frontmatter(&content).unwrap_or_default();
    let body = extract_body(&content).to_string();
    let preview: String = body.chars().take(200).collect();

    Ok((
        KnowledgeEntry {
            id: id.to_string(),
            title: if title.is_empty() {
                id.to_string()
            } else {
                title
            },
            tags,
            created,
            updated,
            source,
            preview,
            path: path.display().to_string(),
        },
        body,
    ))
}

/// Save (create or update) a knowledge entry
pub fn save_entry(
    data_dir: &Path,
    title: &str,
    tags: &[String],
    content: &str,
    id: Option<&str>,
) -> Result<KnowledgeEntry, String> {
    let dir = knowledge_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create dir: {e}"))?;

    let today = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let days = now / 86400;
        let y = 1970 + (days * 400 / 146097);
        // Simplified: just use chrono-free date
        format!(
            "{:04}-{:02}-{:02}",
            y,
            (days % 365 / 30) + 1,
            (days % 30) + 1
        )
    };

    // Generate or reuse ID
    let entry_id = if let Some(existing) = id {
        existing.to_string()
    } else {
        // Slugify title
        let slug: String = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let slug = slug.trim_matches('-').to_string();
        let slug = if slug.is_empty() {
            format!("entry-{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis())
        } else if slug.len() > 60 {
            slug[..60].trim_matches('-').to_string()
        } else {
            slug
        };
        // Deduplicate
        if dir.join(format!("{slug}.md")).exists() {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            format!("{slug}-{ts}")
        } else {
            slug
        }
    };

    let tags_str = tags
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(", ");

    let is_update = id.is_some() && dir.join(format!("{entry_id}.md")).exists();
    let created = if is_update {
        // Preserve original created date
        let path = dir.join(format!("{entry_id}.md"));
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| parse_frontmatter(&c))
            .map(|(_, _, c, _, _)| c)
            .unwrap_or_else(|| today.clone())
    } else {
        today.clone()
    };

    let file_content = format!(
        "---\ntitle: \"{title}\"\ntags: [{tags_str}]\ncreated: \"{created}\"\nupdated: \"{today}\"\nsource: \"manual\"\n---\n\n{content}\n"
    );

    let path = dir.join(format!("{entry_id}.md"));
    std::fs::write(&path, &file_content).map_err(|e| format!("Failed to write: {e}"))?;

    let preview: String = content.chars().take(200).collect();
    Ok(KnowledgeEntry {
        id: entry_id,
        title: title.to_string(),
        tags: tags.to_vec(),
        created,
        updated: today,
        source: "manual".to_string(),
        preview,
        path: path.display().to_string(),
    })
}

/// Delete a knowledge entry
pub fn delete_entry(data_dir: &Path, id: &str) -> Result<(), String> {
    let dir = knowledge_dir(data_dir);
    let path = dir.join(format!("{id}.md"));
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete: {e}"))?;
        return Ok(());
    }
    let path_txt = dir.join(format!("{id}.txt"));
    if path_txt.exists() {
        std::fs::remove_file(&path_txt).map_err(|e| format!("Failed to delete: {e}"))?;
        return Ok(());
    }
    Err("Entry not found".to_string())
}

/// Get knowledge base stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeStats {
    pub total_entries: usize,
    pub total_tags: usize,
    pub top_tags: Vec<(String, usize)>,
    pub dir_path: String,
}

pub fn get_stats(data_dir: &Path) -> KnowledgeStats {
    let entries = list_entries(data_dir);
    let mut tag_counts = std::collections::HashMap::new();
    for entry in &entries {
        for tag in &entry.tags {
            *tag_counts.entry(tag.clone()).or_insert(0usize) += 1;
        }
    }
    let mut top_tags: Vec<(String, usize)> = tag_counts.into_iter().collect();
    top_tags.sort_by(|a, b| b.1.cmp(&a.1));
    top_tags.truncate(20);

    KnowledgeStats {
        total_entries: entries.len(),
        total_tags: top_tags.len(),
        top_tags,
        dir_path: knowledge_dir(data_dir).display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_data_dir(name: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("plaw-kb-test-{name}"));
        let _ = fs::remove_dir_all(&tmp);
        let kb_dir = tmp.join(".plaw").join("knowledge");
        fs::create_dir_all(&kb_dir).unwrap();
        tmp
    }

    #[test]
    fn test_parse_frontmatter_valid() {
        let content = r#"---
title: "Rust async patterns"
tags: [rust, async, tokio]
created: "2026-03-08"
updated: "2026-03-08"
source: "conversation"
---

Some content about async patterns.
"#;
        let (title, tags, created, updated, source) = parse_frontmatter(content).unwrap();
        assert_eq!(title, "Rust async patterns");
        assert_eq!(tags, vec!["rust", "async", "tokio"]);
        assert_eq!(created, "2026-03-08");
        assert_eq!(updated, "2026-03-08");
        assert_eq!(source, "conversation");
        println!("  title: {title}");
        println!("  tags: {tags:?}");
    }

    #[test]
    fn test_parse_frontmatter_missing() {
        assert!(parse_frontmatter("# No frontmatter").is_none());
        assert!(parse_frontmatter("---\ntitle: test\n").is_none()); // no closing ---
    }

    #[test]
    fn test_extract_body() {
        let content = "---\ntitle: test\n---\n\nBody content here.";
        let body = extract_body(content);
        assert_eq!(body, "Body content here.");
    }

    #[test]
    fn test_list_and_search_entries() {
        let data_dir = test_data_dir("list-search");
        let kb_dir = data_dir.join(".plaw").join("knowledge");

        // Write two test entries
        fs::write(kb_dir.join("rust-patterns.md"), r#"---
title: "Rust Patterns"
tags: [rust, patterns]
created: "2026-03-01"
updated: "2026-03-08"
source: "conversation"
---

Use match for pattern matching.
"#).unwrap();

        fs::write(kb_dir.join("python-tips.md"), r#"---
title: "Python Tips"
tags: [python]
created: "2026-03-05"
updated: "2026-03-05"
source: "conversation"
---

Use list comprehensions for concise code.
"#).unwrap();

        let entries = list_entries(&data_dir);
        assert_eq!(entries.len(), 2);
        println!("  Found {} entries:", entries.len());
        for e in &entries {
            println!("    - {} (tags: {:?})", e.title, e.tags);
        }

        // Search
        let rust_entries = search_entries(&data_dir, "rust");
        assert_eq!(rust_entries.len(), 1);
        assert_eq!(rust_entries[0].title, "Rust Patterns");

        let python_entries = search_entries(&data_dir, "python");
        assert_eq!(python_entries.len(), 1);

        let all_entries = search_entries(&data_dir, "");
        assert_eq!(all_entries.len(), 2);

        // Cleanup
        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn test_read_and_delete_entry() {
        let data_dir = test_data_dir("read-delete");
        let kb_dir = data_dir.join(".plaw").join("knowledge");

        fs::write(kb_dir.join("test-entry.md"), r#"---
title: "Test Entry"
tags: [test]
created: "2026-03-08"
updated: "2026-03-08"
source: "test"
---

Test body content.
"#).unwrap();

        let (entry, body) = read_entry(&data_dir, "test-entry").unwrap();
        assert_eq!(entry.title, "Test Entry");
        assert_eq!(body, "Test body content.\n");
        println!("  Read entry: {} / body: {}", entry.title, body.trim());

        // Delete
        delete_entry(&data_dir, "test-entry").unwrap();
        assert!(read_entry(&data_dir, "test-entry").is_err());
        println!("  Deleted successfully");

        // Cleanup
        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn test_get_stats() {
        let data_dir = test_data_dir("stats");
        let kb_dir = data_dir.join(".plaw").join("knowledge");

        fs::write(kb_dir.join("a.md"), "---\ntitle: A\ntags: [rust, web]\nupdated: \"2026-03-08\"\n---\nA").unwrap();
        fs::write(kb_dir.join("b.md"), "---\ntitle: B\ntags: [rust, cli]\nupdated: \"2026-03-07\"\n---\nB").unwrap();

        let stats = get_stats(&data_dir);
        assert_eq!(stats.total_entries, 2);
        assert!(stats.top_tags.iter().any(|(t, c)| t == "rust" && *c == 2));
        println!("  Stats: {} entries, top tags: {:?}", stats.total_entries, stats.top_tags);

        let _ = fs::remove_dir_all(&data_dir);
    }
}
