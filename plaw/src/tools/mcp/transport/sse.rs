//! Server-Sent Events parser — WHATWG HTML §9.2.6 compliant.
//!
//! Pure, sync, byte-fed state machine. Decoupled from HTTP and from
//! JSON-RPC. The caller (currently
//! `transport::http::HttpTransport::read_sse_response`) drives the
//! parser by feeding `reqwest::Response::bytes_stream()` chunks. The
//! parser owns:
//!
//! - Line terminator trichotomy (`\n`, `\r\n`, bare `\r`) — WHATWG
//!   requires all three.
//! - Single-leading-space strip after `:` (NOT all whitespace —
//!   exactly one space per spec).
//! - Multi-line `data:` accumulation joined with `\n` (NOT space, NOT
//!   concat without separator).
//! - `event:` / `id:` / `retry:` field recognition; comments
//!   (`:`-prefixed lines) silently consumed.
//! - One-shot UTF-8 BOM consumption at stream start.
//! - Mid-codepoint and mid-line chunk-boundary safety via `Vec<u8>`
//!   internal buffer. Decoding happens AFTER locating a complete
//!   line — `String` buffer would risk a `from_utf8` panic on a
//!   chunk that splits a codepoint mid-byte. Critical: the existing
//!   OpenAI `SseAccumulator` in `providers/compatible.rs` uses a
//!   `String` buffer and has this latent panic; this module
//!   deliberately does NOT inherit that bug.
//! - Explicit byte cap on the internal buffer — a hung or malicious
//!   server streaming infinite bytes without a blank-line dispatch
//!   would otherwise grow the buffer without bound.
//! - NULL-byte rejection on `id:` values per WHATWG.
//! - Partial-event-on-EOF rejection in `finish()` per WHATWG §9.2.5.
//!
//! The parser does NOT know about JSON-RPC, MCP, Mcp-Session-Id, or
//! the OpenAI `[DONE]` sentinel — those concerns live in the caller.
//! Field names are case-sensitive per WHATWG: `Data:` is unrecognized.
//!
//! Last-Event-ID is CAPTURED here for future Phase 3 reconnect-resend
//! wiring (the HttpTransport mirrors it into a `Mutex<Option<String>>`
//! field for cross-call observability), but Phase 2a does NOT
//! retransmit it.

use anyhow::{bail, Result};

/// One dispatched SSE event per WHATWG §9.2.6 "dispatch the event"
/// algorithm.
///
/// `event` is `None` when the server omits the `event:` field — the
/// caller treats that as the spec-default `"message"`. `data` is the
/// fully-assembled, newline-joined data buffer (NO trailing newline).
/// `id` is the most recent `id:` value at the time of dispatch
/// (events without an `id:` reset/inherit per spec — we capture
/// per-event rather than letting the caller chase the global state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

/// SSE byte-fed state machine.
pub(crate) struct SseParser {
    /// Raw bytes received but not yet parsed into complete lines.
    /// `Vec<u8>` (not `String`) so chunk boundaries that split a
    /// UTF-8 codepoint mid-byte do not panic. UTF-8 validation
    /// happens after locating a complete line.
    buf: Vec<u8>,
    /// Current event-name buffer (`event:` field). `None` when no
    /// `event:` field has appeared in the current event block.
    event_name: Option<String>,
    /// Current data buffer. WHATWG: multi-line `data:` joined with
    /// `\n`. Trailing `\n` is stripped at dispatch time.
    data: String,
    /// Current event id (`id:` field).
    current_id: Option<String>,
    /// Most recent `id:` ever seen on a DISPATCHED event. Captured
    /// for future Phase 3 reconnect-resend; never retransmitted in
    /// Phase 2a.
    last_event_id: Option<String>,
    /// True after the leading UTF-8 BOM (if any) has been consumed.
    bom_consumed: bool,
    /// Maximum internal buffer size in bytes. A server that streams
    /// without ever emitting a blank-line dispatch must not be able
    /// to grow our memory unboundedly.
    byte_cap: usize,
    /// Last byte fed was `\r`. WHATWG: when the next byte is `\n`,
    /// the pair is one terminator (CRLF). When the next byte is
    /// anything else, the `\r` was a bare-CR terminator on its own.
    pending_cr: bool,
}

