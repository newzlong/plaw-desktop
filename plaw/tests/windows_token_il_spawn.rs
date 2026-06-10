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

use plaw::windows_token_il::{
    current_process_integrity, spawn_with_lowered_token, spawn_with_lowered_token_piped,
    IntegrityLevel,
};

fn probe_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_plaw-il-probe"))
}

/// Full path to cmd.exe. `CreateProcessAsUserW`'s lpApplicationName
/// does NOT search PATH (unlike tokio's spawn), so piped tests must
/// use the absolute path — same gotcha PR #90 documented.
fn cmd_exe() -> std::ffi::OsString {
    std::env::var_os("ComSpec")
        .unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows\System32\cmd.exe"))
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
///
/// PR #93 (audit #11 self-review M-5) added the parent-IL guard
/// below. The test runner is normally at Medium IL on a typical
/// developer workstation, but CI runners can run elevated (High)
/// or under AppContainer / Win Sandbox (Low / Untrusted). In those
/// cases the test would either false-fail (probe sees High and
/// doesn't match Medium) or hang (Untrusted DLL init). The guard
/// skips with a clear stderr message rather than appearing flaky.
#[test]
fn spawn_probe_at_medium_writes_medium_sid() {
    let parent_il = current_process_integrity().expect("OpenProcessToken on self must succeed");
    if parent_il != IntegrityLevel::Medium {
        eprintln!(
            "Skipping spawn_probe_at_medium_writes_medium_sid: \
             parent IL is {parent_il:?}, test requires Medium. \
             (Elevated session / AppContainer / Win Sandbox CI runner?)"
        );
        return;
    }

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

// ─── Phase 1c.2a: piped-stdio spawn (named pipes + IOCP) ─────────────

/// Drain a `LoweredChild`'s piped stdout to a String via IOCP.
/// connect() completes the named-pipe handshake; read_to_end drains
/// until the child's write end closes (on exit). Bounded by an outer
/// `tokio::time::timeout` in each test.
#[cfg(target_os = "windows")]
async fn drain_stdout(child: &mut plaw::windows_token_il::LoweredChild) -> String {
    use tokio::io::AsyncReadExt;
    let mut server = child.take_stdout().expect("piped child must have stdout");
    server.connect().await.expect("pipe connect");
    let mut buf = Vec::new();
    server.read_to_end(&mut buf).await.expect("read stdout");
    String::from_utf8_lossy(&buf).into_owned()
}

/// THE Phase 1c.2a regression: a lowered-IL child's stdout is captured
/// through the IOCP-backed named pipe end-to-end. Spawns `cmd /C echo`
/// at Low IL and reads the echoed marker back from the pipe.
///
/// This is the capability ShellTool needs before Phase 1c.2c can lift
/// its deferred-feature bail.
#[tokio::test]
async fn spawn_piped_at_low_captures_child_stdout() {
    // Drain stderr concurrently so a chatty child can't deadlock on a
    // full stderr pipe buffer (the classic 4 KiB pipe deadlock) — even
    // though `echo` writes nothing to stderr, this models the real
    // drain pattern.
    let mut child = spawn_with_lowered_token_piped(
        cmd_exe(),
        &["/C".to_string(), "echo piped-low-marker".to_string()],
        IntegrityLevel::Low,
    )
    .expect("piped Low-IL spawn must succeed for a Medium parent");

    let mut stderr_server = child.take_stderr().expect("piped child must have stderr");
    let stderr_drain = async move {
        use tokio::io::AsyncReadExt;
        stderr_server.connect().await.ok();
        let mut b = Vec::new();
        let _ = stderr_server.read_to_end(&mut b).await;
        b
    };

    let out = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(drain_stdout(&mut child), stderr_drain)
    })
    .await
    .expect("piped read must not hang — IOCP path delivers child stdout");

    assert!(
        out.0.contains("piped-low-marker"),
        "captured stdout must contain the child's echo marker; got {:?}",
        out.0
    );

    // Reap the child so the test leaves no orphan.
    let _ = child.wait();
}

/// The captured child genuinely runs at Low IL: the probe (spawned
/// DIRECTLY, no cmd wrapper) queries its OWN token and prints the
/// locale-invariant mandatory-label SID `S-1-16-4096` to the piped
/// stdout. Proves the lowered token + the pipe capture work TOGETHER —
/// not just that some child wrote to the pipe. Spawning the probe
/// directly avoids the cmd→whoami grandchild handle-propagation quirk.
#[tokio::test]
async fn spawn_piped_at_low_child_token_is_low_il() {
    let parent_il = current_process_integrity().expect("get parent IL");
    if parent_il != IntegrityLevel::Medium {
        eprintln!("Skipping: parent IL is {parent_il:?}, test requires Medium");
        return;
    }

    let mut child = spawn_with_lowered_token_piped(
        probe_path(),
        &["--stdout".to_string()],
        IntegrityLevel::Low,
    )
    .expect("piped Low-IL spawn must succeed");

    let mut stderr_server = child.take_stderr().expect("stderr");
    let stderr_drain = async move {
        use tokio::io::AsyncReadExt;
        stderr_server.connect().await.ok();
        let mut b = Vec::new();
        let _ = stderr_server.read_to_end(&mut b).await;
    };

    let (stdout, ()) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::join!(drain_stdout(&mut child), stderr_drain)
    })
    .await
    .expect("probe pipe read must not hang");

    assert_eq!(
        stdout.trim(),
        IntegrityLevel::Low.sid_string().unwrap(),
        "probe's own-token SID read back through the captured pipe must \
         be the Low mandatory-label SID (S-1-16-4096), proving the \
         lowered token applied AND the capture works; got:\n{stdout}"
    );

    let _ = child.wait();
}

/// `spawn_with_lowered_token_piped` rejects `Default` at the front door
/// (mirrors the non-piped path). Defense-in-depth against passing the
/// Default sentinel through to the token machinery.
#[tokio::test]
async fn spawn_piped_rejects_default_level() {
    let err = spawn_with_lowered_token_piped(
        std::path::PathBuf::from("non-existent.exe"),
        &[],
        IntegrityLevel::Default,
    )
    .expect_err("Default must be rejected before any syscall");
    assert!(
        err.to_string().contains("Default"),
        "rejection must name the Default sentinel; got {err}"
    );
}

/// Called OUTSIDE a tokio runtime, the piped spawn fails fast with a
/// typed error instead of panicking deep inside the IOCP reactor when
/// `ServerOptions::create` runs. Plain `#[test]` = no ambient runtime.
/// (Review S-2 hardening, CLAUDE.md §3.5 fail-fast.)
#[test]
fn spawn_piped_outside_runtime_errors_not_panics() {
    let err = spawn_with_lowered_token_piped(
        std::path::PathBuf::from("non-existent.exe"),
        &[],
        IntegrityLevel::Low,
    )
    .expect_err("must error (not panic) when no tokio runtime is present");
    assert!(
        err.to_string().contains("tokio runtime"),
        "error must name the missing runtime; got {err}"
    );
}
