use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ranking::RankedTag;

pub struct RenderParams {
    pub max_line_len: usize,
    pub line_margin: char,
    pub ellipsis: &'static str,
}

impl Default for RenderParams {
    fn default() -> Self {
        Self {
            max_line_len: 100,
            line_margin: '│',
            ellipsis: "⋮...",
        }
    }
}

pub trait SourceLoader {
    fn load(&self, rel_path: &Path) -> Option<String>;
}

pub struct DiskSourceLoader {
    root: PathBuf,
}

impl DiskSourceLoader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl SourceLoader for DiskSourceLoader {
    fn load(&self, rel_path: &Path) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel_path)).ok()
    }
}

/// In-memory map of rel-path → source string. Useful for tests + cases where
/// the caller has already read files for parsing.
pub struct InMemorySourceLoader<'a>(pub &'a std::collections::HashMap<PathBuf, String>);

impl SourceLoader for InMemorySourceLoader<'_> {
    fn load(&self, rel_path: &Path) -> Option<String> {
        self.0.get(rel_path).cloned()
    }
}

pub fn render<L: SourceLoader>(ranked: &[RankedTag], loader: &L, params: &RenderParams) -> String {
    // Group by rel_path, preserving insertion order via BTreeMap then
    // re-ordering by first appearance in `ranked`.
    let mut first_seen: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut groups: BTreeMap<PathBuf, Vec<&RankedTag>> = BTreeMap::new();
    for (i, t) in ranked.iter().enumerate() {
        first_seen.entry(t.rel_path.clone()).or_insert(i);
        groups.entry(t.rel_path.clone()).or_default().push(t);
    }

    let mut order: Vec<(PathBuf, usize)> = first_seen.into_iter().collect();
    order.sort_by_key(|(_, i)| *i);

    let mut out = String::new();
    for (path, _) in order {
        let tags = &groups[&path];
        let header = format!("{}:\n", path.display());
        out.push_str(&header);

        let Some(source) = loader.load(&path) else {
            // File listed but no source → emit just the filename block.
            out.push('\n');
            continue;
        };
        let lines: Vec<&str> = source.lines().collect();

        // Collect distinct line numbers (LOIs). Sort + dedupe.
        let mut lois: Vec<usize> = tags
            .iter()
            .filter_map(|t| {
                // Tags carry rows from tree-sitter (0-indexed). Skip out-of-range.
                if t.rel_path == path && (t.ident_line() < lines.len()) {
                    Some(t.ident_line())
                } else {
                    None
                }
            })
            .collect();
        lois.sort_unstable();
        lois.dedup();

        if lois.is_empty() {
            out.push('\n');
            continue;
        }

        let mut prev: Option<usize> = None;
        for lineno in lois {
            let need_ellipsis = match prev {
                None => true,
                Some(p) => lineno > p + 1,
            };
            if need_ellipsis {
                out.push_str(params.ellipsis);
                out.push('\n');
            }
            let raw = lines[lineno];
            let truncated: String = raw.chars().take(params.max_line_len).collect();
            out.push(params.line_margin);
            out.push_str(&truncated);
            out.push('\n');
            prev = Some(lineno);
        }
        out.push_str(params.ellipsis);
        out.push('\n');
        out.push('\n');
    }
    out
}

impl RankedTag {
    pub fn ident_line(&self) -> usize {
        self.line.unwrap_or(0)
    }

    pub fn with_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn renders_single_file_with_ellipsis() {
        let src = "line0\nline1\nfn foo() {}\nline3\nfn bar() {}\nline5\n";
        let mut map = HashMap::new();
        map.insert(PathBuf::from("a.rs"), src.to_string());
        let loader = InMemorySourceLoader(&map);

        let tags = vec![
            RankedTag {
                rel_path: PathBuf::from("a.rs"),
                ident: "foo".into(),
                score: 1.0,
                line: Some(2),
            },
            RankedTag {
                rel_path: PathBuf::from("a.rs"),
                ident: "bar".into(),
                score: 0.5,
                line: Some(4),
            },
        ];

        let out = render(&tags, &loader, &RenderParams::default());
        assert!(out.contains("a.rs:"));
        assert!(out.contains("│fn foo() {}"));
        assert!(out.contains("│fn bar() {}"));
        // Non-adjacent (line 2 → 4) ⇒ ellipsis between.
        assert!(out.contains("⋮..."));
    }

    #[test]
    fn truncates_long_lines() {
        let long = "x".repeat(200);
        let src = format!("fn aaa() {{ {} }}\n", long);
        let mut map = HashMap::new();
        map.insert(PathBuf::from("z.rs"), src);
        let loader = InMemorySourceLoader(&map);
        let tags = vec![RankedTag {
            rel_path: PathBuf::from("z.rs"),
            ident: "aaa".into(),
            score: 1.0,
            line: Some(0),
        }];
        let out = render(&tags, &loader, &RenderParams::default());
        let body_line = out.lines().find(|l| l.starts_with('│')).unwrap();
        // 100 chars + 1 for the margin
        assert!(
            body_line.chars().count() <= 101,
            "got {} chars",
            body_line.chars().count()
        );
    }

    #[test]
    fn missing_source_still_emits_header() {
        let map: HashMap<PathBuf, String> = HashMap::new();
        let loader = InMemorySourceLoader(&map);
        let tags = vec![RankedTag {
            rel_path: PathBuf::from("ghost.rs"),
            ident: "x".into(),
            score: 1.0,
            line: Some(0),
        }];
        let out = render(&tags, &loader, &RenderParams::default());
        assert!(out.contains("ghost.rs:"));
    }
}
