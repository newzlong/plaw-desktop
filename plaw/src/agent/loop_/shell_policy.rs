//! Render the runtime shell-execution policy as a system-prompt block.
//!
//! Extracted from `loop_.rs` so the wording the model sees about
//! autonomy / allowlists / approval gating lives in one auditable file.
//! Edits here change what every plaw shell-tool turn knows about its
//! own constraints.

use std::collections::BTreeSet;
use std::fmt::Write;

use crate::config::AutonomyConfig;
use crate::security::AutonomyLevel;

/// Cap on individual command names listed in the prompt before
/// summarising the rest as "+N more". Keeps prompt token usage
/// bounded when an operator pastes a large allowlist.
const MAX_DISPLAY_COMMANDS: usize = 64;

/// Build shell-policy instructions for the system prompt so the model
/// is aware of command-level execution constraints before it emits
/// tool calls.
///
/// Output is a Markdown section starting with `## Shell Policy`. The
/// shape is contract — channel layers and the `build_system_prompt`
/// pipeline both insert this verbatim.
pub(crate) fn build_shell_policy_instructions(autonomy: &AutonomyConfig) -> String {
    let mut instructions = String::new();
    instructions.push_str("\n## Shell Policy\n\n");
    instructions
        .push_str("When using the `shell` tool, follow these runtime constraints exactly.\n\n");

    let autonomy_label = match autonomy.level {
        AutonomyLevel::ReadOnly => "read_only",
        AutonomyLevel::Supervised => "supervised",
        AutonomyLevel::Full => "full",
    };
    let _ = writeln!(instructions, "- Autonomy level: `{autonomy_label}`");

    if autonomy.level == AutonomyLevel::ReadOnly {
        instructions.push_str(
            "- Shell execution is disabled in `read_only` mode. Do not emit shell tool calls.\n",
        );
        return instructions;
    }

    let normalized: BTreeSet<String> = autonomy
        .allowed_commands
        .iter()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if normalized.contains("*") {
        instructions.push_str(
            "- Allowed commands: wildcard `*` is configured (any command name/path may be allowlisted).\n",
        );
    } else if normalized.is_empty() {
        instructions
            .push_str("- Allowed commands: none configured. Any shell command will be rejected.\n");
    } else {
        let shown: Vec<String> = normalized
            .iter()
            .take(MAX_DISPLAY_COMMANDS)
            .map(|cmd| format!("`{cmd}`"))
            .collect();
        let hidden = normalized.len().saturating_sub(MAX_DISPLAY_COMMANDS);
        let _ = write!(instructions, "- Allowed commands: {}", shown.join(", "));
        if hidden > 0 {
            let _ = write!(instructions, " (+{hidden} more)");
        }
        instructions.push('\n');
    }

    if autonomy.level == AutonomyLevel::Supervised && autonomy.require_approval_for_medium_risk {
        instructions.push_str(
            "- Medium-risk shell commands require explicit approval in `supervised` mode.\n",
        );
    }
    if autonomy.block_high_risk_commands {
        instructions.push_str(
            "- High-risk shell commands are blocked even when command names are allowed.\n",
        );
    }
    instructions.push_str(
        "- If a requested command is outside policy, choose allowed alternatives and explain the limitation.\n",
    );

    instructions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_allowlist_with_normalised_unique_entries() {
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Supervised;
        autonomy.allowed_commands = vec!["grep".into(), "cat".into(), "grep".into()];

        let out = build_shell_policy_instructions(&autonomy);

        assert!(out.contains("## Shell Policy"));
        assert!(out.contains("Autonomy level: `supervised`"));
        assert!(out.contains("`cat`"));
        assert!(out.contains("`grep`"));
        // Duplicate "grep" must collapse — BTreeSet de-dupes.
        let grep_count = out.matches("`grep`").count();
        assert_eq!(grep_count, 1, "duplicates must be collapsed, got {out}");
    }

    #[test]
    fn handles_wildcard_explicitly_in_allowlist() {
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Full;
        autonomy.allowed_commands = vec!["*".into()];

        let out = build_shell_policy_instructions(&autonomy);

        assert!(out.contains("Autonomy level: `full`"));
        assert!(out.contains("wildcard `*`"));
    }

    #[test]
    fn read_only_short_circuits_with_disabled_message() {
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::ReadOnly;

        let out = build_shell_policy_instructions(&autonomy);

        assert!(out.contains("Autonomy level: `read_only`"));
        assert!(out.contains("Shell execution is disabled"));
        // Must NOT mention allowlist or approval — read_only short-circuits.
        assert!(!out.contains("Allowed commands"));
        assert!(!out.contains("require explicit approval"));
    }

    #[test]
    fn supervised_with_medium_risk_approval_emits_approval_line() {
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Supervised;
        autonomy.require_approval_for_medium_risk = true;
        autonomy.allowed_commands = vec!["ls".into()];

        let out = build_shell_policy_instructions(&autonomy);
        assert!(out.contains("Medium-risk shell commands require explicit approval"));
    }

    #[test]
    fn full_mode_omits_supervised_approval_line() {
        // Approval gating applies only in supervised mode, even when
        // require_approval_for_medium_risk is left at its default true.
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Full;
        autonomy.require_approval_for_medium_risk = true;
        autonomy.allowed_commands = vec!["ls".into()];

        let out = build_shell_policy_instructions(&autonomy);
        assert!(!out.contains("require explicit approval"));
    }

    #[test]
    fn high_risk_block_flag_emits_block_line() {
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Full;
        autonomy.block_high_risk_commands = true;
        autonomy.allowed_commands = vec!["ls".into()];

        let out = build_shell_policy_instructions(&autonomy);
        assert!(out.contains("High-risk shell commands are blocked"));
    }

    #[test]
    fn empty_allowlist_explicitly_says_none_configured() {
        // Empty list must make the model aware that no shell will run,
        // not just leave it ambiguous.
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Supervised;
        autonomy.allowed_commands = vec![];

        let out = build_shell_policy_instructions(&autonomy);
        assert!(out.contains("none configured"));
        assert!(out.contains("rejected"));
    }

    #[test]
    fn long_allowlist_is_truncated_with_count_suffix() {
        // Past MAX_DISPLAY_COMMANDS the prompt MUST summarise rather
        // than dumping every name (token budget guard).
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Supervised;
        autonomy.allowed_commands = (0..(MAX_DISPLAY_COMMANDS + 5))
            .map(|i| format!("cmd{i:03}"))
            .collect();

        let out = build_shell_policy_instructions(&autonomy);
        assert!(out.contains("(+5 more)"));
    }

    #[test]
    fn whitespace_only_entries_are_dropped() {
        // Operator-pasted lists often have stray "  " or "\n" entries;
        // they must not appear as backtick-empty markdown noise.
        let mut autonomy = AutonomyConfig::default();
        autonomy.level = AutonomyLevel::Supervised;
        autonomy.allowed_commands = vec!["ls".into(), "   ".into(), "\t".into()];

        let out = build_shell_policy_instructions(&autonomy);
        assert!(out.contains("`ls`"));
        assert!(!out.contains("``"));
        assert!(!out.contains("`   `"));
    }
}
