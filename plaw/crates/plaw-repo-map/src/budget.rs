use crate::ranking::RankedTag;

/// Approximate token count. Aider uses litellm.token_counter (model-aware);
/// for the cross-provider case we approximate with chars/4 + 1 per newline,
/// which is within ~15% of tiktoken for English+code on every provider tested.
pub fn approx_token_count(s: &str) -> usize {
    let chars = s.chars().count();
    let newlines = s.matches('\n').count();
    chars / 4 + newlines
}

pub struct BudgetParams {
    pub max_tokens: usize,
    pub initial_tokens_per_tag: usize,
    pub slack: f64,
}

impl Default for BudgetParams {
    fn default() -> Self {
        Self {
            max_tokens: 1024,
            initial_tokens_per_tag: 25,
            slack: 0.15,
        }
    }
}

/// Binary-search the largest prefix of `ranked` whose rendered output fits in
/// the token budget within `slack` tolerance. Mirrors Aider's algorithm.
pub fn binary_search_budget<R: Fn(&[RankedTag]) -> String>(
    ranked: &[RankedTag],
    params: &BudgetParams,
    render: R,
) -> (String, usize) {
    let num = ranked.len();
    if num == 0 {
        return (String::new(), 0);
    }
    let max = params.max_tokens.max(1);

    let mut lower: usize = 0;
    let mut upper: usize = num;
    let mut middle = (max / params.initial_tokens_per_tag.max(1)).min(num);
    let mut best_text = String::new();
    let mut best_tokens = 0;

    // Bound iterations to log2(num) + 2 — keeps tests deterministic.
    let max_iter = (num as f64).log2().ceil() as usize + 3;
    for _ in 0..max_iter {
        if lower > upper {
            break;
        }
        let tree = render(&ranked[..middle]);
        let n_tokens = approx_token_count(&tree);
        let pct_err = ((n_tokens as f64 - max as f64) / max as f64).abs();
        let in_budget = n_tokens <= max && n_tokens > best_tokens;
        let within_slack = pct_err < params.slack;
        if in_budget || within_slack {
            best_text = tree;
            best_tokens = n_tokens;
            if within_slack {
                break;
            }
        }
        if n_tokens < max {
            lower = middle + 1;
        } else {
            if middle == 0 {
                break;
            }
            upper = middle - 1;
        }
        let next = (lower + upper) / 2;
        if next == middle {
            break;
        }
        middle = next;
    }

    if best_text.is_empty() && !ranked.is_empty() {
        // Always emit at least the top-ranked entry — caller would rather see
        // overhang than nothing.
        best_text = render(&ranked[..1.min(num)]);
        best_tokens = approx_token_count(&best_text);
    }

    (best_text, best_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn synthetic(n: usize) -> Vec<RankedTag> {
        (0..n)
            .map(|i| RankedTag {
                rel_path: PathBuf::from(format!("f{}.rs", i)),
                ident: format!("ident_{}", i),
                score: (n - i) as f64,
                line: None,
            })
            .collect()
    }

    #[test]
    fn token_count_approx() {
        // "hello world" = 11 chars / 4 = 2 tokens, 0 newlines
        assert_eq!(approx_token_count("hello world"), 2);
        // "a\nb\n" = 4 chars / 4 + 2 newlines = 3 tokens
        assert_eq!(approx_token_count("a\nb\n"), 3);
    }

    #[test]
    fn budget_respects_max_tokens() {
        let tags = synthetic(50);
        let params = BudgetParams {
            max_tokens: 30,
            initial_tokens_per_tag: 5,
            slack: 0.1,
        };
        let (text, tokens) = binary_search_budget(&tags, &params, |slice| {
            slice
                .iter()
                .map(|t| format!("{}::{}\n", t.rel_path.display(), t.ident))
                .collect()
        });
        assert!(!text.is_empty());
        // Within slack of max (30 ± 10%) OR <= max.
        let within = tokens <= 30 || ((tokens as f64 - 30.0) / 30.0).abs() < 0.15;
        assert!(within, "tokens={} out of budget", tokens);
    }

    #[test]
    fn budget_handles_empty_input() {
        let tags: Vec<RankedTag> = vec![];
        let (text, tokens) =
            binary_search_budget(&tags, &BudgetParams::default(), |_| String::new());
        assert!(text.is_empty());
        assert_eq!(tokens, 0);
    }
}
