use std::path::PathBuf;

/// Get the portable data directory.
/// Priority: exe beside plaw-data/ (portable mode)
/// Fallback: %LOCALAPPDATA%/plaw-desktop/ (when exe dir is not writable, e.g. Program Files)
pub fn get_data_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        return manifest_dir.parent().unwrap().join("plaw-data");
    }

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                return PathBuf::from(local).join("plaw-desktop");
            }
            return PathBuf::from("plaw-data");
        }
    };
    let install_dir = match exe_path.parent() {
        Some(p) => p,
        None => {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                return PathBuf::from(local).join("plaw-desktop");
            }
            return PathBuf::from("plaw-data");
        }
    };
    let portable_dir = install_dir.join("plaw-data");

    if portable_dir.exists() && is_dir_writable(&portable_dir) {
        return portable_dir;
    }

    if !portable_dir.exists() {
        if std::fs::create_dir_all(&portable_dir).is_ok() && is_dir_writable(&portable_dir) {
            return portable_dir;
        }
        let _ = std::fs::remove_dir(&portable_dir);
    }

    let fallback = dirs_next::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("plaw-desktop");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

fn is_dir_writable(dir: &std::path::Path) -> bool {
    let test_file = dir.join(".write_test");
    match std::fs::write(&test_file, b"test") {
        Ok(()) => {
            let _ = std::fs::remove_file(&test_file);
            true
        }
        Err(_) => false,
    }
}
