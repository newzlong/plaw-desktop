use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use crate::tag::{Tag, TagKind};

#[derive(Debug, Default)]
pub(crate) struct Graph {
    pub files: Vec<PathBuf>,
    file_idx: HashMap<PathBuf, usize>,
    pub out_edges: Vec<Vec<Edge>>,
}

#[derive(Debug, Clone)]
pub(crate) struct Edge {
    pub dst: usize,
    pub ident: String,
    pub weight: f64,
}

impl Graph {
    fn intern(&mut self, path: PathBuf) -> usize {
        if let Some(&i) = self.file_idx.get(&path) {
            return i;
        }
        let i = self.files.len();
        self.file_idx.insert(path.clone(), i);
        self.files.push(path);
        self.out_edges.push(Vec::new());
        i
    }

    pub fn n(&self) -> usize {
        self.files.len()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GraphParams {
    pub mention_mul: f64,
    pub long_name_mul: f64,
    pub long_name_min_len: usize,
    pub private_mul: f64,
    pub over_defined_mul: f64,
    pub over_defined_threshold: usize,
    pub chat_file_mul: f64,
    pub orphan_self_edge_weight: f64,
}

impl Default for GraphParams {
    fn default() -> Self {
        Self {
            mention_mul: 10.0,
            long_name_mul: 10.0,
            long_name_min_len: 8,
            private_mul: 0.1,
            over_defined_mul: 0.1,
            over_defined_threshold: 5,
            chat_file_mul: 50.0,
            orphan_self_edge_weight: 0.1,
        }
    }
}

pub(crate) struct BuiltGraph {
    pub graph: Graph,
}

pub(crate) fn build_graph(
    tags: &[Tag],
    chat_rel_paths: &HashSet<PathBuf>,
    mentioned_idents: &HashSet<String>,
    params: &GraphParams,
) -> BuiltGraph {
    let mut g = Graph::default();
    let mut defines: HashMap<String, Vec<usize>> = HashMap::new();
    let mut references: HashMap<String, Vec<usize>> = HashMap::new();

    for tag in tags {
        let idx = g.intern(tag.rel_path.clone());
        match tag.kind {
            TagKind::Def => {
                let entry = defines.entry(tag.name.clone()).or_default();
                if !entry.contains(&idx) {
                    entry.push(idx);
                }
            }
            TagKind::Ref => {
                references.entry(tag.name.clone()).or_default().push(idx);
            }
        }
    }

    // Self-edges for orphan defs (defined but never referenced) — workaround for
    // grammars that miss reference patterns. Weight intentionally tiny.
    for (ident, definers) in defines.iter() {
        if references.contains_key(ident) {
            continue;
        }
        for &def in definers {
            g.out_edges[def].push(Edge {
                dst: def,
                ident: ident.clone(),
                weight: params.orphan_self_edge_weight,
            });
        }
    }

    // Edges from referencer → each definer per shared identifier.
    let chat_indices: HashSet<usize> = chat_rel_paths
        .iter()
        .filter_map(|p| g.file_idx.get(p).copied())
        .collect();

    let idents: Vec<&String> = defines
        .keys()
        .filter(|k| references.contains_key(*k))
        .collect();

    for ident in idents {
        let definers = defines.get(ident).expect("defines key");
        let refs = references.get(ident).expect("references key");

        let mut mul = 1.0;
        if mentioned_idents.contains(ident) {
            mul *= params.mention_mul;
        }
        if ident.len() >= params.long_name_min_len && is_compound_name(ident) {
            mul *= params.long_name_mul;
        }
        if ident.starts_with('_') {
            mul *= params.private_mul;
        }
        if definers.len() > params.over_defined_threshold {
            mul *= params.over_defined_mul;
        }

        // Counter(references[ident]) — count refs per referencer file.
        let mut ref_counts: BTreeMap<usize, usize> = BTreeMap::new();
        for &r in refs {
            *ref_counts.entry(r).or_insert(0) += 1;
        }

        for (&referencer, &num_refs) in &ref_counts {
            let mut use_mul = mul;
            if chat_indices.contains(&referencer) {
                use_mul *= params.chat_file_mul;
            }
            let scaled = use_mul * (num_refs as f64).sqrt();
            for &definer in definers {
                g.out_edges[referencer].push(Edge {
                    dst: definer,
                    ident: ident.clone(),
                    weight: scaled,
                });
            }
        }
    }

    BuiltGraph { graph: g }
}

fn is_compound_name(s: &str) -> bool {
    let has_alpha = s.chars().any(|c| c.is_alphabetic());
    if !has_alpha {
        return false;
    }
    let is_snake = s.contains('_') && has_alpha;
    let is_kebab = s.contains('-') && has_alpha;
    let is_camel = s.chars().any(|c| c.is_uppercase()) && s.chars().any(|c| c.is_lowercase());
    is_snake || is_kebab || is_camel
}

/// Weighted personalized PageRank.
///
/// `n` nodes; `out` is `out[src] = [(dst, weight)]`.
/// `personalization[i]` must be a non-negative distribution; it is renormalized
/// internally. Returns rank vector summing to 1.0.
pub(crate) fn weighted_pagerank(
    n: usize,
    out: &[Vec<Edge>],
    personalization: &[f64],
    damping: f64,
    max_iter: usize,
    tol: f64,
) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    debug_assert_eq!(out.len(), n);
    debug_assert_eq!(personalization.len(), n);

    let pers_sum: f64 = personalization.iter().sum();
    let pers: Vec<f64> = if pers_sum > 0.0 {
        personalization.iter().map(|p| p / pers_sum).collect()
    } else {
        vec![1.0 / n as f64; n]
    };

    let out_weight: Vec<f64> = out
        .iter()
        .map(|edges| edges.iter().map(|e| e.weight).sum::<f64>())
        .collect();

    let mut rank = pers.clone();
    let mut next = vec![0.0; n];

    for _ in 0..max_iter {
        next.iter_mut().for_each(|v| *v = 0.0);
        let mut dangling = 0.0;
        for i in 0..n {
            if out_weight[i] == 0.0 {
                dangling += rank[i];
            }
        }

        for src in 0..n {
            if out_weight[src] == 0.0 {
                continue;
            }
            let factor = rank[src] / out_weight[src];
            for edge in &out[src] {
                next[edge.dst] += damping * factor * edge.weight;
            }
        }
        for i in 0..n {
            next[i] += damping * dangling * pers[i];
            next[i] += (1.0 - damping) * pers[i];
        }

        let delta: f64 = (0..n).map(|i| (next[i] - rank[i]).abs()).sum();
        std::mem::swap(&mut rank, &mut next);
        if delta < tol * n as f64 {
            break;
        }
    }

    rank
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagerank_sums_to_one() {
        // 3 nodes: 0→1 (w=2), 1→2 (w=1), 2→0 (w=1)
        let out = vec![
            vec![Edge {
                dst: 1,
                ident: "x".into(),
                weight: 2.0,
            }],
            vec![Edge {
                dst: 2,
                ident: "y".into(),
                weight: 1.0,
            }],
            vec![Edge {
                dst: 0,
                ident: "z".into(),
                weight: 1.0,
            }],
        ];
        let pers = vec![1.0 / 3.0; 3];
        let r = weighted_pagerank(3, &out, &pers, 0.85, 100, 1e-6);
        let total: f64 = r.iter().sum();
        assert!((total - 1.0).abs() < 1e-3, "total={}", total);
    }

    #[test]
    fn pagerank_dangling_distributes() {
        // 0→1, 1 dangling. With personalization pinned on node 2 (sink replacement),
        // rank should drain to node 2.
        let out = vec![
            vec![Edge {
                dst: 1,
                ident: "x".into(),
                weight: 1.0,
            }],
            vec![],
            vec![],
        ];
        let pers = vec![0.0, 0.0, 1.0];
        let r = weighted_pagerank(3, &out, &pers, 0.85, 200, 1e-9);
        assert!(r[2] > r[0]);
        assert!(r[2] > r[1]);
    }

    #[test]
    fn is_compound_name_buckets() {
        assert!(is_compound_name("snake_case_name"));
        assert!(is_compound_name("CamelCase"));
        assert!(is_compound_name("kebab-case"));
        assert!(!is_compound_name("short"));
        assert!(!is_compound_name("____"));
        assert!(!is_compound_name(""));
    }

    #[test]
    fn build_graph_creates_edge_for_shared_ident() {
        use crate::tag::TagKind;
        let tags = vec![
            Tag {
                rel_path: PathBuf::from("a.rs"),
                abs_path: PathBuf::from("/a.rs"),
                line: 0,
                name: "Foo".into(),
                kind: TagKind::Def,
            },
            Tag {
                rel_path: PathBuf::from("b.rs"),
                abs_path: PathBuf::from("/b.rs"),
                line: 0,
                name: "Foo".into(),
                kind: TagKind::Ref,
            },
        ];
        let bg = build_graph(
            &tags,
            &HashSet::new(),
            &HashSet::new(),
            &GraphParams::default(),
        );
        assert_eq!(bg.graph.n(), 2);
        // b → a edge for "Foo"
        let b_idx = bg.graph.file_idx[&PathBuf::from("b.rs")];
        let a_idx = bg.graph.file_idx[&PathBuf::from("a.rs")];
        assert!(bg.graph.out_edges[b_idx]
            .iter()
            .any(|e| e.dst == a_idx && e.ident == "Foo"));
    }
}
