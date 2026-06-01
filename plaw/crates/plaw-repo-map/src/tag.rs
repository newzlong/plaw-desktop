use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TagKind {
    Def,
    Ref,
}

#[derive(Debug, Clone)]
pub struct Tag {
    pub rel_path: PathBuf,
    pub abs_path: PathBuf,
    pub line: usize,
    pub name: String,
    pub kind: TagKind,
}

impl Tag {
    pub fn is_def(&self) -> bool {
        matches!(self.kind, TagKind::Def)
    }

    pub fn is_ref(&self) -> bool {
        matches!(self.kind, TagKind::Ref)
    }
}
