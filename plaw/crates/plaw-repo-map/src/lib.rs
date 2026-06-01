//! plaw-repo-map — Aider-style repository map for plaw.
//!
//! Pipeline: walk → parse (tree-sitter) → tag → graph → personalized PageRank →
//! rank-propagate → token-budget binary search → render.
//!
//! Public API: [`RepoMapBuilder`] / [`RepoMap`].
//!
//! Default config matches Aider's empirically-tuned constants (mention 10×,
//! chat-file 50×, private 0.1×, sqrt(num_refs), 100/n personalization unit,
//! ±15% budget slack). Don't retune without an eval suite.
//!
//! Phase 0 (this crate) is **standalone** — no wiring into plaw's prompt
//! pipeline. PR #70 will plug into ws.rs session cache + agent::turn.

mod budget;
mod cache;
mod graph;
mod lang;
mod parser;
mod ranking;
mod renderer;
mod tag;
mod walk;

pub use budget::{approx_token_count, BudgetParams};
pub use cache::TagsCache;
pub use graph::GraphParams;
pub use lang::Lang;
pub use ranking::{rank, RankInput, RankedTag};
pub use renderer::{render, DiskSourceLoader, InMemorySourceLoader, RenderParams, SourceLoader};
pub use tag::{Tag, TagKind};
pub use walk::walk_supported;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// High-level facade. Walks a repo root, extracts tags, ranks them,
/// and renders a token-budgeted map.
pub struct RepoMapBuilder {
    root: PathBuf,
    max_tokens: usize,
    chat_files: HashSet<PathBuf>,
    mentioned_files: HashSet<PathBuf>,
    mentioned_idents: HashSet<String>,
    graph_params: GraphParams,
    budget_params: BudgetParams,
    cache: TagsCache,
}

impl RepoMapBuilder {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_tokens: 1024,
            chat_files: HashSet::new(),
            mentioned_files: HashSet::new(),
            mentioned_idents: HashSet::new(),
            graph_params: GraphParams::default(),
            budget_params: BudgetParams::default(),
            cache: TagsCache::new(),
        }
    }

    pub fn with_max_tokens(mut self, n: usize) -> Self {
        self.max_tokens = n;
        self.budget_params.max_tokens = n;
        self
    }

    pub fn with_chat_files<I: IntoIterator<Item = PathBuf>>(mut self, paths: I) -> Self {
        self.chat_files = paths.into_iter().collect();
        self
    }

    pub fn with_mentioned_idents<I: IntoIterator<Item = String>>(mut self, idents: I) -> Self {
        self.mentioned_idents = idents.into_iter().collect();
        self
    }

    pub fn with_mentioned_files<I: IntoIterator<Item = PathBuf>>(mut self, paths: I) -> Self {
        self.mentioned_files = paths.into_iter().collect();
        self
    }

    pub fn build(&self) -> anyhow::Result<RepoMap> {
        let files = walk_supported(&self.root);
        let mut all_tags: Vec<Tag> = Vec::new();
        let mut all_files: Vec<PathBuf> = Vec::with_capacity(files.len());

        for (abs, rel) in &files {
            all_files.push(rel.clone());
            let tags = self
                .cache
                .get_or_compute(abs, rel, parser::extract_tags_from_file)
                .unwrap_or_default();
            all_tags.extend(tags);
        }

        let input = RankInput {
            all_tags: &all_tags,
            all_files: &all_files,
            chat_files: &self.chat_files,
            mentioned_files: &self.mentioned_files,
            mentioned_idents: &self.mentioned_idents,
        };
        let ranked = rank(input, &self.graph_params);

        let render_params = RenderParams::default();
        let loader = DiskSourceLoader::new(self.root.clone());
        let (text, tokens) = budget::binary_search_budget(&ranked, &self.budget_params, |slice| {
            render(slice, &loader, &render_params)
        });

        Ok(RepoMap {
            text,
            tokens,
            file_count: all_files.len(),
            tag_count: all_tags.len(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct RepoMap {
    pub text: String,
    pub tokens: usize,
    pub file_count: usize,
    pub tag_count: usize,
}

impl RepoMap {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Convenience: one-shot build with defaults + chat files.
pub fn build_for_root(root: &Path, max_tokens: usize) -> anyhow::Result<RepoMap> {
    RepoMapBuilder::new(root)
        .with_max_tokens(max_tokens)
        .build()
}
