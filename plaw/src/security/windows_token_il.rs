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
    //    these as SECURITY_MANDATORY_*_RID in winnt.h:
    //    - 0x0000 SECURITY_MANDATORY_UNTRUSTED_RID
    //    - 0x1000 SECURITY_MANDATORY_LOW_RID
    //    - 0x2000 SECURITY_MANDATORY_MEDIUM_RID
    //    - 0x3000 SECURITY_MANDATORY_HIGH_RID  (elevated)
    //    - 0x4000 SECURITY_MANDATORY_SYSTEM_RID  (SYSTEM account)
    //    - 0x5000 SECURITY_MANDATORY_PROTECTED_PROCESS_RID
    //
    //    Plaw doesn't model High/System/ProtectedProcess distinctly
    //    because no lowering path uses them as a "from" level —
    //    plaw-desktop runs unelevated by design. We downgrade them
    //    to Medium with a `warn!` log so the operator sees the
    //    elevation without silently broadening capabilities
    //    (§3.5 fail-fast: a silent "we're Medium" lie when actually
    //    High would mask a real elevation footgun).
    //
    //    True unknown values (0x6000+ or anything not in winnt.h)
    //    surface as `io::Error` so the caller can decide. This was
    //    previously a silent `_ => Medium` fallback fixed in audit #11
    //    self-review M-2.
    let level = match il_value {
        0x0000 => IntegrityLevel::Untrusted,
        0x1000 => IntegrityLevel::Low,
        0x2000 => IntegrityLevel::Medium,
        0x3000 | 0x4000 | 0x5000 => {
            tracing::warn!(
                il_value = format_args!("0x{il_value:04X}"),
                "process IL is elevated (High/System/ProtectedProcess); \
                 reporting as Medium because plaw doesn't model elevated IL as a distinct variant. \
                 If you intended to run plaw elevated, verify that any \
                 `[security.sandbox.integrity]` lowering targets are correct."
            );
            IntegrityLevel::Medium
        }
        other => {
            return Err(io::Error::other(format!(
                "TOKEN_MANDATORY_LABEL returned unrecognized integrity level 0x{other:04X}; \
                 expected one of 0x0000 (Untrusted) / 0x1000 (Low) / 0x2000 (Medium) / \
                 0x3000 (High) / 0x4000 (System) / 0x5000 (ProtectedProcess) per winnt.h. \
                 If this is a new Windows IL value, plaw needs an explicit mapping here."
            )));
        }
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

// ─── Phase 1a-2 (PR #88b): token-duplication + spawn primitives ────
//
// Three new unsafe call-site classes vs Phase 1a-1's four:
//
// 5. `DuplicateTokenEx(src, TOKEN_ALL_ACCESS, NULL, SecurityImpersonation,
//     TokenPrimary, &mut dup)` — clone the current process's token into
//    a NEW handle we can mutate without affecting the caller. Returns
//    `dup` as a primary token suitable for CreateProcessAsUserW.
//    Handle MUST be `CloseHandle`-d via `OwnedHandle::Drop`.
//
// 6. `SetTokenInformation(dup, TokenIntegrityLevel, &mil, sizeof(mil))`
//    — apply the new IL to the duplicated token. Requires
//    `TOKEN_ADJUST_DEFAULT` (covered by TOKEN_ALL_ACCESS above).
//    Lowers without privilege escalation when target ≤ current
//    (validate_lowerable() enforces this pre-flight).
//
// 7. `CreateProcessAsUserW(token, lpApplicationName, lpCommandLine,
//     NULL, NULL, FALSE, dwCreationFlags, lpEnvironment,
//     lpCurrentDirectory, &si, &pi)` — spawn the child with the
//    lowered primary token. `pi.hProcess` and `pi.hThread` are new
//    handles that the caller (LoweredChild) MUST close.
//
// The same SAFETY-comment pattern from Phase 1a-1 applies: Sized POD
// structs, return-value-checked syscalls, handles closed via
// `OwnedHandle::Drop` or explicit `CloseHandle`.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitStatus;

use std::os::windows::io::{FromRawHandle, OwnedHandle};

/// Duplicate the current process's token, lower it to `target`, and
/// return the new handle. The returned `OwnedHandle` is a PRIMARY
/// token suitable for `CreateProcessAsUserW` (impersonation tokens
/// would be rejected by that syscall).
///
/// Errors when:
/// - `target == IntegrityLevel::Default` (the caller MUST resolve
///   Default before invoking — `Default` has no concrete SID).
/// - `validate_lowerable(target)` rejects the lowering.
/// - Any of the OpenProcessToken / DuplicateTokenEx /
///   SetTokenInformation Win32 calls fail (surfaced as
///   `io::Error::from_raw_os_error`).
pub fn duplicate_current_token_lowered(target: IntegrityLevel) -> io::Result<OwnedHandle> {
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::{
        DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenIntegrityLevel,
        TokenPrimary, SID_AND_ATTRIBUTES, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    if matches!(target, IntegrityLevel::Default) {
        return Err(io::Error::other(
            "duplicate_current_token_lowered: caller must resolve IntegrityLevel::Default \
             against current_process_integrity() before invoking — Default has no SID",
        ));
    }
    validate_lowerable(target)?;

    // 1. Open a duplicate-capable handle to our own process token.
    //    TOKEN_DUPLICATE is the minimum right needed for
    //    DuplicateTokenEx; TOKEN_QUERY lets us read the existing IL
    //    for diagnostic logging if needed.
    let src_raw: HANDLE = {
        let mut raw: HANDLE = INVALID_HANDLE_VALUE;
        // SAFETY: pseudo-handle GetCurrentProcess() + Sized POD raw.
        let ok = unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut raw)
        };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        raw
    };
    // SAFETY: src_raw is a fresh, unaliased handle from OpenProcessToken.
    let _src_owner = unsafe { OwnedHandle::from_raw_handle(src_raw as _) };

    // 2. DuplicateTokenEx → primary token with TOKEN_ALL_ACCESS so we
    //    can call SetTokenInformation(TokenIntegrityLevel) below.
    //    SecurityImpersonation is the safe default level; TokenPrimary
    //    makes it usable with CreateProcessAsUserW (impersonation
    //    tokens would be rejected).
    let dup_raw: HANDLE = {
        let mut raw: HANDLE = INVALID_HANDLE_VALUE;
        // SAFETY: src_raw is live; the &mut raw is a Sized POD.
        let ok = unsafe {
            DuplicateTokenEx(
                src_raw,
                TOKEN_ALL_ACCESS,
                std::ptr::null_mut(),
                SecurityImpersonation,
                TokenPrimary,
                &mut raw,
            )
        };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        raw
    };
    // SAFETY: dup_raw is a fresh, unaliased handle from DuplicateTokenEx.
    let dup_owner = unsafe { OwnedHandle::from_raw_handle(dup_raw as _) };

    // 3. Build a TOKEN_MANDATORY_LABEL pointing at a SID we construct
    //    in-place using the well-known SECURITY_MANDATORY_LABEL_AUTHORITY
    //    + the IL-specific RID. The SID lives on the stack (well,
    //    in a heap-Vec for the variable-length RID array) and survives
    //    until the SetTokenInformation call returns.
    let il_rid: u32 = match target {
        IntegrityLevel::Default => {
            unreachable!("Default rejected at function entry");
        }
        IntegrityLevel::Untrusted => 0x0000,
        IntegrityLevel::Low => 0x1000,
        IntegrityLevel::Medium => 0x2000,
    };
    let sid_bytes = build_integrity_sid(il_rid);
    let mil = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: sid_bytes.as_ptr() as *mut _,
            // SE_GROUP_INTEGRITY = 0x20 is the canonical attribute
            // for a mandatory label SID per winnt.h. windows-sys
            // does not re-export this constant under
            // `Win32::Security`, so we inline its well-known value
            // — it has been stable since Vista.
            Attributes: 0x0000_0020,
        },
    };
    let mil_size = (std::mem::size_of::<TOKEN_MANDATORY_LABEL>() + sid_bytes.len()) as u32;
    // SAFETY: dup_raw owns TOKEN_ALL_ACCESS; mil + sid_bytes live for
    // the duration of this call; sizes are computed correctly.
    let ok = unsafe {
        SetTokenInformation(
            dup_raw,
            TokenIntegrityLevel,
            &mil as *const _ as *const _,
            mil_size,
        )
    };
    if ok == 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ));
    }

    Ok(dup_owner)
}

