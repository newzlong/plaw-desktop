// Clawhub skill-registry HTTP client. Consumed only by the
// `skills/mod.rs` install / search functions, which are themselves
// reached via the `plaw skills ...` CLI handler. Same reasoning as
// the module-level allow on skills/mod.rs — the entire surface is
// CLI plumbing, invisible to `cargo build --lib`'s dead-code pass.
#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use tracing::debug;

const CLAWHUB_REGISTRY: &str = "https://clawhub.ai";

#[derive(Debug, Deserialize)]
pub struct SearchResult {
    pub slug: String,
    #[serde(alias = "displayName", default)]
    pub display_name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

/// Search ClawHub for skills matching the given query.
pub fn search(query: &str, limit: u32) -> Result<Vec<SearchResult>> {
    let url = format!(
        "{}/api/v1/search?q={}&limit={}",
        CLAWHUB_REGISTRY,
        urlencoding::encode(query),
        limit
    );

    debug!(url = %url, "Searching ClawHub");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .context("failed to send search request to ClawHub")?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "ClawHub search failed with status {}: {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    let body: SearchResponse = resp
        .json()
        .context("failed to parse ClawHub search response")?;

    Ok(body.results)
}

/// Download a skill from ClawHub as a ZIP and extract it to `skills_path/<slug>/`.
pub fn download_and_install(slug: &str, skills_path: &Path) -> Result<(std::path::PathBuf, usize)> {
    let url = format!(
        "{}/api/v1/download?slug={}",
        CLAWHUB_REGISTRY,
        urlencoding::encode(slug),
    );

    debug!(url = %url, slug = %slug, "Downloading skill from ClawHub");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("failed to build HTTP client")?;

    let resp = client
        .get(&url)
        .send()
        .context("failed to download skill from ClawHub")?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "ClawHub download failed for '{}' with status {}: {}",
            slug,
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    let zip_bytes = resp.bytes().context("failed to read download response")?;

    let dest_dir = skills_path.join(slug);
    if dest_dir.exists() {
        std::fs::remove_dir_all(&dest_dir)
            .with_context(|| format!("failed to remove existing skill dir: {}", dest_dir.display()))?;
    }
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("failed to create skill dir: {}", dest_dir.display()))?;

    let cursor = std::io::Cursor::new(&zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).context("failed to read ZIP archive")?;

    let mut files_extracted = 0usize;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("failed to read ZIP entry")?;

        let raw_name = file.name().to_string();

        // Strip top-level directory if present (common in GitHub-style ZIPs)
        let relative = strip_top_level_dir(&raw_name);

        // Security: reject path traversal
        if relative.contains("..") {
            debug!(entry = %raw_name, "Skipping ZIP entry with path traversal");
            continue;
        }

        let out_path = dest_dir.join(relative);

        if file.is_dir() {
            std::fs::create_dir_all(&out_path).ok();
            continue;
        }

        // Ensure parent directory exists
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let mut out_file = std::fs::File::create(&out_path)
            .with_context(|| format!("failed to create file: {}", out_path.display()))?;
        std::io::copy(&mut file, &mut out_file)
            .with_context(|| format!("failed to write file: {}", out_path.display()))?;

        files_extracted += 1;
    }

    Ok((dest_dir, files_extracted))
}

/// Strip the first path component if all entries share the same top-level directory.
fn strip_top_level_dir(path: &str) -> &str {
    if let Some(idx) = path.find('/') {
        let rest = &path[idx + 1..];
        if rest.is_empty() {
            return rest;
        }
        return rest;
    }
    path
}

/// Check whether a source string looks like a ClawHub slug (not a URL, not a local path).
pub fn is_clawhub_slug(source: &str) -> bool {
    // A ClawHub slug is a simple identifier: alphanumeric, hyphens, underscores
    // It is NOT a URL (no ://) and NOT a local path (no / or \ at start, no . prefix)
    if source.is_empty() {
        return false;
    }
    if source.contains("://") || source.starts_with('.') || source.starts_with('/') || source.starts_with('\\') {
        return false;
    }
    // Must look like a slug: only alphanumeric, hyphens, underscores, dots
    source.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
}
