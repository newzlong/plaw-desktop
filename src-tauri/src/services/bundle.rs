use std::path::Path;

/// Extract bundled tar.gz archives on first run.
pub fn extract_bundle_if_needed(data_dir: &Path) {
    let bundles: &[(&str, &str)] = &[
        ("agent-browser-bundle.tar.gz", "agent-browser"),
        ("browsers-bundle.tar.gz", "browsers"),
        ("python-bundle.tar.gz", "python"),
        ("pandoc-bundle.tar.gz", "pandoc"),
        ("libreoffice-bundle.tar.gz", "libreoffice"),
        ("skills-bundle.tar.gz", ".plaw"),
        ("node-modules-bundle.tar.gz", "node_modules_global"),
        ("poppler-bundle.tar.gz", "poppler"),
        ("cli-bundle.tar.gz", "bin/cli"),
    ];
    for (archive_name, check_dir) in bundles {
        let archive_path = data_dir.join(archive_name);
        let target_dir = data_dir.join(check_dir);
        if !archive_path.exists() {
            continue;
        }
        if target_dir.is_dir() && dir_has_subdirs(&target_dir) {
            continue;
        }
        eprintln!("[plaw] Extracting {} ...", archive_name);
        if let Err(e) = extract_tar_gz(&archive_path, data_dir) {
            eprintln!("[plaw] Failed to extract {}: {}", archive_name, e);
        } else {
            eprintln!("[plaw] Extracted {} successfully", archive_name);
            let _ = std::fs::remove_file(&archive_path);
        }
    }
}

fn dir_has_subdirs(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn extract_tar_gz(
    archive_path: &Path,
    target_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(archive_path)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(target_dir)?;
    Ok(())
}

/// Kill orphaned browser daemon and chrome-headless-shell processes.
pub async fn kill_browser_orphans() {
    use tokio::process::Command;
    use std::process::Stdio;
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "chrome-headless-shell.exe"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status().await;
        let _ = Command::new("wmic")
            .args(["process", "where",
                   "name='node.exe' and commandline like '%daemon.js%'",
                   "delete"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status().await;
    }
}