/// Build a minimal SID byte-buffer for an integrity level RID.
///
/// SID structure layout per MS-DTYP §2.4.2:
/// - 1 byte: Revision (always 1)
/// - 1 byte: SubAuthorityCount (always 1 for an IL SID)
/// - 6 bytes: IdentifierAuthority (SECURITY_MANDATORY_LABEL_AUTHORITY = 16)
/// - 4 bytes: SubAuthority[0] = the IL RID (little-endian)
///
/// Total: 12 bytes. We hand-build the buffer rather than calling
/// `AllocateAndInitializeSid` to avoid the matching `FreeSid` lifetime
/// dance — the buffer's lifetime is statically the caller's stack
/// frame.
fn build_integrity_sid(il_rid: u32) -> Vec<u8> {
    let mut sid = Vec::with_capacity(12);
    sid.push(1); // Revision
    sid.push(1); // SubAuthorityCount
                 // IdentifierAuthority: 6 bytes, big-endian per spec.
                 // SECURITY_MANDATORY_LABEL_AUTHORITY = 16 = 0x00_00_00_00_00_10
    sid.extend_from_slice(&[0, 0, 0, 0, 0, 16]);
    // SubAuthority[0]: 4 bytes little-endian.
    sid.extend_from_slice(&il_rid.to_le_bytes());
    sid
}

