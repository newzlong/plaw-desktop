//! Windows Token Integrity Level (IL) observation primitives.
//!
//! This module is the FOUNDATION of audit #11 Phase 1 — kernel-enforced
//! write isolation for plaw's subprocess tools beyond the Job Object
//! resource caps shipped in PR #77. The Phase 0 Job Object kills
//! processes and caps their resources, but does not prevent a child from
//! writing to `%USERPROFILE%` or `%APPDATA%`. Token IL on a child
//! process produces a kernel-enforced write deny on objects above the
//! child's mandatory label.
//!
//! # Phase split (per [[plaw-dormant-subsystem-pattern]] discipline)
//!
//! - **Phase 1a-1 (this PR #88)** — **observation primitives only**.
//!   The four well-known integrity-level SID strings, query the current
//!   process's token, decide whether a child can be lowered to a target
//!   level. ZERO callers in production code. Strictly dormant per
//!   [[plaw-dormant-subsystem-pattern]] so a bad merge cannot regress
//!   existing users.
//!
//! - **Phase 1a-2 (PR #88b, future)** — token-duplication +
//!   `CreateProcessAsUserW` spawn primitives + `LoweredChild` newtype +
//!   `plaw-il-probe` workspace-member test binary. The synthesis Lens D
//!   flagged the `LoweredChild` stdio-pipe API as the highest-risk
//!   design decision; spiking it AFTER this PR lands lets observation
//!   tests pin the SID round-trip independently.
//!
//! - **Phase 1b (PR #89, future)** — extend the `Sandbox` trait with
//!   `spawn_with_integrity(cmd, level) -> SandboxedChild` and wire
//!   `WindowsJobObjectSandbox` to delegate into the spawn primitives
//!   from Phase 1a-2.
//!
//! - **Phase 1c (PR #90, future)** — `SandboxIntegrityConfig` schema
//!   + opt-in wiring on `ShellTool`. **DEFAULT OFF** per Lens C — a
//!   default-on workspace-wide Low IL would break first-run
//!   `cargo build` / `npm install` and users would disable sandboxing
//!   entirely (the Gatekeeper failure mode).
//!
//! # `#![allow(unsafe_code)]` justification (Phase 1a-1 surface)
//!
//! The crate root carries `#![deny(unsafe_code)]`. This module overrides
//! with `#![allow(unsafe_code)]` because the four observation
//! primitives below MUST call out to Win32 syscalls that have no
//! safe-Rust replacement in the standard library:
//!
//! 1. `OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle)` —
//!    open a query-only handle to our own process token. Cannot fail
//!    for our own process when TOKEN_QUERY is the requested right (we
//!    own the process); errors only on OS-level resource exhaustion.
//!    Handle MUST be `CloseHandle`-d via `OwnedHandle::Drop`.
//!
//! 2. `GetTokenInformation(TokenIntegrityLevel, &mut info, ...)` —
//!    read the `TOKEN_MANDATORY_LABEL` from the token. Two-call
//!    pattern: first call with `null` info pointer returns the
//!    required buffer size; second call fills the buffer. The
//!    returned `TOKEN_MANDATORY_LABEL.Label.Sid` points INTO the
//!    buffer we just allocated — must keep the buffer alive while
//!    reading the SID.
//!
//! 3. `GetSidSubAuthorityCount(sid)` + `GetSidSubAuthority(sid, idx)`
//!    — extract the last sub-authority (the actual IL value, e.g.
//!    `0x2000` for Medium). Both functions return raw pointers into
//!    the caller-owned SID; deref is safe iff the SID is well-formed
//!    (the kernel guarantees this for SIDs it returned to us).
//!
//! 4. `GetCurrentProcess()` — pseudo-handle constant (-1 cast to
//!    HANDLE). Doesn't actually allocate; returns the same value
//!    every time. NOT closed.
//!
//! Each unsafe block in this module:
//! - Operates on Sized POD structs the FFI populates via raw pointer.
//! - Checks the return value before reading any out-parameter.
//! - Closes handles deterministically via `OwnedHandle::Drop` or
//!   explicit `CloseHandle`.
//! - Has a `// SAFETY:` comment naming the invariant the call relies
//!   on.

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use std::io;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Mandatory Integrity Level of a Windows process token.
///
/// The kernel's mandatory label policy denies writes to objects above
/// the writer's level by default. Lowering a child process to `Low`
/// prevents it from writing to anything labeled `Medium` (which is
/// almost the entire filesystem for interactive users) without an
/// explicit ACL grant.
///
/// `Default` is a sentinel for "do not lower" — when serialized into
/// config, it represents the caller saying "use whatever the parent
/// process has". It is NOT a valid lookup key for
/// [`Self::sid_string`] because the parent's IL is queried at runtime
/// via [`current_process_integrity`].
///
/// The numeric values match the well-known IL constants Microsoft
/// publishes (e.g. SECURITY_MANDATORY_LOW_RID = 0x1000) but we don't
/// expose them as a `u32` — callers should match on the variant and
/// only the FFI bridge reaches for the raw value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IntegrityLevel {
    /// Use the parent process's IL — no lowering. Resolved to a
    /// concrete level at spawn time via [`current_process_integrity`].
    Default,
    /// Medium (`0x2000`). The default IL for processes run by an
    /// interactive user on Windows. Can write to user profile,
    /// `%APPDATA%`, `%TEMP%`, and most filesystem locations.
    Medium,
    /// Low (`0x1000`). The IL Chromium renderers run at. Can write to
    /// `%LOCALAPPDATA%\Low`, `%TEMP%\Low`, and explicit Low-IL ACLs;
    /// CANNOT write to user profile, `%APPDATA%`, or most filesystem
    /// without explicit grant.
    Low,
    /// Untrusted (`0x0000`). The most restrictive IL. Can write only
    /// to Untrusted-IL ACLs. Most applications break at this level —
    /// included for completeness so Phase 1c config can express the
    /// most-restrictive opt-in profile.
    Untrusted,
}

