//! End-to-end integration tests for PR #88b's Token IL spawn
//! primitives. Lives in `plaw/tests/` because we need
//! `CARGO_BIN_EXE_plaw-il-probe` — cargo only populates that env var
//! for integration tests of the SAME package as the bin target. The
//! `plaw-il-probe` binary lives at `plaw/src/bin/plaw-il-probe.rs` for
//! that reason.
//!
//! These tests are Windows-only — they use the production
//! `spawn_with_lowered_token` to launch the probe at a target IL,
//! `wait()` for exit, and read the probe's locale-invariant SID
//! output. Without the probe + spawn pipeline, plaw's Phase 1b/c
//! work (Sandbox trait extension + per-tool config) would be flying
//! blind on whether the kernel actually applied the IL.

#![cfg(target_os = "windows")]

use plaw::windows_token_il::{spawn_with_lowered_token, IntegrityLevel};

fn probe_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_plaw-il-probe"))
}

/// End-to-end regression for Low IL spawning. Spawn the probe at Low,
/// wait, accept EITHER:
/// - exit 0 + SID == S-1-16-4096 (probe somehow had a Low-IL-writable
///   output path; unusual but possible if tempdir has a Low ACL),
/// - exit 5 (probe's `File::create` failed with access-denied — the
///   kernel-enforced mandatory label deny working as designed against
///   a Low-IL process writing to a Medium-IL tempdir). THIS is the
///   common outcome; it IS the proof that the IL was actually applied.
///
/// Any other exit code would be a real bug (e.g. syscall failure in
/// the probe, or the probe writing the WRONG SID).
#[test]
fn spawn_probe_at_low_applies_low_il() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let out = tmp.path().join("probe-low.txt");

    let child = spawn_with_lowered_token(
        probe_path(),
        &[out.to_string_lossy().into_owned()],
        IntegrityLevel::Low,
    )
    .expect("spawn_with_lowered_token(Low) must succeed unelevated");

    let pid = child.id();
    assert!(pid > 0, "LoweredChild must report a valid OS pid");

    let status = child.wait().expect("wait must succeed");
    let code = status.code().unwrap_or(-1);

    if code == 0 {
        let sid = std::fs::read_to_string(&out)
            .expect("probe at Low exited 0 → it MUST have written SID");
        assert_eq!(
            sid.trim(),
            IntegrityLevel::Low.sid_string().unwrap(),
            "child IL SID must match the requested level"
        );
    } else if code == 5 {
        // Kernel-enforced filesystem-deny — proof that Low IL was
        // applied and the probe COULDN'T write to the Medium-IL
        // tempdir. This is the expected outcome on a standard
        // Windows test box. The probe HAD to start (DLL init didn't
        // fail like Untrusted) → IL machinery worked end-to-end.
    } else {
        panic!(
            "probe at Low IL should exit 0 (success) or 5 (filesystem-deny); \
             got exit code {code}. Output path: {}",
            out.display()
        );
    }
}

/// Untrusted IL is restrictive enough that the C runtime DLL init
/// (vcruntime / kernel32) often fails before the probe's main()
/// can run. STATUS_DLL_INIT_FAILED (0xC0000142) is itself proof that
/// the IL was applied — the kernel refused to let the probe load
/// at Untrusted. Acceptable outcomes:
/// - exit 0 + SID written (rare; only on very permissive
///   environments),
/// - exit 5 (filesystem-deny),
/// - exit code STATUS_DLL_INIT_FAILED (the IL was *too* restrictive
///   for the C runtime — itself a positive signal).
#[test]
fn spawn_probe_at_untrusted_applies_untrusted_il() {
    const STATUS_DLL_INIT_FAILED: i32 = 0xC000_0142_u32 as i32;

    let tmp = tempfile::tempdir().expect("create tempdir");
    let out = tmp.path().join("probe-untrusted.txt");

    let child = spawn_with_lowered_token(
        probe_path(),
        &[out.to_string_lossy().into_owned()],
        IntegrityLevel::Untrusted,
    )
    .expect("spawn_with_lowered_token(Untrusted) must succeed unelevated");

    let pid = child.id();
    assert!(pid > 0, "LoweredChild must report a valid OS pid");

    let status = child.wait().expect("wait must succeed");
    let code = status.code().unwrap_or(-1);

    if code == 0 {
        let sid = std::fs::read_to_string(&out)
            .expect("probe at Untrusted exited 0 → it MUST have written SID");
        assert_eq!(sid.trim(), IntegrityLevel::Untrusted.sid_string().unwrap(),);
    } else if code == 5 || code == STATUS_DLL_INIT_FAILED {
        // Both are valid signals that the IL was applied:
        // - 5 = probe ran but couldn't write (filesystem deny)
        // - 0xC0000142 = OS refused to even load the DLLs (IL too low)
    } else {
        panic!(
            "probe at Untrusted IL should exit 0 / 5 / STATUS_DLL_INIT_FAILED; \
             got exit code 0x{code:x} (decimal {code})"
        );
    }
}

/// Spawn at Medium → child observes the same IL as the test runner
/// (Medium when unelevated). Demonstrates spawn_with_lowered_token
/// works at the same-level boundary — a sentinel against future
/// refactors that accidentally optimize "level == current" into a
/// short-circuit but break the spawn path.
#[test]
fn spawn_probe_at_medium_writes_medium_sid() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let out = tmp.path().join("probe-medium.txt");

    let child = spawn_with_lowered_token(
        probe_path(),
        &[out.to_string_lossy().into_owned()],
        IntegrityLevel::Medium,
    )
    .expect("spawn_with_lowered_token(Medium) must succeed for a Medium parent");

    let status = child.wait().expect("wait must succeed");
    assert!(
        status.success(),
        "probe at Medium IL should exit 0; got {status:?}"
    );

    let sid = std::fs::read_to_string(&out)
        .expect("probe should have written its SID to the output path");
    assert_eq!(sid.trim(), IntegrityLevel::Medium.sid_string().unwrap(),);
}
