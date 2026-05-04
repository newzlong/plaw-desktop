//! Numeric budgets for the agent tool-call loop.
//!
//! Six values that bound runtime behaviour: how big a streaming chunk
//! is, how many iterations the loop tolerates, when auto-save kicks in,
//! how long history grows before trimming, how long we wait on a
//! non-CLI approval, and how often we poll for one. Extracted from
//! `loop_.rs` so the budget table is auditable in one place — when
//! tuning latency or memory pressure, this is the file to edit.
//!
//! Most callers should *not* read these directly: the loop reads
//! config-driven values (`agent.max_tool_iterations`,
//! `agent.max_history_messages`) and only falls back to the constants
//! here when the config is missing or set to zero.

/// Minimum characters per chunk when relaying LLM text to a streaming
/// draft. Lower values flood the channel transport (Telegram, Discord)
/// with rate-limit-eligible draft updates; higher values make the
/// draft visibly stutter.
pub(super) const STREAM_CHUNK_MIN_CHARS: usize = 80;

/// Default maximum agentic tool-use iterations per user message to
/// prevent runaway loops. Used as a safe fallback when
/// `max_tool_iterations` is unset or configured as zero. Set high
/// enough to allow full-chain autonomous work
/// (read → edit → build → fix → repeat) while still preventing
/// infinite runaway. Mid-loop trim keeps context manageable.
///
/// Must be ≤ `i64::MAX` so the value round-trips through TOML (whose
/// integer type is i64). `usize::MAX` would overflow on 64-bit
/// platforms and break dashboard config serialize→parse cycles — see
/// `gateway::api::tests::normalize_dashboard_config_toml_*`. `i64::MAX`
/// is still effectively "no built-in cap" (~9.2 quintillion).
pub(super) const DEFAULT_MAX_TOOL_ITERATIONS: usize = i64::MAX as usize;

/// Minimum user-message length (in chars) for auto-save to memory.
/// Matches the channel-side constant in `channels/mod.rs` — short
/// pings ("ok", "thanks") aren't worth a memory write and would
/// pollute the recall set.
pub(super) const AUTOSAVE_MIN_MESSAGE_CHARS: usize = 20;

/// Default trigger for auto-compaction when non-system message count
/// exceeds this threshold. Prefer passing the config-driven value via
/// `run_tool_call_loop`; this constant is only used when callers omit
/// the parameter.
pub(super) const DEFAULT_MAX_HISTORY_MESSAGES: usize = 50;

/// Maximum time the loop will wait for a non-CLI approval response
/// before treating the request as denied. Channels that drop the
/// approver's reply (network blip, app close) must not pin the loop
/// indefinitely.
pub(super) const NON_CLI_APPROVAL_WAIT_TIMEOUT_SECS: u64 = 300;

/// Poll interval while awaiting a non-CLI approval. Tight enough that
/// a fast "yes" reply lands in <0.5 s perceptible latency, loose
/// enough not to burn CPU on a 5-minute spin.
pub(super) const NON_CLI_APPROVAL_POLL_INTERVAL_MS: u64 = 250;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_tool_iterations_round_trips_through_toml_i64() {
        // Regression test for the `usize::MAX → i64::MAX` change made
        // when dashboard config serialisation broke. The constant must
        // fit in i64 so the value the loop falls back to matches what
        // a re-loaded config would produce.
        let n = DEFAULT_MAX_TOOL_ITERATIONS;
        let as_i64 = i64::try_from(n).expect("must fit in i64 for TOML round-trip");
        assert_eq!(as_i64, i64::MAX);
    }

    #[test]
    fn default_history_threshold_is_below_typical_context_window() {
        // 50 messages × ~1 KB avg = 50 KB; well under any provider's
        // context window even at the legacy ChatGPT-3.5 16 KB scale.
        // If we ever raise this, double-check it stays well below the
        // smallest provider's documented window.
        assert!(DEFAULT_MAX_HISTORY_MESSAGES > 0);
        assert!(DEFAULT_MAX_HISTORY_MESSAGES < 1000);
    }

    #[test]
    fn approval_poll_interval_fits_within_wait_timeout_with_room() {
        // Sanity: poll interval must be small enough that the wait
        // window allows a meaningful number of polls. At least 100
        // polls per timeout window catches "configured to never see a
        // reply" misconfigurations.
        let polls_per_timeout =
            (NON_CLI_APPROVAL_WAIT_TIMEOUT_SECS * 1000) / NON_CLI_APPROVAL_POLL_INTERVAL_MS;
        assert!(
            polls_per_timeout >= 100,
            "polls_per_timeout={polls_per_timeout}; raise the timeout or shrink the interval"
        );
    }

    #[test]
    fn stream_chunk_min_is_human_readable_size() {
        // Streaming chunks below ~20 chars feel like Morse code on the
        // receiving channel; chunks above ~200 chars defeat the purpose
        // of streaming (looks identical to a single send). 80 is the
        // documented sweet spot — pin the band so cleanup PRs don't
        // drift it accidentally.
        assert!((20..=200).contains(&STREAM_CHUNK_MIN_CHARS));
    }
}
