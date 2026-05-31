//! Tool-name classifications used by the agent loop's anti-loop guards
//! and external-content security tagging.
//!
//! Extracted from `loop_.rs` so the "which tools count as exempt /
//! tight-loop / external-content" decisions live in one auditable file.
//! Add a tool name in exactly one place when the classification changes.

/// Maximum times the same tool can be called in a single turn before
/// anti-loop protection kicks in. Prevents adversarial web content from
/// inducing the AI into fetch/search/browser loops.
pub(super) const MAX_SAME_TOOL_PER_TURN: usize = 6;

/// Tighter per-turn cap for tools where adversarial / non-existent
/// queries (fake citations, hallucinated papers) cause the agent to retry
/// many rephrasings before giving up. 3 is enough to try a few variations.
pub(super) const TIGHT_LOOP_TOOLS: &[(&str, usize)] = &[
    ("web_search_tool", 3),
];

/// Tools exempt from per-turn frequency limits.
/// Browser automation naturally requires many sequential calls
/// (open → snapshot → click → wait → ...). File operations
/// (reading/writing multiple files) are also normal in complex tasks.
/// Shell and delegate tools are also exempt since complex tasks
/// (e.g. PPT generation) need many calls.
pub(super) const ANTI_LOOP_EXEMPT_TOOLS: &[&str] = &[
    // File operations — multi-file generation (e.g. 10 HTML slides → PPT)
    "file_read",
    "file_write",
    "file_edit",
    "glob_search",
    "content_search",
    // Shell — complex tasks chain many commands
    "shell",
    // Browser — navigation requires many sequential calls
    "browser",
    "browser_open",
    "screenshot",
    // Delegation — parallel/sub-agent orchestration
    "delegate",
    "parallel_delegate",
    // Web — research tasks may fetch many pages
    "web_fetch",
    "http_request",
];

/// Tools that return external (untrusted) content which may contain
/// prompt injection. Output from any of these passes through the
/// PromptGuard scanner before being fed back into the LLM.
const EXTERNAL_CONTENT_TOOLS: &[&str] = &[
    "web_fetch",
    "web_search_tool",
    "browser",
    "http_request",
    "content_search", // search results from user files could also be adversarial
    "pdf_read",       // PDFs can ship from arbitrary URLs / email / chat — untrusted by default
    "mcp_call",       // MCP servers (PR #63) are by-definition external processes — every response is untrusted
];

/// Check if a tool produces external/untrusted content. Match is by
/// prefix so versioned variants (e.g. `web_fetch_v2`) inherit the
/// classification of their canonical form without an extra entry here.
pub(super) fn is_external_content_tool(name: &str) -> bool {
    EXTERNAL_CONTENT_TOOLS
        .iter()
        .any(|t| name.starts_with(t) || name == *t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_content_classifier_matches_exact_names() {
        assert!(is_external_content_tool("web_fetch"));
        assert!(is_external_content_tool("web_search_tool"));
        assert!(is_external_content_tool("browser"));
        assert!(is_external_content_tool("http_request"));
        assert!(is_external_content_tool("content_search"));
    }

    #[test]
    fn external_content_classifier_matches_versioned_prefix() {
        // Prefix-match means versioned variants inherit the classification
        // without needing a new EXTERNAL_CONTENT_TOOLS entry.
        assert!(is_external_content_tool("web_fetch_v2"));
        assert!(is_external_content_tool("browser_open"));
    }

    #[test]
    fn external_content_classifier_rejects_internal_tools() {
        // Tools that return only locally-sourced content must NOT be
        // tagged as external — otherwise plaw would prefix every
        // file_read with the "untrusted content" warning, which
        // (a) burns tokens and (b) trains the model to ignore the
        // warning when it actually matters.
        assert!(!is_external_content_tool("file_read"));
        assert!(!is_external_content_tool("file_write"));
        assert!(!is_external_content_tool("memory_recall"));
        assert!(!is_external_content_tool("delegate"));
        assert!(!is_external_content_tool("shell"));
    }

    #[test]
    fn anti_loop_exempt_list_includes_critical_orchestration_tools() {
        // These tools legitimately get called many times per turn during
        // complex tasks (multi-file PPT generation, browser automation
        // chains, parallel delegation). Removing any of them would cause
        // anti-loop to kill the turn early on a normal long task.
        for must_be_exempt in [
            "file_read",
            "file_write",
            "file_edit",
            "shell",
            "browser",
            "delegate",
            "parallel_delegate",
        ] {
            assert!(
                ANTI_LOOP_EXEMPT_TOOLS.contains(&must_be_exempt),
                "{must_be_exempt} should be anti-loop-exempt"
            );
        }
    }

    #[test]
    fn tight_loop_caps_are_strictly_below_default() {
        // Tight-loop limit must be less than the default per-tool limit;
        // otherwise the "tight" class is a no-op.
        for (_, lim) in TIGHT_LOOP_TOOLS {
            assert!(
                *lim < MAX_SAME_TOOL_PER_TURN,
                "tight-loop limit {lim} should be < {MAX_SAME_TOOL_PER_TURN}"
            );
        }
    }
}