impl IntegrityLevel {
    /// String form of the well-known SID for this integrity level.
    ///
    /// Locale-invariant (English / German / Chinese Windows all see
    /// the same SID). Used by tests to assert child-process IL
    /// without parsing the localized "Mandatory Label\\Low Mandatory
    /// Level" string that `whoami /groups` prints.
    ///
    /// Returns `None` for [`Self::Default`] because there is no SID
    /// for "use the parent's IL" — callers MUST resolve `Default`
    /// against [`current_process_integrity`] before formatting.
    pub fn sid_string(&self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Medium => Some("S-1-16-8192"),
            Self::Low => Some("S-1-16-4096"),
            Self::Untrusted => Some("S-1-16-0"),
        }
    }

    /// Ordering by restrictiveness — higher values are MORE permissive.
    /// `Untrusted` < `Low` < `Medium`. `Default` panics because it is
    /// not orderable until resolved against the parent's IL.
    fn rank(&self) -> u32 {
        match self {
            Self::Untrusted => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::Default => {
                unreachable!("IntegrityLevel::Default must be resolved before ranking")
            }
        }
    }

    /// `true` when `target` is at-or-below `self` (i.e. lowering to
    /// `target` from a process running at `self` is permitted without
    /// `SeImpersonatePrivilege` or other elevation). Backs
    /// [`validate_lowerable`].
    fn permits_lowering_to(&self, target: Self) -> bool {
        target.rank() <= self.rank()
    }
}