/// Convert an OsStr to a null-terminated UTF-16 wide string for Win32
/// `*W` APIs. The returned `Vec<u16>` owns the buffer; the trailing
/// NUL is included so callers can pass `.as_ptr()` directly.
fn wide_z<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    let mut v: Vec<u16> = s.as_ref().encode_wide().collect();
    v.push(0);
    v
}

/// Build the `lpCommandLine` buffer for `CreateProcessAsUserW`.
///
/// Per Microsoft's command-line parsing rules (the
/// `CommandLineToArgvW` algorithm in reverse), each argument is:
/// - left bare if it contains no whitespace, no `"`, and no backslash
///   followed by `"`,
/// - wrapped in `"..."` with embedded `\` doubled before any `"` and
///   doubled at end-of-string before the closing `"`.
///
/// The first token MUST be the program name; subsequent tokens are
/// the args. We embed `program` as token 0 even when `lpApplicationName`
/// is also passed because some Win32 documentation strongly recommends
/// keeping argv[0] meaningful even though it's not used to locate the
/// binary when lpApplicationName is set.
fn build_command_line<P: AsRef<OsStr>>(program: P, args: &[String]) -> Vec<u16> {
    let mut acc = String::new();
    append_argv_token(&mut acc, &program.as_ref().to_string_lossy());
    for arg in args {
        acc.push(' ');
        append_argv_token(&mut acc, arg);
    }
    wide_z(OsStr::new(&acc))
}

fn append_argv_token(acc: &mut String, token: &str) {
    let needs_quoting =
        token.is_empty() || token.contains(|c: char| c == ' ' || c == '\t' || c == '"');
    if !needs_quoting {
        acc.push_str(token);
        return;
    }
    acc.push('"');
    let mut backslashes: usize = 0;
    for ch in token.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                // Double every preceding backslash + escape the quote.
                for _ in 0..(backslashes * 2 + 1) {
                    acc.push('\\');
                }
                acc.push('"');
                backslashes = 0;
            }
            other => {
                for _ in 0..backslashes {
                    acc.push('\\');
                }
                backslashes = 0;
                acc.push(other);
            }
        }
    }
    // Trailing backslashes must be doubled before the closing quote
    // so CommandLineToArgvW doesn't interpret them as escaping the `"`.
    for _ in 0..(backslashes * 2) {
        acc.push('\\');
    }
    acc.push('"');
}

/// A child process spawned with a lowered Token IL.
///
/// Owns both the process handle (for wait + kill) and the primary
/// thread handle (closed alongside the process on Drop). `id()`
/// returns the OS process id; `wait()` consumes self and returns
/// `ExitStatus`; `kill()` consumes self and force-terminates.
///
/// NOT a `tokio::process::Child` — `Child` has no public constructor
/// for adopting a foreign handle. Lens D flagged this as the highest-
/// risk API design; we ship a minimal newtype now and revisit tokio
/// integration in Phase 1b if any caller actually needs async waits.
#[derive(Debug)]
pub struct LoweredChild {
    /// PROCESS_INFORMATION.hProcess — used by WaitForSingleObject +
    /// GetExitCodeProcess + TerminateProcess.
    process: OwnedHandle,
    /// PROCESS_INFORMATION.hThread — held purely for RAII closure;
    /// the primary thread runs to completion as part of the process.
    _thread: OwnedHandle,
    /// PROCESS_INFORMATION.dwProcessId — stable across the child's
    /// lifetime; safe to expose even after the process exits.
    pid: u32,
}

