//! Streaming control sentinels for the agent loop's draft channel.
//!
//! While a tool-call turn is in progress plaw streams two kinds of
//! payloads through the same `mpsc` delta channel:
//!
//!   1. user-visible deltas (final-answer text, partial completions);
//!   2. internal progress lines (thinking summaries, tool-call traces,
//!      compaction notices) that should be displayed in CLI/debug
//!      surfaces but suppressed from chat-channel output by default.
//!
//! Multiplexing both over one channel keeps the streaming pipeline
//! single-receiver simple; distinguishing the two kinds requires
//! in-band sentinels. We use NUL-bracketed ASCII tags because:
//!
//!   - they are syntactically invalid in any language the model emits
//!     as natural text (NUL-terminated strings can't contain a literal
//!     NUL), so collisions with model output are not possible;
//!   - they survive verbatim across the `tokio::sync::mpsc` boundary
//!     and the `&str` -> `String` -> websocket-frame path in
//!     `gateway/ws.rs`;
//!   - they are cheap to detect with `str::starts_with` /
//!     `str::strip_prefix` in the receiver.
//!
//! [`DRAFT_CLEAR_SENTINEL`] tells the draft updater to discard everything
//! it has accumulated so far before the final answer arrives — used to
//! wipe per-turn progress noise so the user sees only the clean
//! response, not the trace that produced it.
//!
//! [`DRAFT_PROGRESS_SENTINEL`] is a *prefix*: any delta starting with
//! this tag is an internal progress line, and channel layers can
//! suppress them by default (only exposing when the user explicitly
//! asks for command/tool-execution detail).
//!
//! These constants are part of the cross-module contract between
//! `agent::loop_`, `channels`, and `gateway::ws` — changing the byte
//! values is a breaking change for any in-flight delta consumer.

/// Sentinel that instructs the draft updater to clear accumulated text.
///
/// Sent through `on_delta` immediately before streaming the final
/// assistant response so any progress / thinking / tool-trace lines
/// that were emitted during the tool-call loop are replaced with the
/// clean answer rather than appended after them.
///
/// The value is a literal NUL-bracketed ASCII tag (`\x00CLEAR\x00`);
/// receivers compare with exact equality (`delta == DRAFT_CLEAR_SENTINEL`),
/// not prefix-match — a CLEAR delta carries no payload after the tag.
pub(crate) const DRAFT_CLEAR_SENTINEL: &str = "\x00CLEAR\x00";

/// Sentinel *prefix* marking a delta as an internal progress line
/// (thinking summary, tool-call trace, compaction notice, etc.) rather
/// than user-visible final-answer text.
///
/// The value is a literal NUL-bracketed ASCII tag (`\x00PROGRESS\x00`)
/// followed by the actual progress payload. Receivers detect a
/// progress line with `delta.starts_with(DRAFT_PROGRESS_SENTINEL)` or
/// extract the payload via `delta.strip_prefix(DRAFT_PROGRESS_SENTINEL)`.
///
/// Channel layers (Telegram, Discord, web UI) typically *suppress*
/// progress deltas by default and only forward them when the user has
/// opted into a "show tool execution" / verbose mode — so the model's
/// internal noise doesn't pollute the conversation surface.
pub(crate) const DRAFT_PROGRESS_SENTINEL: &str = "\x00PROGRESS\x00";

#[cfg(test)]
mod tests {
    use super::*;

    // ── Byte-value invariants ────────────────────────────────────────
    //
    // These tests exist to *pin* the exact byte value of each sentinel,
    // not to reproduce the constant's source. The values are part of
    // an implicit cross-module contract (agent::loop_ ⇄ channels ⇄
    // gateway::ws), so an accidental edit to the literal here would
    // silently desynchronise consumers that compare against the same
    // bytes via `delta == "..."` / `delta.starts_with("...")`.

    #[test]
    fn clear_sentinel_byte_value_is_pinned() {
        assert_eq!(DRAFT_CLEAR_SENTINEL, "\x00CLEAR\x00");
        assert_eq!(DRAFT_CLEAR_SENTINEL.len(), 7);
    }

    #[test]
    fn progress_sentinel_byte_value_is_pinned() {
        assert_eq!(DRAFT_PROGRESS_SENTINEL, "\x00PROGRESS\x00");
        assert_eq!(DRAFT_PROGRESS_SENTINEL.len(), 10);
    }

    // ── Disjointness invariants ──────────────────────────────────────
    //
    // The two sentinels MUST be distinguishable: receivers commonly
    // first check for an exact CLEAR match and then a PROGRESS prefix
    // match, so accidentally making CLEAR a prefix of PROGRESS (or the
    // other way around) would route progress deltas through the clear
    // path and wipe the draft buffer mid-stream.

    #[test]
    fn clear_is_not_a_prefix_of_progress() {
        assert!(!DRAFT_PROGRESS_SENTINEL.starts_with(DRAFT_CLEAR_SENTINEL));
    }

    #[test]
    fn progress_is_not_a_prefix_of_clear() {
        assert!(!DRAFT_CLEAR_SENTINEL.starts_with(DRAFT_PROGRESS_SENTINEL));
    }

    // ── NUL-bracket invariant ────────────────────────────────────────

    #[test]
    fn both_sentinels_are_nul_bracketed() {
        // Leading + trailing NUL is the property that makes the
        // sentinels collision-free against any natural-language model
        // output (which cannot contain literal NUL bytes by C-string
        // convention upstream of the JSON encoder). If a future edit
        // dropped one of the brackets the sentinel could be matched
        // inside ordinary text.
        for s in [DRAFT_CLEAR_SENTINEL, DRAFT_PROGRESS_SENTINEL] {
            assert!(s.starts_with('\x00'), "{s:?} must start with NUL");
            assert!(s.ends_with('\x00'), "{s:?} must end with NUL");
        }
    }

    // ── Strip-prefix payload-recovery sanity ─────────────────────────

    #[test]
    fn progress_prefix_strips_to_payload() {
        // Progress deltas carry payload after the sentinel; consumers
        // recover it via strip_prefix. This pins that contract.
        let delta = format!("{DRAFT_PROGRESS_SENTINEL}thinking about it…");
        let payload = delta.strip_prefix(DRAFT_PROGRESS_SENTINEL).unwrap();
        assert_eq!(payload, "thinking about it…");
    }
}