impl SseParser {
    /// Construct a parser with a byte cap on the internal buffer.
    /// 4 MiB is a reasonable default for MCP — any single
    /// JSON-RPC response that doesn't fit in 4 MiB is almost
    /// certainly a buggy server.
    pub(crate) fn new(byte_cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            event_name: None,
            data: String::new(),
            current_id: None,
            last_event_id: None,
            bom_consumed: false,
            byte_cap,
            pending_cr: false,
        }
    }

    /// Feed a chunk of bytes. Returns any events that were fully
    /// dispatched within this chunk (zero, one, or many). Returns an
    /// error if the byte cap is exceeded without dispatch.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>> {
        if self.buf.len() + chunk.len() > self.byte_cap {
            bail!(
                "SSE buffer exceeded byte cap ({} bytes); aborting stream to prevent unbounded growth",
                self.byte_cap
            );
        }
        self.buf.extend_from_slice(chunk);

        // BOM strip exactly once at stream start. Per WHATWG: only
        // consume the BOM if it appears at offset 0; otherwise treat
        // those bytes as data.
        if !self.bom_consumed && self.buf.len() >= 3 {
            if self.buf.starts_with(b"\xEF\xBB\xBF") {
                self.buf.drain(..3);
            }
            self.bom_consumed = true;
        }

        let mut dispatched = Vec::new();
        loop {
            let Some((line_end, terminator_len)) = find_line_terminator(&self.buf, self.pending_cr)
            else {
                break;
            };
            // Special case: the previous chunk ended with `\r` and
            // this chunk's first byte is `\n` — that's a CRLF pair
            // straddling the chunk boundary; consume the `\n` and
            // ignore it (the `\r` already dispatched the line).
            if self.pending_cr {
                self.pending_cr = false;
                if line_end == 0 {
                    // Lone `\n` immediately after our pending `\r`:
                    // consume it and start over without dispatching.
                    self.buf.drain(..1);
                    continue;
                }
            }

            let line_bytes: Vec<u8> = self.buf.drain(..line_end).collect();
            self.buf.drain(..terminator_len);

            // If the terminator was a bare `\r` at the very end of
            // the buffer (we don't yet know if `\n` follows), set the
            // pending flag and stop until more bytes arrive.
            if terminator_len == 1
                && line_bytes.is_empty()
                && !self.buf.is_empty()
                && self.buf[0] == b'\n'
            {
                // This shouldn't happen via find_line_terminator —
                // defensive only.
                self.buf.drain(..1);
                continue;
            }

            // UTF-8 validate the line ONLY now that we have a
            // complete line. Lossy decode would corrupt a future
            // codepoint that gets split across lines, but lines
            // themselves are byte sequences delimited by ASCII
            // terminators — full codepoints always fit within a
            // single line.
            let line = match std::str::from_utf8(&line_bytes) {
                Ok(s) => s.to_string(),
                Err(e) => bail!("SSE line was not valid UTF-8: {e}"),
            };

            if line.is_empty() {
                // Blank line → dispatch event.
                if let Some(ev) = self.dispatch_event() {
                    dispatched.push(ev);
                }
            } else if line.starts_with(':') {
                // Comment. Ignore.
            } else {
                self.process_field_line(&line)?;
            }
        }

        // Check for trailing pending-CR: if the buffer's last byte is
        // `\r`, we may be in a CRLF pair straddling chunks. Mark for
        // next feed.
        if !self.buf.is_empty() && *self.buf.last().unwrap() == b'\r' {
            // We already consumed this `\r` as a terminator in the
            // loop above (find_line_terminator would have matched
            // it). The flag is only needed if a `\n` arrives FIRST
            // in the next chunk.
            self.pending_cr = true;
            self.buf.pop();
        }

        Ok(dispatched)
    }

    /// Finalize the stream. Returns any final events that were
    /// pending due to an end-of-stream bare-CR terminator
    /// (incremental mode cannot resolve `\r` at the very end of a
    /// chunk until either a follow-up chunk arrives or `finish()`
    /// is called). Returns an error if a TRUE partial event was
    /// buffered (per WHATWG §9.2.5 — never deliver half-events).
    pub(crate) fn finish(mut self) -> Result<Vec<SseEvent>> {
        let mut final_events = Vec::new();
        // A pending bare-CR is unambiguously a terminator at EOF.
        // Process it as if we had received an empty-line dispatch
        // signal: any buffered event state is emitted.
        if self.pending_cr {
            self.pending_cr = false;
            if let Some(ev) = self.dispatch_event() {
                final_events.push(ev);
            }
        }
        // A partial event = any field state accumulated without a
        // subsequent dispatch.
        if !self.buf.is_empty()
            || self.event_name.is_some()
            || !self.data.is_empty()
            || self.current_id.is_some()
        {
            bail!(
                "SSE stream ended with a partial event buffered (no blank line); discarding half-event per WHATWG §9.2.5"
            );
        }
        Ok(final_events)
    }

    /// Most recent `id:` value across all DISPATCHED events. Returns
    /// `None` until the first dispatched event with an `id:` field.
    pub(crate) fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    fn process_field_line(&mut self, line: &str) -> Result<()> {
        // Field name = up to first `:`. Spec: when no `:`, the whole
        // line is the field name and the value is the empty string.
        let (name, value) = match line.find(':') {
            Some(idx) => {
                let (name, rest) = line.split_at(idx);
                // Strip the colon and ONE leading space (not all).
                let value = &rest[1..];
                let value = value.strip_prefix(' ').unwrap_or(value);
                (name, value)
            }
            None => (line, ""),
        };

        match name {
            "event" => self.event_name = Some(value.to_string()),
            "data" => {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value);
            }
            "id" => {
                // WHATWG: NULL bytes in `id:` cause the field to be
                // ignored entirely.
                if !value.contains('\0') {
                    self.current_id = Some(value.to_string());
                }
            }
            "retry" => {
                // Parse but discard. Phase 2a does not honour the
                // reconnect hint (Phase 3 will when the GET listener
                // lands).
                let _ = value.parse::<u32>();
            }
            _ => {
                // Unknown field name — silently ignored per spec.
                // Case-sensitive: `Data` is NOT `data`.
            }
        }
        Ok(())
    }

    fn dispatch_event(&mut self) -> Option<SseEvent> {
        // Per WHATWG §9.2.6 "dispatch the event": if data buffer is
        // empty AND event name has not been set, do NOT dispatch.
        if self.data.is_empty() && self.event_name.is_none() {
            // Still reset current_id per spec? No — id persists across
            // empty dispatches.
            self.current_id = None;
            return None;
        }

        let event = SseEvent {
            event: self.event_name.take(),
            data: std::mem::take(&mut self.data),
            id: self.current_id.take(),
        };
        if let Some(id) = &event.id {
            self.last_event_id = Some(id.clone());
        }
        Some(event)
    }
}