impl LoweredChild {
    /// OS process id of the spawned child. Stable for the lifetime of
    /// the `LoweredChild` value. Matches `Child::id()` from std.
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// Block until the child exits and return its `ExitStatus`.
    /// Consumes `self` so the handles are closed deterministically
    /// once the wait completes — no leak even if the caller drops
    /// the returned status without inspecting it.
    pub fn wait(self) -> io::Result<ExitStatus> {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::ExitStatusExt;
        use windows_sys::Win32::Foundation::{GetLastError, HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, WaitForSingleObject, INFINITE,
        };

        let process_raw = self.process.as_raw_handle() as HANDLE;
        // SAFETY: process_raw is owned + alive until `self.process`
        // drops at end of scope. WaitForSingleObject is documented to
        // not modify the handle.
        let wait_result = unsafe { WaitForSingleObject(process_raw, INFINITE) };
        if wait_result != WAIT_OBJECT_0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        let mut code: u32 = 0;
        // SAFETY: process_raw is owned + alive; &mut code is a Sized POD.
        let ok = unsafe { GetExitCodeProcess(process_raw, &mut code) };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        Ok(ExitStatus::from_raw(code))
    }

    /// Force-terminate the child with exit code 1. Consumes `self`
    /// to close the handles. Useful for test cleanup if a probe
    /// hangs unexpectedly.
    #[allow(dead_code)]
    pub fn kill(self) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
        use windows_sys::Win32::System::Threading::TerminateProcess;

        let process_raw = self.process.as_raw_handle() as HANDLE;
        // SAFETY: process_raw is owned + alive.
        let ok = unsafe { TerminateProcess(process_raw, 1) };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        Ok(())
    }
}

/// Spawn `program` with the given `args` at the requested integrity
/// level. The child inherits the parent's environment + working
/// directory; future PRs can add env/cwd parameters when a caller
/// needs them (YAGNI per CLAUDE.md §3.2).
///
/// Errors when:
/// - `level == IntegrityLevel::Default` — caller must resolve Default
///   first (mirrors `duplicate_current_token_lowered`).
/// - Token duplication fails (`duplicate_current_token_lowered`).
/// - `CreateProcessAsUserW` fails (binary not found, command-line too
///   long, etc.) — surfaced as the underlying Win32 error.
pub fn spawn_with_lowered_token<P: AsRef<Path>>(
    program: P,
    args: &[String],
    level: IntegrityLevel,
) -> io::Result<LoweredChild> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Threading::{
        CreateProcessAsUserW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    if matches!(level, IntegrityLevel::Default) {
        return Err(io::Error::other(
            "spawn_with_lowered_token: caller must resolve IntegrityLevel::Default before invoking",
        ));
    }
    let token = duplicate_current_token_lowered(level)?;

    let program_wide = wide_z(program.as_ref().as_os_str());
    let mut cmd_line = build_command_line(program.as_ref().as_os_str(), args);

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // SAFETY: All pointer-typed args point to live, properly-sized
    // buffers. lpApplicationName is a null-terminated wide string;
    // lpCommandLine is a null-terminated wide string the FFI is
    // allowed to mutate (Windows docs say so — we pass an owned Vec
    // so this is fine). lpProcessAttributes / lpThreadAttributes are
    // NULL → default security descriptors. bInheritHandles = FALSE
    // because the probe communicates via a file path, not inherited
    // handles. dwCreationFlags = 0 → child inherits console, no
    // CREATE_NEW_PROCESS_GROUP. lpEnvironment = NULL → inherit
    // parent's env. lpCurrentDirectory = NULL → inherit parent's cwd.
    let ok = unsafe {
        CreateProcessAsUserW(
            token.as_raw_handle() as _,
            program_wide.as_ptr(),
            cmd_line.as_mut_ptr(),
            std::ptr::null::<SECURITY_ATTRIBUTES>() as *mut _,
            std::ptr::null::<SECURITY_ATTRIBUTES>() as *mut _,
            0, // bInheritHandles = FALSE
            0, // dwCreationFlags
            std::ptr::null_mut(),
            std::ptr::null(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ));
    }

    // SAFETY: pi.hProcess and pi.hThread are fresh, unaliased handles
    // populated by a successful CreateProcessAsUserW call.
    let process = unsafe { OwnedHandle::from_raw_handle(pi.hProcess as _) };
    let thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread as _) };

    Ok(LoweredChild {
        process,
        _thread: thread,
        pid: pi.dwProcessId,
    })
}

