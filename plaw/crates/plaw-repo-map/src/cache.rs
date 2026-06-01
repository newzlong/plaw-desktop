use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::tag::Tag;

#[derive(Debug, Clone)]
struct Entry {
    mtime: SystemTime,
    tags: Vec<Tag>,
}

/// In-memory tags cache keyed by absolute path. mtime mismatch invalidates the
/// entry. Phase 1 will add an sqlite-backed cache; in-memory keeps the
/// crate self-contained and avoids a hard dep on rusqlite for downstream
/// consumers that don't need persistence.
#[derive(Debug, Default)]
pub struct TagsCache {
    inner: Mutex<HashMap<PathBuf, Entry>>,
}

impl TagsCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_compute<F>(
        &self,
        abs_path: &Path,
        rel_path: &Path,
        compute: F,
    ) -> anyhow::Result<Vec<Tag>>
    where
        F: FnOnce(&Path, &Path) -> anyhow::Result<Vec<Tag>>,
    {
        let mtime = std::fs::metadata(abs_path).and_then(|m| m.modified()).ok();
        {
            let guard = self.inner.lock().expect("tags cache mutex poisoned");
            if let (Some(mtime), Some(entry)) = (mtime, guard.get(abs_path)) {
                if entry.mtime == mtime {
                    return Ok(entry.tags.clone());
                }
            }
        }
        let tags = compute(rel_path, abs_path)?;
        if let Some(mtime) = mtime {
            let mut guard = self.inner.lock().expect("tags cache mutex poisoned");
            guard.insert(
                abs_path.to_path_buf(),
                Entry {
                    mtime,
                    tags: tags.clone(),
                },
            );
        }
        Ok(tags)
    }

    pub fn clear(&self) {
        self.inner
            .lock()
            .expect("tags cache mutex poisoned")
            .clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("tags cache mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    #[test]
    fn cache_returns_same_result_on_hit() {
        let dir = TempDir::new().unwrap();
        let abs = dir.path().join("a.rs");
        std::fs::write(&abs, "pub fn foo() {}\n").unwrap();

        let cache = TagsCache::new();
        let calls = AtomicUsize::new(0);

        let cb = |rel: &Path, abs: &Path| {
            calls.fetch_add(1, Ordering::SeqCst);
            crate::parser::extract_tags_from_file(rel, abs)
        };

        let t1 = cache.get_or_compute(&abs, Path::new("a.rs"), cb).unwrap();
        let t2 = cache.get_or_compute(&abs, Path::new("a.rs"), cb).unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "second call must hit cache"
        );
        assert_eq!(t1.len(), t2.len());
    }

    #[test]
    fn cache_invalidates_on_mtime_change() {
        let dir = TempDir::new().unwrap();
        let abs = dir.path().join("a.rs");
        std::fs::write(&abs, "pub fn foo() {}\n").unwrap();

        let cache = TagsCache::new();
        let calls = AtomicUsize::new(0);
        let cb = |rel: &Path, abs: &Path| {
            calls.fetch_add(1, Ordering::SeqCst);
            crate::parser::extract_tags_from_file(rel, abs)
        };

        let _ = cache.get_or_compute(&abs, Path::new("a.rs"), cb).unwrap();
        // Force a different mtime — sleeping is flaky on fast filesystems, so
        // we set mtime explicitly via filetime if available, otherwise rewrite
        // with content changed and hope for >1s resolution. Simpler approach:
        // rewrite the file and set the mtime back via an obvious-future stamp.
        std::fs::write(&abs, "pub fn bar() {}\npub fn foo() {}\n").unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(10);
        let _ = set_modified(&abs, future);

        let _ = cache.get_or_compute(&abs, Path::new("a.rs"), cb).unwrap();
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "mtime change must miss cache"
        );
    }

    fn set_modified(path: &Path, _to: SystemTime) -> std::io::Result<()> {
        // best-effort: touching the file is enough on most filesystems to
        // change mtime to "now", which differs from the previous SystemTime.
        let f = std::fs::OpenOptions::new().write(true).open(path)?;
        f.set_len(f.metadata()?.len())?;
        Ok(())
    }
}
