//! Build the prompt-mode tool-instruction block for the system prompt.
//!
//! When the agent runs against a provider that does not (or cannot)
//! emit native `tool_calls` JSON, plaw falls back to *prompt-mode*
//! tool calling: the model emits `<tool_call>...</tool_call>` XML
//! payloads inside its response text, and `loop_::parsing` parses
//! them back out. For that to work the model has to *know* the XML
//! shape and the available tool names — that knowledge is injected
//! via the system prompt block this module produces.
//!
//! Contract with consumers:
//!
//!   - The block always starts with `\n## Tool Use Protocol\n\n` so
//!     `SystemPromptBuilder` (and the channel system-prompt assemblers
//!     in `channels/mod.rs`) can `push_str` it directly without
//!     worrying about leading whitespace.
//!   - Each tool entry uses the `**name**: description\nParameters:
//!     `<schema>`` shape — plain Markdown, no escaping, since tool
//!     descriptions are author-controlled and trusted.
//!   - The wording is intentionally *stricter* than the lower-level
//!     `providers::traits::build_tool_instructions_text` (which serves
//!     prompt-mode fallback for providers that drop native tool calls
//!     mid-stream): this version has the explicit "CRITICAL: Output
//!     actual `<tool_call>` tags — never describe steps or give
//!     examples" line that survived multiple iterations of fighting
//!     models that would emit tool *examples* instead of *real*
//!     tool calls. Do not refactor the two into a single function
//!     without re-running the prompt-mode regression suite.

use std::fmt::Write;

use crate::tools::Tool;

/// Convenience wrapper: build instructions from a live tool registry
/// by collecting their `spec()`s and forwarding to
/// [`build_tool_instructions_from_specs`]. Used by the agent loop's
/// system-prompt assembler when the registry is in scope.
pub(crate) fn build_tool_instructions(tools_registry: &[Box<dyn Tool>]) -> String {
    let specs: Vec<crate::tools::ToolSpec> =
        tools_registry.iter().map(|tool| tool.spec()).collect();
    build_tool_instructions_from_specs(&specs)
}