/// Query the integrity level of the **current** plaw process.
///
/// The two-call `GetTokenInformation` pattern handles the case where
/// the `TOKEN_MANDATORY_LABEL` buffer size varies between Windows
/// versions. The first call (NULL output buffer) returns the required
/// size in `cb_needed`; we allocate exactly that many bytes; the
/// second call fills them.
///
/// Returns an `io::Error` rather than `IntegrityLevel::Default` on
/// failure so callers can surface a useful diagnostic — silently
/// defaulting to "Medium" would mask a real OS-level problem (e.g.
/// running inside an AppContainer with stripped TOKEN_QUERY rights).
pub fn current_process_integrity() -> io::Result<IntegrityLevel> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // 1. Open a query-only handle to our own process token. This
    //    cannot fail for TOKEN_QUERY on the current process under
    //    normal conditions; an error implies OS-level resource
    //    exhaustion or a stripped token (e.g. AppContainer).
    let token_handle: HANDLE = {
        let mut raw: HANDLE = INVALID_HANDLE_VALUE;
        // SAFETY: GetCurrentProcess returns a pseudo-handle constant
        // (-1) that never needs closing; OpenProcessToken writes the
        // duplicated handle into `raw` on success. We immediately
        // wrap it in OwnedHandle below to ensure CloseHandle on Drop.
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        raw
    };
    // Adopt the raw HANDLE into RAII so CloseHandle fires on Drop
    // even if subsequent calls fail.
    //
    // SAFETY: `token_handle` was just produced by a successful
    // OpenProcessToken call; we have not aliased it.
    let _token = unsafe { OwnedHandle::from_raw_handle(token_handle as _) };

    // 2. First GetTokenInformation call: NULL output buffer →
    //    Windows writes the required buffer size into `cb_needed`
    //    and returns FALSE with GetLastError() == ERROR_INSUFFICIENT_BUFFER.
    let mut cb_needed: u32 = 0;
    // SAFETY: passing a NULL info pointer + length 0 is the documented
    // sizing call. cb_needed is a Sized POD we hold a &mut to.
    unsafe {
        GetTokenInformation(
            token_handle,
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut cb_needed,
        );
    }
    if cb_needed == 0 {
        return Err(io::Error::other(
            "GetTokenInformation sizing call returned zero buffer size",
        ));
    }

    // 3. Allocate exactly cb_needed bytes and re-call. The buffer
    //    contains a TOKEN_MANDATORY_LABEL struct followed by SID
    //    sub-authorities — we MUST keep the Vec alive while reading
    //    the SID because the struct's `Label.Sid` field points
    //    INTO this buffer.
    let mut buf: Vec<u8> = vec![0; cb_needed as usize];
    // SAFETY: buf.as_mut_ptr() is well-aligned for u8 (alignment 1);
    // cb_needed bytes are writable. The FFI populates the buffer with
    // a TOKEN_MANDATORY_LABEL header at offset 0.
    let ok = unsafe {
        GetTokenInformation(
            token_handle,
            TokenIntegrityLevel,
            buf.as_mut_ptr().cast(),
            cb_needed,
            &mut cb_needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ));
    }

    // 4. Extract the IL from the SID's last sub-authority.
    //    TOKEN_MANDATORY_LABEL.Label.Sid points to a SID structure
    //    whose last sub-authority IS the integrity level value
    //    (SECURITY_MANDATORY_LOW_RID = 0x1000, MEDIUM = 0x2000, ...).
    //
    // SAFETY: buf is at least size_of::<TOKEN_MANDATORY_LABEL> by the
    // first call's contract. Casting buf to TOKEN_MANDATORY_LABEL is
    // aligned (the struct has u64 alignment, our buffer is heap-
    // allocated by Vec → aligned to max_align_t per allocator
    // contract; on Windows the system allocator returns 16-byte
    // aligned blocks for sizes >= 16).
    let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    let sid = label.Label.Sid;
    if sid.is_null() {
        return Err(io::Error::other("TOKEN_MANDATORY_LABEL.Label.Sid was null"));
    }
    // SAFETY: GetSidSubAuthorityCount returns a *mut u8 pointing into
    // the SID structure the kernel just gave us; the kernel
    // guarantees the SID is well-formed.
    let sub_count: u8 = unsafe { *GetSidSubAuthorityCount(sid) };
    if sub_count == 0 {
        return Err(io::Error::other("SID has zero sub-authorities"));
    }
    // SAFETY: GetSidSubAuthority(sid, n) returns a *mut u32 into the
    // SID's sub-authority array; valid for index 0..sub_count.
    let il_value: u32 = unsafe { *GetSidSubAuthority(sid, (sub_count - 1) as u32) };

    // 5. Map raw IL value → IntegrityLevel variant. Microsoft defines
    //    these as SECURITY_MANDATORY_*_RID in winnt.h. Values that
    //    don't match any well-known constant are treated as
    //    "unrecognized — treat as Medium" because mapping to a
    //    more-restrictive level would surface as a privilege error
    //    that isn't the real issue.
    let level = match il_value {
        0x0000 => IntegrityLevel::Untrusted,
        0x1000 => IntegrityLevel::Low,
        0x2000 => IntegrityLevel::Medium,
        _ => IntegrityLevel::Medium,
    };
    Ok(level)
}

