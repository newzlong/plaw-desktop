use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::lang::Lang;
use crate::tag::{Tag, TagKind};

const DEF_CAPTURE_PREFIX: &str = "name.definition.";
const REF_CAPTURE_PREFIX: &str = "name.reference.";

pub fn extract_tags(rel_path: &Path, abs_path: &Path, source: &str) -> Result<Vec<Tag>> {
    let Some(lang) = Lang::from_path(rel_path) else {
        return Ok(Vec::new());
    };

    let language = lang.tree_sitter_language();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .with_context(|| format!("set_language failed for {:?}", rel_path))?;

    let tree = parser
        .parse(source, None)
        .with_context(|| format!("parse returned None for {:?}", rel_path))?;

    let query = Query::new(&language, lang.query_source())
        .with_context(|| format!("query compile failed for {:?}", rel_path))?;

    let capture_names = query.capture_names();
    let bytes = source.as_bytes();
    let mut tags = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let cap_name = capture_names[cap.index as usize];
            let kind = if cap_name.starts_with(DEF_CAPTURE_PREFIX) {
                TagKind::Def
            } else if cap_name.starts_with(REF_CAPTURE_PREFIX) {
                TagKind::Ref
            } else {
                continue;
            };

            let node = cap.node;
            let text = node.utf8_text(bytes).unwrap_or("");
            if text.is_empty() {
                continue;
            }

            tags.push(Tag {
                rel_path: rel_path.to_path_buf(),
                abs_path: abs_path.to_path_buf(),
                line: node.start_position().row,
                name: text.to_string(),
                kind,
            });
        }
    }
    Ok(tags)
}

pub fn extract_tags_from_file(rel_path: &Path, abs_path: &Path) -> Result<Vec<Tag>> {
    let source = std::fs::read_to_string(abs_path)
        .with_context(|| format!("read failed: {:?}", abs_path))?;
    extract_tags(rel_path, abs_path, &source)
}

#[allow(dead_code)]
pub(crate) fn parse_paths_pair(rel: &str, abs: &str) -> (PathBuf, PathBuf) {
    (PathBuf::from(rel), PathBuf::from(abs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rust_struct_and_fn_extracted() {
        let src = r#"
pub struct Foo {
    bar: u32,
}

impl Foo {
    pub fn new() -> Self { Self { bar: 0 } }
}

pub fn make_foo() -> Foo {
    Foo::new()
}
"#;
        let tags = extract_tags(Path::new("a.rs"), Path::new("/tmp/a.rs"), src).unwrap();
        let defs: Vec<_> = tags
            .iter()
            .filter(|t| t.is_def())
            .map(|t| t.name.as_str())
            .collect();
        let refs: Vec<_> = tags
            .iter()
            .filter(|t| t.is_ref())
            .map(|t| t.name.as_str())
            .collect();
        assert!(
            defs.contains(&"Foo"),
            "expected Foo def, got defs={:?}",
            defs
        );
        assert!(defs.contains(&"new"));
        assert!(defs.contains(&"make_foo"));
        assert!(refs.contains(&"new") || refs.contains(&"Foo"));
    }

    #[test]
    fn python_class_and_call_extracted() {
        let src = r#"
class Bar:
    def hello(self):
        print("hi")

def main():
    b = Bar()
    b.hello()
"#;
        let tags = extract_tags(Path::new("a.py"), Path::new("/tmp/a.py"), src).unwrap();
        let defs: Vec<_> = tags
            .iter()
            .filter(|t| t.is_def())
            .map(|t| t.name.as_str())
            .collect();
        let refs: Vec<_> = tags
            .iter()
            .filter(|t| t.is_ref())
            .map(|t| t.name.as_str())
            .collect();
        assert!(defs.contains(&"Bar"));
        assert!(defs.contains(&"hello"));
        assert!(defs.contains(&"main"));
        assert!(refs.contains(&"Bar"));
        assert!(refs.contains(&"hello"));
    }

    #[test]
    fn go_func_and_type_extracted() {
        let src = r#"
package foo

type Item struct {
    name string
}

func NewItem() *Item {
    return &Item{name: "x"}
}

func process() {
    NewItem()
}
"#;
        let tags = extract_tags(Path::new("a.go"), Path::new("/tmp/a.go"), src).unwrap();
        let defs: Vec<_> = tags
            .iter()
            .filter(|t| t.is_def())
            .map(|t| t.name.as_str())
            .collect();
        let refs: Vec<_> = tags
            .iter()
            .filter(|t| t.is_ref())
            .map(|t| t.name.as_str())
            .collect();
        assert!(defs.contains(&"Item"));
        assert!(defs.contains(&"NewItem"));
        assert!(defs.contains(&"process"));
        assert!(refs.contains(&"NewItem"));
    }

    #[test]
    fn unsupported_extension_returns_empty() {
        let tags = extract_tags(Path::new("a.md"), Path::new("/tmp/a.md"), "# hi").unwrap();
        assert!(tags.is_empty());
    }

    #[test]
    fn typescript_interface_extracted() {
        let src = r#"
export interface Foo { bar: number; }
export class Impl implements Foo { bar = 1; }
function use(): Foo { return new Impl(); }
"#;
        let tags = extract_tags(Path::new("a.ts"), Path::new("/tmp/a.ts"), src).unwrap();
        let defs: Vec<_> = tags
            .iter()
            .filter(|t| t.is_def())
            .map(|t| t.name.as_str())
            .collect();
        assert!(defs.contains(&"Foo"));
        assert!(defs.contains(&"Impl"));
        assert!(defs.contains(&"use"));
    }
}