// Bring AsRawHandle into scope for the spawn fn body.
use std::os::windows::io::AsRawHandle;

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
        // The test runner is plaw's cargo-test binary running on a
        // normal desktop session. It MUST observe at least Low IL
        // (cargo wouldn't be able to read its own build cache
        // otherwise).
        //
        // Per audit #11 self-review M-2 (§3.5 fix): High / System /
        // ProtectedProcess parents downgrade to Medium with a
        // tracing::warn! log instead of silently lying about being
        // Medium. So an elevated test runner now observes Medium +
        // a log entry rather than a silent misreport. The `matches!`
        // below still covers it because the downgrade lands on
        // Medium.
        let observed =
            current_process_integrity().expect("OpenProcessToken on self should always succeed");
        assert!(
            matches!(observed, IntegrityLevel::Low | IntegrityLevel::Medium),
            "expected Low or Medium IL for test runner (High/System/ProtectedProcess \
             downgrade to Medium per M-2), got {observed:?}"
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

    /// Default must be resolved by the caller. Surfacing a clear
    /// error here is the regression-pin that prevents a future
    /// caller from accidentally passing Default through.
    #[test]
    fn duplicate_current_token_lowered_rejects_default() {
        let err = duplicate_current_token_lowered(IntegrityLevel::Default).unwrap_err();
        assert!(
            err.to_string().contains("Default"),
            "rejection error should name the Default sentinel; got: {err}"
        );
    }

    /// Smoke test: token duplication + IL lowering succeeds for
    /// Low (which our test runner can always lower to from Medium).
    /// The end-to-end CreateProcessAsUserW + LoweredChild test
    /// lives in `plaw/tests/windows_token_il_spawn.rs` because it
    /// needs `CARGO_BIN_EXE_plaw-il-probe` which is only set for
    /// integration tests.
    #[test]
    fn duplicate_current_token_lowered_produces_handle_for_low() {
        let handle = duplicate_current_token_lowered(IntegrityLevel::Low)
            .expect("Medium → Low lowering must succeed unelevated");
        drop(handle);
    }

    /// `spawn_with_lowered_token` MUST refuse Default at the front
    /// door so callers never pass it through to
    /// `duplicate_current_token_lowered` (which would also refuse it
    /// — defense-in-depth).
    #[test]
    fn spawn_with_lowered_token_rejects_default_level() {
        // We don't have CARGO_BIN_EXE here so use a definitely-non-
        // existent path — the Default-check fires BEFORE we ever
        // call into the spawn syscall.
        let err = spawn_with_lowered_token(
            std::path::PathBuf::from("non-existent.exe"),
            &[],
            IntegrityLevel::Default,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Default"),
            "rejection error should name the Default sentinel; got: {err}"
        );
    }

    /// Pin the command-line quoting behaviour against the
    /// `CommandLineToArgvW`-reverse algorithm. Without this test, a
    /// future refactor that "simplifies" the quoting (e.g. doesn't
    /// double backslashes before `"`) would silently break
    /// CreateProcessAsUserW for arg-rich invocations.
    #[test]
    fn build_command_line_handles_whitespace_quotes_and_backslashes() {
        let decode = |wide: &[u16]| -> String {
            let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
            String::from_utf16_lossy(&wide[..end])
        };

        // Plain ASCII, no quoting needed.
        let bare = build_command_line("plaw.exe", &["status".to_string()]);
        assert_eq!(decode(&bare), "plaw.exe status");

        // Whitespace → quoted.
        let spaced = build_command_line("plaw.exe", &["with space".to_string()]);
        assert_eq!(decode(&spaced), "plaw.exe \"with space\"");

        // Embedded quote → escaped.
        let quoted = build_command_line("plaw.exe", &["a\"b".to_string()]);
        assert_eq!(decode(&quoted), "plaw.exe \"a\\\"b\"");

        // Backslashes only escape if they precede a `"` — bare
        // backslashes inside a quoted region are NOT doubled because
        // no `"` follows them.
        let backslashed = build_command_line("plaw.exe", &["C:\\Program Files\\plaw".to_string()]);
        assert_eq!(decode(&backslashed), "plaw.exe \"C:\\Program Files\\plaw\"");
    }
}