/// Decide whether the current process can lower a child to `target`.
///
/// Lowering one's own token requires NO special privilege when target
/// ≤ current_il. Plaw-desktop runs at Medium IL unelevated, so
/// lowering to Low or Untrusted is always permitted. Lowering to
/// Medium when already at Medium is a no-op (returns `Ok(())`).
/// Lowering to Default short-circuits to `Ok(())` because Default
/// means "no lowering" (resolved at spawn time).
///
/// Returns a detailed `io::Error` when the target would RAISE
/// instead of lower the IL — a real OS-level rejection mode that
/// would otherwise surface as `ERROR_PRIVILEGE_NOT_HELD` deep
/// inside `SetTokenInformation` and require operator-level Win32
/// debugging.
pub fn validate_lowerable(target: IntegrityLevel) -> io::Result<()> {
    if matches!(target, IntegrityLevel::Default) {
        return Ok(());
    }
    let current = current_process_integrity()?;
    // Current must be at-or-above target for the lowering to be
    // permitted without privilege escalation.
    if !current.permits_lowering_to(target) {
        return Err(io::Error::other(format!(
            "cannot raise child token integrity from {current:?} to {target:?}: \
             lowering to a higher level would require SeImpersonatePrivilege; \
             plaw-desktop runs unelevated and refuses to attempt it"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sid_string_for_well_known_levels_matches_microsoft_constants() {
        // These SIDs are documented under MS-DTYP §2.4.2.4 and have
        // remained stable since Vista. Asserting on the string form
        // gives us a locale-invariant regression that survives
        // Windows version bumps + every UI-locale variant.
        assert_eq!(IntegrityLevel::Medium.sid_string(), Some("S-1-16-8192"));
        assert_eq!(IntegrityLevel::Low.sid_string(), Some("S-1-16-4096"));
        assert_eq!(IntegrityLevel::Untrusted.sid_string(), Some("S-1-16-0"));
    }

    #[test]
    fn sid_string_for_default_is_none() {
        // Default is a sentinel meaning "use the parent process's IL".
        // It has no well-known SID — resolving it requires runtime
        // inspection via current_process_integrity().
        assert_eq!(IntegrityLevel::Default.sid_string(), None);
    }

    #[test]
    fn current_process_integrity_returns_at_least_low() {
        // The test runner is plaw's cargo-test binary running
        // unelevated on a normal desktop session. It MUST observe
        // at least Low IL (cargo wouldn't be able to read its own
        // build cache otherwise). We don't pin to exactly Medium
        // because CI runners + AppContainer + UAC-elevated dev
        // shells produce different baselines.
        let observed =
            current_process_integrity().expect("OpenProcessToken on self should always succeed");
        assert!(
            matches!(observed, IntegrityLevel::Low | IntegrityLevel::Medium),
            "expected Low or Medium IL for test runner, got {observed:?}"
        );
    }

    #[test]
    fn validate_lowerable_accepts_default_unconditionally() {
        // Default == "use parent's IL" → always Ok(()) without
        // even probing the parent. Cheap fast path for the common
        // config (`level = "default"` in TOML).
        validate_lowerable(IntegrityLevel::Default).unwrap();
    }

    #[test]
    fn validate_lowerable_accepts_lowering_to_untrusted() {
        // Plaw-desktop runs at Medium → can ALWAYS lower to
        // Untrusted (the most restrictive). This is the canonical
        // "lower to maximum isolation" smoke test.
        validate_lowerable(IntegrityLevel::Untrusted).unwrap();
    }

    #[test]
    fn validate_lowerable_accepts_lowering_to_low() {
        validate_lowerable(IntegrityLevel::Low).unwrap();
    }

    #[test]
    fn validate_lowerable_accepts_same_level_medium() {
        // Lowering to your own level is a no-op (SetTokenInformation
        // succeeds with the same SID), so validate_lowerable also
        // says Ok rather than over-rejecting.
        let current = current_process_integrity().unwrap();
        if matches!(current, IntegrityLevel::Medium) {
            validate_lowerable(IntegrityLevel::Medium).unwrap();
        }
    }

    #[test]
    fn integrity_level_rank_orders_by_restrictiveness() {
        // Smaller rank = more restrictive. The trait method backs
        // permits_lowering_to which backs validate_lowerable; pinning
        // the order keeps a future refactor from accidentally
        // inverting it.
        assert!(IntegrityLevel::Untrusted.rank() < IntegrityLevel::Low.rank());
        assert!(IntegrityLevel::Low.rank() < IntegrityLevel::Medium.rank());
    }

    #[test]
    #[should_panic(expected = "IntegrityLevel::Default must be resolved before ranking")]
    fn integrity_level_default_rank_panics() {
        // Default is the "use parent's IL" sentinel and has no
        // numeric ordering until resolved. Calling rank() on it is a
        // programming bug — pin the panic so a future caller that
        // tries to skip the resolution step fails loudly in tests
        // instead of silently producing weird ordering.
        let _ = IntegrityLevel::Default.rank();
    }

    #[test]
    fn serde_roundtrip_uses_lowercase_variants() {
        // Lowercase JSON keys are the wire-format contract for
        // future `[security.sandbox.integrity]` config keys
        // (Phase 1c). Pinning the round-trip + the specific lowercase
        // forms prevents accidental camelCase / PascalCase drift in
        // a future serde-rename refactor.
        for (level, key) in [
            (IntegrityLevel::Default, "default"),
            (IntegrityLevel::Medium, "medium"),
            (IntegrityLevel::Low, "low"),
            (IntegrityLevel::Untrusted, "untrusted"),
        ] {
            let serialized = serde_json::to_string(&level).unwrap();
            assert_eq!(serialized, format!("\"{key}\""));
            let parsed: IntegrityLevel = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, level);
        }
    }
}
