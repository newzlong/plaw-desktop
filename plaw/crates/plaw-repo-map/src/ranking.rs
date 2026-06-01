use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::graph::{build_graph, weighted_pagerank, GraphParams};
use crate::tag::Tag;

#[derive(Debug, Clone)]
pub struct RankedTag {
    pub rel_path: PathBuf,
    pub ident: String,
    pub score: f64,
    /// Optional source line (0-indexed) of the definition.
    /// Caller-attached via [`RankedTag::with_line`]; renderer falls back to 0.
    pub line: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct RankInput<'a> {
    pub all_tags: &'a [Tag],
    pub all_files: &'a [PathBuf],
    pub chat_files: &'a HashSet<PathBuf>,
    pub mentioned_files: &'a HashSet<PathBuf>,
    pub mentioned_idents: &'a HashSet<String>,
}

pub fn rank(input: RankInput<'_>, params: &GraphParams) -> Vec<RankedTag> {
    let bg = build_graph(
        input.all_tags,
        input.chat_files,
        input.mentioned_idents,
        params,
    );

    let n = bg.graph.n();
    if n == 0 {
        return Vec::new();
    }

    // Personalization vector — Aider's "personalize = 100 / len(fnames)" unit,
    // but we just renormalize after, so the constant doesn't matter.
    let unit = 1.0;
    let mut pers = vec![0.0_f64; n];
    let total_files = input.all_files.len().max(1) as f64;

    for (i, p) in bg.graph.files.iter().enumerate() {
        if input.chat_files.contains(p) {
            pers[i] += unit;
        }
        if input.mentioned_files.contains(p) {
            // Cap (per Aider: max not add) — but here we just ensure ≥ unit.
            if pers[i] < unit {
                pers[i] = unit;
            }
        }
        // Path-component match against mentioned_idents (coarse heuristic).
        if path_matches_any_ident(p, input.mentioned_idents) {
            pers[i] += unit;
        }
    }

    // If nothing personalized, fall back to uniform.
    let pers_sum: f64 = pers.iter().sum();
    if pers_sum == 0.0 {
        pers = vec![1.0 / total_files; n];
    }

    let ranks = weighted_pagerank(n, &bg.graph.out_edges, &pers, 0.85, 100, 1e-6);

    // Rank propagation: distribute each node's rank across its outgoing edges
    // weighted by edge.weight / sum_of_out_weights. Sum per (dst, ident).
    let mut ranked: HashMap<(usize, String), f64> = HashMap::new();
    for (src, edges) in bg.graph.out_edges.iter().enumerate() {
        let total_w: f64 = edges.iter().map(|e| e.weight).sum();
        if total_w == 0.0 {
            continue;
        }
        let src_rank = ranks[src];
        for edge in edges {
            let contribution = src_rank * edge.weight / total_w;
            *ranked.entry((edge.dst, edge.ident.clone())).or_insert(0.0) += contribution;
        }
    }

    // Attach lines: pick the FIRST definition line for each (file, ident) pair.
    let mut line_of: HashMap<(PathBuf, String), usize> = HashMap::new();
    for tag in input.all_tags {
        if !tag.is_def() {
            continue;
        }
        line_of
            .entry((tag.rel_path.clone(), tag.name.clone()))
            .or_insert(tag.line);
    }

    let mut out: Vec<RankedTag> = ranked
        .into_iter()
        .filter(|((dst, _), _)| {
            // Skip tags whose file is in the chat (model already sees that file).
            !input.chat_files.contains(&bg.graph.files[*dst])
        })
        .map(|((dst, ident), score)| {
            let rel_path = bg.graph.files[dst].clone();
            let line = line_of.get(&(rel_path.clone(), ident.clone())).copied();
            RankedTag {
                rel_path,
                ident,
                score,
                line,
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rel_path.cmp(&b.rel_path))
            .then_with(|| a.ident.cmp(&b.ident))
    });
    out
}

fn path_matches_any_ident(path: &Path, mentioned_idents: &HashSet<String>) -> bool {
    if mentioned_idents.is_empty() {
        return false;
    }
    for component in path.components() {
        let Some(s) = component.as_os_str().to_str() else {
            continue;
        };
        if mentioned_idents.contains(s) {
            return true;
        }
        // Also check basename without extension.
        if let Some(stem) = Path::new(s).file_stem().and_then(|x| x.to_str()) {
            if mentioned_idents.contains(stem) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tag::TagKind;

    fn def(rel: &str, name: &str) -> Tag {
        Tag {
            rel_path: PathBuf::from(rel),
            abs_path: PathBuf::from("/abs/").join(rel),
            line: 0,
            name: name.into(),
            kind: TagKind::Def,
        }
    }
    fn refer(rel: &str, name: &str) -> Tag {
        Tag {
            rel_path: PathBuf::from(rel),
            abs_path: PathBuf::from("/abs/").join(rel),
            line: 0,
            name: name.into(),
            kind: TagKind::Ref,
        }
    }

    #[test]
    fn defs_referenced_from_chat_file_rank_higher() {
        let tags = vec![
            def("api.rs", "Hub"),
            def("util.rs", "helper"),
            refer("chat.rs", "Hub"),
        ];
        let files: Vec<PathBuf> = vec!["api.rs".into(), "util.rs".into(), "chat.rs".into()];
        let mut chat = HashSet::new();
        chat.insert(PathBuf::from("chat.rs"));

        let input = RankInput {
            all_tags: &tags,
            all_files: &files,
            chat_files: &chat,
            mentioned_files: &HashSet::new(),
            mentioned_idents: &HashSet::new(),
        };
        let ranked = rank(input, &GraphParams::default());

        // Chat file's defs filtered out; api.rs::Hub should be present.
        assert!(ranked
            .iter()
            .any(|r| r.rel_path == Path::new("api.rs") && r.ident == "Hub"));
        assert!(!ranked.iter().any(|r| r.rel_path == Path::new("chat.rs")));
    }

    #[test]
    fn empty_input_returns_empty() {
        let files: Vec<PathBuf> = vec![];
        let input = RankInput {
            all_tags: &[],
            all_files: &files,
            chat_files: &HashSet::new(),
            mentioned_files: &HashSet::new(),
            mentioned_idents: &HashSet::new(),
        };
        let ranked = rank(input, &GraphParams::default());
        assert!(ranked.is_empty());
    }

    #[test]
    fn private_names_demoted() {
        // Both _hidden and Public defined in api.rs and called from user.rs.
        // Public must rank higher than _hidden because of the 0.1× private mul.
        let tags = vec![
            def("api.rs", "_hidden"),
            def("api.rs", "Public"),
            refer("user.rs", "_hidden"),
            refer("user.rs", "Public"),
        ];
        let files: Vec<PathBuf> = vec!["api.rs".into(), "user.rs".into()];
        let input = RankInput {
            all_tags: &tags,
            all_files: &files,
            chat_files: &HashSet::new(),
            mentioned_files: &HashSet::new(),
            mentioned_idents: &HashSet::new(),
        };
        let ranked = rank(input, &GraphParams::default());
        let public_score = ranked
            .iter()
            .find(|r| r.ident == "Public")
            .map(|r| r.score)
            .unwrap_or(0.0);
        let hidden_score = ranked
            .iter()
            .find(|r| r.ident == "_hidden")
            .map(|r| r.score)
            .unwrap_or(0.0);
        assert!(
            public_score > hidden_score,
            "Public {} should rank above _hidden {}",
            public_score,
            hidden_score
        );
    }
}