/// Locate the next line terminator in `buf` and return the offset of
/// the line content (excluding terminator) plus the terminator
/// length. WHATWG accepts `\n`, `\r\n`, and bare `\r`.
///
/// If `pending_cr` is true (the previous chunk ended with `\r`), a
/// leading `\n` is one byte of a CRLF pair (terminator_len = 1
/// against `line_end = 0`).
fn find_line_terminator(buf: &[u8], pending_cr: bool) -> Option<(usize, usize)> {
    if pending_cr {
        // Previous chunk ended in `\r`. If THIS chunk starts with
        // `\n`, that's the second half of a CRLF — but we already
        // dispatched. Caller handles this by drain-and-skip.
        if !buf.is_empty() && buf[0] == b'\n' {
            return Some((0, 1));
        }
    }

    let mut i = 0;
    while i < buf.len() {
        match buf[i] {
            b'\n' => return Some((i, 1)),
            b'\r' => {
                if i + 1 < buf.len() && buf[i + 1] == b'\n' {
                    return Some((i, 2));
                }
                // Bare `\r` is a terminator only if we have more
                // bytes after it (otherwise it might be the first
                // half of a CRLF whose `\n` is in the next chunk).
                if i + 1 < buf.len() {
                    return Some((i, 1));
                }
                // Else: bare `\r` at EOF-of-buffer. Caller's
                // `pending_cr` post-loop check handles this.
                return None;
            }
            _ => i += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: usize = 4 * 1024 * 1024;

    fn one(parser: &mut SseParser, bytes: &[u8]) -> Vec<SseEvent> {
        parser.feed(bytes).expect("feed must succeed")
    }

    #[test]
    fn feeds_canonical_message_event() {
        let mut p = SseParser::new(CAP);
        let events = one(
            &mut p,
            b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n",
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(
            events[0].data,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}"
        );
        assert!(events[0].id.is_none());
    }

    /// Parameterized regression for every byte-boundary chunk split.
    /// 200-byte fixture split into two feeds at every offset 1..199;
    /// the dispatched event vector MUST be identical to the single-
    /// feed case.
    #[test]
    fn splits_at_every_byte_boundary() {
        let fixture: &[u8] =
            b"event: message\nid: abc-123\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"x\":\"hello\"}}\n\n";
        let expected = {
            let mut p = SseParser::new(CAP);
            p.feed(fixture).unwrap()
        };
        assert_eq!(expected.len(), 1, "fixture should dispatch one event");

        for split in 1..fixture.len() {
            let mut p = SseParser::new(CAP);
            let a = p.feed(&fixture[..split]).expect("first feed");
            let b = p.feed(&fixture[split..]).expect("second feed");
            let mut got = a;
            got.extend(b);
            assert_eq!(
                got, expected,
                "split at byte {split} produced different events"
            );
        }
    }

    /// UTF-8 codepoint MUST NOT be corrupted by a chunk-boundary
    /// split. The existing OpenAI `SseAccumulator` uses a `String`
    /// buffer and would panic here; we use `Vec<u8>` and validate
    /// only at line boundaries, so this works.
    #[test]
    fn splits_mid_utf8_codepoint() {
        let mut p = SseParser::new(CAP);
        // 世 = U+4E16 = 3 bytes: 0xE4 0xB8 0x96
        let chunk1 = b"data: \xE4\xB8";
        let chunk2 = b"\x96\n\n";
        let a = p.feed(chunk1).expect("first feed");
        let b = p.feed(chunk2).expect("second feed");
        let mut got = a;
        got.extend(b);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, "世");
    }

    /// All three WHATWG-allowed line terminators must produce the
    /// same logical events: LF, CRLF, bare CR.
    #[test]
    fn handles_crlf_lf_and_bare_cr_line_endings() {
        fn collect(bytes: &[u8]) -> Vec<SseEvent> {
            let mut p = SseParser::new(CAP);
            let mut events = p.feed(bytes).unwrap();
            events.extend(p.finish().unwrap());
            events
        }
        // Bare-CR-at-EOF is ambiguous in incremental mode (could be
        // the first byte of a CRLF whose `\n` is in the next chunk),
        // so it stays pending until either the next feed
        // disambiguates OR `finish()` is called. Collect both to
        // compare against the LF baseline.
        let canonical = collect(b"event: message\ndata: x\n\n");
        let crlf = collect(b"event: message\r\ndata: x\r\n\r\n");
        let cr = collect(b"event: message\rdata: x\r\r");
        assert_eq!(canonical, crlf);
        assert_eq!(canonical, cr);
    }

    /// Strip exactly ONE leading space after the colon — not all.
    /// `data: hello` → "hello"; `data:hello` → "hello";
    /// `data:  hello` → " hello" (one space remains).
    ///
    /// Note: `data: \n\n` (empty value) does NOT dispatch per WHATWG
    /// §9.2.6 — an empty data buffer with no event name hits the
    /// "discard" branch. Covered by `empty_event_block_does_not_dispatch`.
    #[test]
    fn strips_one_leading_space_after_colon() {
        let cases: &[(&[u8], &str)] = &[
            (b"data: hello\n\n", "hello"),
            (b"data:hello\n\n", "hello"),
            (b"data:  hello\n\n", " hello"),
        ];
        for (input, want) in cases {
            let mut p = SseParser::new(CAP);
            let events = one(&mut p, input);
            assert_eq!(events.len(), 1, "input {:?} should dispatch", input);
            assert_eq!(events[0].data, *want, "input {:?}", input);
        }
    }

    /// Multi-line `data:` joined with `\n` (NOT space, NOT
    /// concat-without-separator).
    #[test]
    fn joins_multiline_data_with_newline() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"data: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    /// Lines starting with `:` are comments — silently consumed.
    #[test]
    fn ignores_comment_lines() {
        let mut p = SseParser::new(CAP);
        let events = one(
            &mut p,
            b":keepalive\n:another comment\nevent: message\ndata: x\n\n",
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data, "x");
    }

    /// BOM consumed exactly once at stream start; subsequent BOM-byte
    /// sequences inside a data field are not stripped.
    #[test]
    fn consumes_bom_only_at_stream_start() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"\xEF\xBB\xBFevent: message\ndata: x\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));

        // Subsequent BOM-bytes inside a data field should pass through.
        let mut p2 = SseParser::new(CAP);
        let events = one(&mut p2, b"data: a\xEF\xBB\xBFb\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "a\u{FEFF}b");
    }

    /// Partial event on EOF MUST surface as an error per
    /// WHATWG §9.2.5.
    #[test]
    fn discards_partial_event_on_finish() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"data: incomplete\n");
        assert!(events.is_empty(), "no blank line yet → no dispatch");
        let err = p.finish().unwrap_err();
        assert!(err.to_string().contains("partial event"), "got: {err}");
    }

    /// Field names are case-sensitive. `Data:` is treated as an
    /// unknown field name and ignored.
    #[test]
    fn field_names_are_case_sensitive() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"Data: x\n\n");
        // No `data:` field arrived, no `event:` either → no dispatch.
        assert!(events.is_empty());
    }

    /// `id:` containing a NULL byte is ignored entirely per WHATWG.
    #[test]
    fn id_containing_null_byte_rejected() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"id: foo\0bar\ndata: x\n\n");
        assert_eq!(events.len(), 1);
        assert!(events[0].id.is_none(), "NULL-bearing id must be ignored");
    }

    /// Byte cap exceeded MUST return error, not panic, not silently
    /// truncate.
    #[test]
    fn byte_cap_exceeded_returns_error() {
        let mut p = SseParser::new(64);
        let huge = vec![b'x'; 128];
        let err = p.feed(&huge).unwrap_err();
        assert!(err.to_string().contains("byte cap"));
    }

    /// OpenAI's `[DONE]` sentinel has no special meaning in spec-
    /// compliant SSE — it's just data.
    #[test]
    fn no_done_sentinel_special_casing() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"data: [DONE]\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "[DONE]");
    }

    /// `last_event_id` tracks the most recent dispatched event with
    /// an `id:` field.
    #[test]
    fn captures_last_event_id_across_events() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"id: e1\ndata: a\n\nid: e2\ndata: b\n\ndata: c\n\n");
        assert_eq!(events.len(), 3);
        // Last seen id was "e2"; third event has no id, but
        // last_event_id still reflects "e2".
        assert_eq!(p.last_event_id(), Some("e2"));
    }

    /// `event:` without `data:` still dispatches.
    #[test]
    fn dispatches_event_with_event_name_but_no_data() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"event: ping\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("ping"));
        assert_eq!(events[0].data, "");
    }

    /// Empty event blocks (just a blank line) do NOT dispatch.
    #[test]
    fn empty_event_block_does_not_dispatch() {
        let mut p = SseParser::new(CAP);
        let events = one(&mut p, b"\n\n\n");
        assert!(events.is_empty());
    }
}
