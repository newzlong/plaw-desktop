use plaw_repo_map::RepoMapBuilder;
use std::fs;
use tempfile::TempDir;

fn write(dir: &TempDir, rel: &str, content: &str) {
    let p = dir.path().join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, content).unwrap();
}

#[test]
fn end_to_end_rust_repo_produces_map() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "src/lib.rs",
        r#"
pub struct Engine {
    inner: u32,
}

impl Engine {
    pub fn new() -> Self { Self { inner: 0 } }
    pub fn tick(&mut self) { self.inner += 1; }
}

pub fn run_engine() {
    let mut e = Engine::new();
    e.tick();
}
"#,
    );
    write(
        &dir,
        "src/user.rs",
        r#"
use crate::Engine;

pub fn use_it() {
    let mut e = Engine::new();
    e.tick();
    e.tick();
}
"#,
    );
    write(
        &dir,
        "src/main.rs",
        r#"
fn main() {
    plaw_repo_map_sample::run_engine();
}
"#,
    );
    write(&dir, "README.md", "# sample repo\n");

    let map = RepoMapBuilder::new(dir.path())
        .with_max_tokens(2048)
        .build()
        .unwrap();

    assert!(map.file_count >= 3, "files={}", map.file_count);
    assert!(map.tag_count > 0, "no tags extracted");
    assert!(!map.text.is_empty(), "map text is empty");
    assert!(
        map.text.contains("Engine") || map.text.contains("tick") || map.text.contains("run_engine"),
        "expected some symbol in rendered text: {}",
        map.text
    );
    // tokens recorded
    assert!(map.tokens > 0);
}

#[test]
fn mixed_language_repo_handles_all() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "a.rs",
        "pub struct Foo {}\npub fn make_foo() -> Foo { Foo {} }\n",
    );
    write(
        &dir,
        "b.py",
        "class Bar:\n    def hi(self): pass\n\ndef call():\n    Bar().hi()\n",
    );
    write(
        &dir,
        "c.go",
        "package x\ntype Item struct{}\nfunc New() *Item { return &Item{} }\n",
    );
    write(
        &dir,
        "d.js",
        "function helper(){}\nclass Widget{}\nnew Widget(); helper();\n",
    );
    write(
        &dir,
        "e.ts",
        "export interface Conf { x: number }\nexport class Impl implements Conf { x = 1 }\n",
    );

    let map = RepoMapBuilder::new(dir.path())
        .with_max_tokens(4096)
        .build()
        .unwrap();

    assert!(map.file_count >= 5);
    assert!(map.tag_count > 0);
}

#[test]
fn chat_file_demotes_its_own_tags() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "api.rs",
        "pub struct Hub {}\npub fn build() -> Hub { Hub {} }\n",
    );
    write(&dir, "chat.rs", "use crate::Hub;\nfn used() { Hub {}; }\n");

    let map_with_chat = RepoMapBuilder::new(dir.path())
        .with_max_tokens(2048)
        .with_chat_files([std::path::PathBuf::from("chat.rs")])
        .build()
        .unwrap();

    // chat.rs symbols filtered out; api.rs symbols still surface.
    assert!(!map_with_chat.text.contains("chat.rs:"));
    // Hub or build should appear since chat.rs references them.
    assert!(map_with_chat.text.contains("Hub") || map_with_chat.text.contains("build"));
}

#[test]
fn respects_dot_ignore() {
    let dir = TempDir::new().unwrap();
    write(&dir, ".ignore", "skip.rs\n");
    write(&dir, "keep.rs", "pub fn k() {}\n");
    write(&dir, "skip.rs", "pub fn s() {}\n");

    let map = RepoMapBuilder::new(dir.path())
        .with_max_tokens(1024)
        .build()
        .unwrap();

    assert_eq!(map.file_count, 1);
    assert!(map.text.contains("keep.rs"));
    assert!(!map.text.contains("skip.rs"));
}

#[test]
fn empty_repo_returns_empty_map() {
    let dir = TempDir::new().unwrap();
    let map = RepoMapBuilder::new(dir.path())
        .with_max_tokens(1024)
        .build()
        .unwrap();
    assert_eq!(map.file_count, 0);
    assert_eq!(map.tag_count, 0);
    assert!(map.text.is_empty());
}
