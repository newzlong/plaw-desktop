use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
}

impl Lang {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())?
            .to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Lang::Rust,
            "py" | "pyi" => Lang::Python,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            "ts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "go" => Lang::Go,
            _ => return None,
        })
    }

    pub fn tree_sitter_language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    pub fn query_source(self) -> &'static str {
        match self {
            Lang::Rust => include_str!("../queries/rust-tags.scm"),
            Lang::Python => include_str!("../queries/python-tags.scm"),
            Lang::JavaScript => include_str!("../queries/javascript-tags.scm"),
            Lang::TypeScript | Lang::Tsx => include_str!("../queries/typescript-tags.scm"),
            Lang::Go => include_str!("../queries/go-tags.scm"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_extensions() {
        assert_eq!(Lang::from_path(Path::new("foo.rs")), Some(Lang::Rust));
        assert_eq!(Lang::from_path(Path::new("foo.py")), Some(Lang::Python));
        assert_eq!(Lang::from_path(Path::new("foo.ts")), Some(Lang::TypeScript));
        assert_eq!(Lang::from_path(Path::new("foo.tsx")), Some(Lang::Tsx));
        assert_eq!(Lang::from_path(Path::new("foo.go")), Some(Lang::Go));
        assert_eq!(Lang::from_path(Path::new("foo.txt")), None);
    }

    #[test]
    fn case_insensitive_extension() {
        assert_eq!(Lang::from_path(Path::new("foo.RS")), Some(Lang::Rust));
    }
}
