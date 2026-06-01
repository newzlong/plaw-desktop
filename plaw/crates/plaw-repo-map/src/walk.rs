use std::path::{Path, PathBuf};

use crate::lang::Lang;

/// Walk `root` respecting .gitignore + .ignore + .git/info/exclude. Returns
/// (abs, rel) tuples for files whose extension maps to a supported language.
pub fn walk_supported(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    use ignore::WalkBuilder;

    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .build();
    for dent in walker.flatten() {
        if !dent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let abs = dent.path().to_path_buf();
        let rel = abs
            .strip_prefix(root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| abs.clone());
        if Lang::from_path(&rel).is_some() {
            out.push((abs, rel));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn walk_finds_only_supported_extensions() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "pub fn a(){}").unwrap();
        std::fs::write(dir.path().join("b.py"), "def b(): pass").unwrap();
        std::fs::write(dir.path().join("c.md"), "# hi").unwrap();
        std::fs::write(dir.path().join("d.txt"), "skip").unwrap();

        let found = walk_supported(dir.path());
        let rels: Vec<_> = found.iter().map(|(_, r)| r.clone()).collect();
        assert!(rels.contains(&PathBuf::from("a.rs")));
        assert!(rels.contains(&PathBuf::from("b.py")));
        assert!(!rels.contains(&PathBuf::from("c.md")));
        assert!(!rels.contains(&PathBuf::from("d.txt")));
    }

    #[test]
    fn walk_respects_gitignore() {
        let dir = TempDir::new().unwrap();
        // gitignore alone is honored only inside a git repo; use .ignore for portability.
        std::fs::write(dir.path().join(".ignore"), "ignored.rs\n").unwrap();
        std::fs::write(dir.path().join("kept.rs"), "pub fn a(){}").unwrap();
        std::fs::write(dir.path().join("ignored.rs"), "pub fn b(){}").unwrap();

        let found = walk_supported(dir.path());
        let rels: Vec<_> = found.iter().map(|(_, r)| r.clone()).collect();
        assert!(rels.contains(&PathBuf::from("kept.rs")));
        assert!(!rels.contains(&PathBuf::from("ignored.rs")));
    }
}
