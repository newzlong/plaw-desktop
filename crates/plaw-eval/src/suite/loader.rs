//! TOML loader for eval suites.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use super::case::Suite;

/// Schema major version we know how to read. Increment with breaking
/// changes to `Suite` / `Case`. See `version.rs`.
pub const SUITE_SCHEMA_MAJOR: u32 = 1;

/// Load a single suite from a `cases.toml` file.
pub fn load_suite(path: impl AsRef<Path>) -> Result<Suite> {
    let path = path.as_ref();
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading suite file {}", path.display()))?;
    let suite: Suite = toml::from_str(&raw)
        .with_context(|| format!("parsing suite TOML at {}", path.display()))?;
    super::version::ensure_compatible(&suite.version, SUITE_SCHEMA_MAJOR)
        .with_context(|| format!("incompatible suite version in {}", path.display()))?;
    if suite.cases.is_empty() {
        return Err(anyhow!("suite '{}' has no cases", suite.name));
    }
    if !duplicate_case_ids(&suite).is_empty() {
        return Err(anyhow!(
            "suite '{}' has duplicate case ids: {:?}",
            suite.name,
            duplicate_case_ids(&suite)
        ));
    }
    Ok(suite)
}

/// Scan a directory for `cases.toml` files (one per sub-directory) and load
/// each as a suite. Suites that fail to load surface their error rather
/// than poisoning the entire scan.
pub fn discover_suites(root: impl AsRef<Path>) -> Result<Vec<(PathBuf, Suite)>> {
    let root = root.as_ref();
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in
        fs::read_dir(root).with_context(|| format!("reading suite root {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let cases = path.join("cases.toml");
        if !cases.exists() {
            continue;
        }
        let suite = load_suite(&cases)?;
        out.push((cases, suite));
    }
    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    Ok(out)
}

fn duplicate_case_ids(suite: &Suite) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut dups = Vec::new();
    for c in &suite.cases {
        if !seen.insert(c.id.as_str()) {
            dups.push(c.id.clone());
        }
    }
    dups
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_suite(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let sub = dir.join(name);
        fs::create_dir_all(&sub).unwrap();
        let path = sub.join("cases.toml");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    fn minimal_suite_toml(name: &str, version: &str) -> String {
        format!(
            r#"
                name = "{name}"
                version = "{version}"
                description = "test suite"

                [default_judge]
                model = "kimi-k2.5"
                provider = "kimi"
                mode = {{ kind = "pairwise", dual_pass = true }}

                [[cases]]
                id = "c1"
                input = {{ kind = "chat", messages = [
                    {{ role = "user", content = "hi" }}
                ] }}
            "#
        )
    }

    #[test]
    fn loads_minimal_suite() {
        let tmp = tempdir();
        let path = write_temp_suite(&tmp, "smoke", &minimal_suite_toml("smoke", "1.0.0"));
        let suite = load_suite(&path).unwrap();
        assert_eq!(suite.name, "smoke");
        assert_eq!(suite.cases.len(), 1);
    }

    #[test]
    fn rejects_incompatible_major_version() {
        let tmp = tempdir();
        let path = write_temp_suite(&tmp, "future", &minimal_suite_toml("future", "2.0.0"));
        let err = load_suite(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("incompatible") || msg.contains("major"));
    }

    #[test]
    fn rejects_duplicate_case_ids() {
        let tmp = tempdir();
        let toml_src = r#"
            name = "dup"
            version = "1.0.0"
            description = ""

            [default_judge]
            model = "kimi-k2.5"
            provider = "kimi"
            mode = { kind = "pairwise", dual_pass = true }

            [[cases]]
            id = "x"
            input = { kind = "chat", messages = [{ role = "user", content = "a" }] }
            [[cases]]
            id = "x"
            input = { kind = "chat", messages = [{ role = "user", content = "b" }] }
        "#;
        let path = write_temp_suite(&tmp, "dup", toml_src);
        let err = load_suite(&path).unwrap_err();
        assert!(format!("{err:#}").contains("duplicate"));
    }

    #[test]
    fn discover_finds_multiple_suites() {
        let tmp = tempdir();
        write_temp_suite(&tmp, "alpha", &minimal_suite_toml("alpha", "1.0.0"));
        write_temp_suite(&tmp, "beta", &minimal_suite_toml("beta", "1.0.0"));
        let suites = discover_suites(&tmp).unwrap();
        assert_eq!(suites.len(), 2);
        assert_eq!(suites[0].1.name, "alpha");
        assert_eq!(suites[1].1.name, "beta");
    }

    #[test]
    fn discover_empty_returns_empty_list() {
        let tmp = tempdir();
        let suites = discover_suites(&tmp).unwrap();
        assert!(suites.is_empty());
    }

    /// Tiny, dependency-free temporary directory in std::env::temp_dir().
    /// Cleanup is best-effort on drop; tests are isolated by unique paths.
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("plaw-eval-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