/// Build the tool instruction block for the system prompt from
/// concrete tool specs. Used directly when the caller already has a
/// `Vec<ToolSpec>` (e.g. channel-side system-prompt assembly that
/// filters specs by autonomy / per-channel allowlist).
pub(crate) fn build_tool_instructions_from_specs(
    tool_specs: &[crate::tools::ToolSpec],
) -> String {
    let mut instructions = String::new();
    instructions.push_str("\n## Tool Use Protocol\n\n");
    instructions
        .push_str("To use a tool, wrap a JSON object in <tool_call></tool_call> tags:\n\n");
    instructions.push_str(
        "```\n<tool_call>\n{\"name\": \"tool_name\", \"arguments\": {\"param\": \"value\"}}\n</tool_call>\n```\n\n",
    );
    instructions.push_str(
        "CRITICAL: Output actual <tool_call> tags—never describe steps or give examples.\n\n",
    );
    instructions.push_str(
        "When a tool is needed, emit a real call (not prose), for example:\n\
<tool_call>\n\
{\"name\":\"tool_name\",\"arguments\":{}}\n\
</tool_call>\n\n",
    );
    instructions.push_str("You may use multiple tool calls in a single response. ");
    instructions.push_str("After tool execution, results appear in <tool_result> tags. ");
    instructions
        .push_str("Continue reasoning with the results until you can give a final answer.\n\n");
    instructions.push_str("### Available Tools\n\n");

    for tool in tool_specs {
        let _ = writeln!(
            instructions,
            "**{}**: {}\nParameters: `{}`\n",
            tool.name, tool.description, tool.parameters
        );
    }

    instructions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolSpec;

    fn spec(name: &str, description: &str, parameters: serde_json::Value) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    // ── Header / structural invariants ───────────────────────────

    #[test]
    fn block_starts_with_protocol_header_and_leading_newline() {
        // Channel assemblers `push_str` this block directly onto an
        // already-built prompt; the leading "\n" is part of the
        // contract so they don't need their own separator. Pinning
        // both the leading newline and the header text catches
        // accidental edits that would shift section spacing.
        let out = build_tool_instructions_from_specs(&[]);
        assert!(out.starts_with("\n## Tool Use Protocol\n\n"));
    }

    #[test]
    fn block_includes_critical_anti_example_directive() {
        // The "CRITICAL: Output actual <tool_call> tags—never describe
        // steps or give examples" line is the contract that fights
        // models that would emit tool *examples* instead of real
        // calls. Stripping or softening it would silently break
        // prompt-mode reliability against weaker models. Pin the
        // exact wording.
        let out = build_tool_instructions_from_specs(&[]);
        assert!(
            out.contains("CRITICAL: Output actual <tool_call> tags—never describe steps or give examples."),
            "anti-example directive must be present verbatim, got: {out}"
        );
    }

    #[test]
    fn block_includes_concrete_call_example_after_critical_line() {
        // The block emits a concrete `<tool_call>...</tool_call>`
        // example AFTER the CRITICAL line so the model sees the
        // real shape it should produce. Reordering would put the
        // example before the prohibition and weaken the message.
        let out = build_tool_instructions_from_specs(&[]);
        let critical_pos = out.find("CRITICAL:").expect("CRITICAL line must exist");
        let example_pos = out
            .find("<tool_call>\n{\"name\":\"tool_name\"")
            .expect("concrete example must exist");
        assert!(
            example_pos > critical_pos,
            "concrete example must follow the CRITICAL line, got example at {example_pos}, CRITICAL at {critical_pos}"
        );
    }

    // ── Tool listing ─────────────────────────────────────────────

    #[test]
    fn empty_specs_still_emits_protocol_header_with_empty_tool_list() {
        // Even with no tools the protocol header + Available Tools
        // header are emitted (so the model gets the shape contract
        // it needs to issue future calls if tools become available
        // mid-conversation). Defensive shape check.
        let out = build_tool_instructions_from_specs(&[]);
        assert!(out.contains("### Available Tools"));
        // No tool entry means no `**name**:` line.
        assert!(!out.contains("**"));
    }

    #[test]
    fn single_spec_emits_name_description_parameters_in_canonical_shape() {
        let specs = vec![spec(
            "shell",
            "Run a shell command",
            serde_json::json!({"type": "object"}),
        )];
        let out = build_tool_instructions_from_specs(&specs);
        // Each tool: bold name + colon + description on one line,
        // backticked parameters JSON on the next. The exact
        // formatter shape is part of the prompt-mode contract — the
        // parser at parsing.rs assumes nothing about it, but the
        // model's prompt expects this layout.
        assert!(
            out.contains("**shell**: Run a shell command\nParameters: `{\"type\":\"object\"}`"),
            "canonical shape missing, got: {out}"
        );
    }

    #[test]
    fn multiple_specs_appear_in_declaration_order() {
        // The `### Available Tools` listing iterates the slice in
        // order; if a downstream filter sorts the slice, the model
        // sees that order. Pin the iteration-faithful behavior here
        // so a future "stable-sort by name" change has to be a
        // conscious decision.
        let specs = vec![
            spec("zeta", "z desc", serde_json::json!({})),
            spec("alpha", "a desc", serde_json::json!({})),
            spec("middle", "m desc", serde_json::json!({})),
        ];
        let out = build_tool_instructions_from_specs(&specs);
        let zeta = out.find("**zeta**").expect("zeta missing");
        let alpha = out.find("**alpha**").expect("alpha missing");
        let middle = out.find("**middle**").expect("middle missing");
        assert!(
            zeta < alpha && alpha < middle,
            "tools must appear in declaration order: zeta@{zeta} < alpha@{alpha} < middle@{middle}"
        );
    }

    // ── Registry-level wrapper ───────────────────────────────────

    #[test]
    fn registry_wrapper_round_trips_through_specs() {
        // build_tool_instructions(registry) == build_tool_instructions_
        // from_specs(registry.iter().map(spec)). Prove the two
        // entry points produce byte-identical output so a caller can
        // freely choose either based on what they have in scope.
        use crate::security::SecurityPolicy;
        let security = std::sync::Arc::new(SecurityPolicy::from_config(
            &crate::config::AutonomyConfig::default(),
            std::path::Path::new("/tmp"),
        ));
        let tools = crate::tools::default_tools(security);
        let specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();
        assert_eq!(
            build_tool_instructions(&tools),
            build_tool_instructions_from_specs(&specs)
        );
    }

    #[test]
    fn registry_wrapper_includes_default_tool_names() {
        // Smoke test against the actual default tool registry —
        // catches a future PR that drops a tool without realising
        // the prompt block stops listing it.
        use crate::security::SecurityPolicy;
        let security = std::sync::Arc::new(SecurityPolicy::from_config(
            &crate::config::AutonomyConfig::default(),
            std::path::Path::new("/tmp"),
        ));
        let tools = crate::tools::default_tools(security);
        let out = build_tool_instructions(&tools);
        for required in ["shell", "file_read", "file_write"] {
            assert!(
                out.contains(required),
                "{required} must appear in default tool listing, got: {out}"
            );
        }
    }
}
