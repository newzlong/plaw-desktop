//! Windows Job Object sandbox backend.
//!
//! Wraps every shell-spawned child process in a kernel-level Job Object so
//! that:
//!
//! 1. **Auto-cleanup on plaw exit.** With `KILL_ON_JOB_CLOSE` set, when
//!    the job handle is dropped (plaw exits or this struct is dropped),
//!    every process assigned to the job is terminated by the kernel.
//!    No orphan tool processes survive a plaw crash.
//!
//! 2. **DoS containment (PR #77).** Four kernel-enforced limits raise the
//!    attack cost from "trivial fork bomb / memory balloon" to "must
//!    escape a kernel Job Object":
//!
//!    - `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` — caps the number of live
//!      processes inside the job; further `CreateProcess` calls inside
//!      the job fail with `ERROR_NOT_ENOUGH_QUOTA`. Default 256.
//!    - `JOB_OBJECT_LIMIT_PROCESS_MEMORY` — per-process commit-charge
//!      cap; the kernel terminates an over-allocating child immediately.
//!      Default 2 GiB.
//!    - `JOB_OBJECT_LIMIT_PROCESS_TIME` — per-process **cumulative CPU
//!      TIME** (not wall-clock); the kernel terminates a runaway tight
//!      loop at exactly the configured duration. Default 600 s.
//!    - `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION` — suppresses the
//!      WerFault popup that would otherwise leave a crashed tool zombied
//!      waiting on a user dialog.
//!
//!    Plus basic UI restrictions (HANDLES + SYSTEMPARAMETERS) so a
//!    sandboxed tool cannot harvest handles from the desktop or mutate
//!    `SystemParametersInfo`.
//!
//! 3. **Sealed process tree foundation.** A process inside a job cannot
//!    `CREATE_BREAKAWAY_FROM_JOB` unless `JOB_OBJECT_LIMIT_BREAKAWAY_OK`
//!    was set on the job. We never set it, and a regression test asserts
//!    the `LimitFlags` bit stays clear so a future refactor can't
//!    accidentally enable escape.
//!
//! # Why post-spawn assignment, not `CREATE_SUSPENDED`
//!
//! The textbook Job-Object pattern is: `CreateProcess(CREATE_SUSPENDED)`
//! → `AssignProcessToJobObject` → `ResumeThread`. That requires the main
//! thread handle from `PROCESS_INFORMATION.hThread`, which Rust's
//! `std::process::Command` does not expose (it closes the thread handle
//! immediately after `CreateProcess`).
//!
//! Instead we assign already-running children via the PID exposed by
//! `Child::id()`. The window between `CreateProcess` and assignment is
//! microseconds and the child cannot meaningfully escape in that gap
//! without already being malicious — plaw's threat model is misbehaving
//! tools, not adversarial code with arbitrary code execution.
//!
//! # The `unsafe` exception
//!
//! The crate root carries `#![deny(unsafe_code)]`. This module overrides
//! that with `#![allow(unsafe_code)]` for three tightly-scoped Win32
//! calls:
//!
//! 1. `OpenProcess` in [`open_process_handle`] — needed to convert
//!    `Child::id()` (u32 PID) into the `HANDLE` that
//!    `AssignProcessToJobObject` requires. Return value is checked for
//!    null and the handle is closed inside [`assign_pid_to_job`].
//! 2. `SetInformationJobObject(JobObjectExtendedLimitInformation)` in
//!    [`apply_extended_limits`] — applies the four kernel-enforced
//!    limits in one syscall. `win32job 2.0`'s `ExtendedLimitInfo.0` is
//!    `pub(crate)` so we cannot mutate the underlying struct via the
//!    safe API; we hand-roll the `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`
//!    struct in safe Rust and pass it to the syscall by raw pointer.
//! 3. `SetInformationJobObject(JobObjectBasicUIRestrictions)` in
//!    [`apply_ui_restrictions`] — `win32job 2.0` has no UI-restriction
//!    setter, so a parallel hand-rolled call.
//!
//! All three calls take a `HANDLE` returned by a safe constructor (or by
//! `OpenProcess` after the null check) and pass `Sized` structs built in
//! safe Rust. There are no raw pointers held across yield points and no
//! aliasing concerns.
//!
//! All higher-level Job Object operations (create, configure
//! `limit_kill_on_job_close`, assign, drop) continue to go through the
//! safe [`win32job`] crate.
//!
//! # Platform gating
//!
//! Entire module is `#[cfg(target_os = "windows")]`. The factory in
//! [`crate::security::detect::create_sandbox`] handles cross-platform
//! dispatch and never references this module on non-Windows builds.

#![cfg(target_os = "windows")]
#![allow(unsafe_code)] // justified above — exactly 3 unsafe call sites

use crate::config::SandboxConfig;
use crate::security::traits::Sandbox;
use std::io;
use std::process::Command;
use win32job::{ExtendedLimitInfo, Job};

/// User-configurable kernel-enforced limits applied to every child of a
/// [`WindowsJobObjectSandbox`].
///
/// Constructed via [`Self::from_config`] (reads optional `windows_*`
/// fields off [`SandboxConfig`] with conservative dev-agent defaults) or
/// directly in tests via the struct literal.
///
/// Unit choice: everything is stored in the kernel's native unit (bytes
/// for memory, 100-ns ticks for CPU time) so the helpers can pass the
/// values to `SetInformationJobObject` without further conversion.
#[derive(Debug, Clone, Copy)]
pub struct WindowsJobLimits {
    /// `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` — caps live processes in the job.
    pub max_processes: u32,
    /// `JOB_OBJECT_LIMIT_PROCESS_MEMORY` — per-process commit cap, in bytes.
    pub process_memory_bytes: u64,
    /// `JOB_OBJECT_LIMIT_PROCESS_TIME` — per-process cumulative CPU TIME,
    /// in 100-ns ticks (the unit `JOBOBJECT_BASIC_LIMIT_INFORMATION`
    /// expects for `PerProcessUserTimeLimit.QuadPart`).
    pub process_cpu_time_100ns: u64,
}

impl WindowsJobLimits {
    /// Conservative-for-dev-agent built-in defaults applied when a user
    /// config does not override them. Tuned so that legitimate
    /// `cargo build` / `npm install` / `pip install` workloads keep
    /// working out of the box; users hardening against untrusted code
    /// should lower all three.
    pub const fn built_in_defaults() -> Self {
        Self {
            max_processes: 256,
            process_memory_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
            process_cpu_time_100ns: 600 * 10_000_000,     // 600 seconds in 100-ns ticks
        }
    }

    /// Read overrides from a [`SandboxConfig`] block, falling back to
    /// [`Self::built_in_defaults`] for any field the user left absent.
    pub fn from_config(cfg: &SandboxConfig) -> Self {
        let defaults = Self::built_in_defaults();
        Self {
            max_processes: cfg.windows_max_processes.unwrap_or(defaults.max_processes),
            process_memory_bytes: cfg
                .windows_process_memory_mb
                .map(|mb| mb.saturating_mul(1024 * 1024))
                .unwrap_or(defaults.process_memory_bytes),
            process_cpu_time_100ns: cfg
                .windows_process_cpu_time_secs
                .map(|s| s.saturating_mul(10_000_000))
                .unwrap_or(defaults.process_cpu_time_100ns),
        }
    }
}

/// Job-Object-backed sandbox. One job per plaw process; every shell child
/// is assigned to it via [`Sandbox::after_spawn`].
pub struct WindowsJobObjectSandbox {
    /// The job handle. `win32job::Job` is `Send + Sync` (its handle is a
    /// raw HANDLE but the underlying kernel object is thread-safe, and
    /// assignment is the only mutation we perform).
    job: Job,
}

impl WindowsJobObjectSandbox {
    /// Create a new Job Object configured with `KILL_ON_JOB_CLOSE` plus
    /// the four PR-#77 kernel-enforced limits and the basic UI
    /// restrictions.
    ///
    /// # Errors
    ///
    /// Returns `io::Error` when `CreateJobObjectW` fails. The four
    /// extended-limit settings and the UI restrictions FAIL LOUD BUT
    /// CONTINUE — if a particular limit class is rejected by the host OS
    /// (e.g. an older Windows build) we log a `tracing::error!` naming
    /// the limit + the OS error code, then continue with the remainder
    /// applied. Rationale: a partial-but-applied limit set is materially
    /// safer than refusing to start at all, and rejection of one class
    /// (commonly UI restrictions on hardened corporate builds) should
    /// not block plaw entirely.
    ///
    /// CreateJob itself is fatal because no Job Object semantics apply
    /// at all without a job handle — falling back to NoopSandbox is the
    /// factory's job (see `detect.rs`).
    pub fn new(limits: WindowsJobLimits) -> io::Result<Self> {
        // Step 1: create the job with the safe win32job API.
        // We keep KILL_ON_JOB_CLOSE via the safe helper so the existing
        // drop-kills-children contract is unchanged.
        let mut limit_info = ExtendedLimitInfo::new();
        limit_info.limit_kill_on_job_close();
        let job = Job::create_with_limit_info(&mut limit_info)
            .map_err(|e| io::Error::other(format!("CreateJobObject failed: {e}")))?;

        // Step 2: apply the four PR-#77 extended limits via a direct
        // SetInformationJobObject call (win32job 2.0's
        // ExtendedLimitInfo.0 is pub(crate) so we cannot mutate it
        // further through the safe API). Fail loud but continue.
        let mut applied: Vec<&'static str> = vec!["KILL_ON_JOB_CLOSE"];
        match apply_extended_limits(&job, &limits) {
            Ok(()) => applied.extend([
                "DIE_ON_UNHANDLED_EXCEPTION",
                "ACTIVE_PROCESS",
                "PROCESS_MEMORY",
                "PROCESS_TIME",
            ]),
            Err(e) => tracing::error!(
                error = %e,
                "Windows Job Object: extended limits NOT applied — \
                 SetInformationJobObject(JobObjectExtendedLimitInformation) failed. \
                 KILL_ON_JOB_CLOSE remains active; resource caps are absent."
            ),
        }

        // Step 3: apply basic UI restrictions (HANDLES + SYSTEMPARAMETERS)
        // — a parallel SetInformationJobObject call because win32job 2.0
        // has no UI-restriction setter. Fail loud but continue.
        match apply_ui_restrictions(&job) {
            Ok(()) => applied.push("UI_HANDLES+SYSTEMPARAMETERS"),
            Err(e) => tracing::error!(
                error = %e,
                "Windows Job Object: UI restrictions NOT applied — \
                 SetInformationJobObject(JobObjectBasicUIRestrictions) failed. \
                 Hardened corporate Windows builds sometimes reject this."
            ),
        }

        tracing::info!(
            limits = ?applied,
            "Windows Job Object sandbox active"
        );

        Ok(Self { job })
    }

    /// Probe constructor — used by the auto-detect arm. Identical to
    /// `new(WindowsJobLimits::built_in_defaults())`; named distinctly so
    /// the factory's `probe()` convention (shared with Landlock /
    /// Firejail / Docker) matches.
    pub fn probe() -> io::Result<Self> {
        Self::new(WindowsJobLimits::built_in_defaults())
    }
}

/// Open a process by PID with the minimum rights `AssignProcessToJobObject`
/// requires: `PROCESS_SET_QUOTA | PROCESS_TERMINATE`. Returns the raw HANDLE
/// (cast to `isize` to keep the safety scope inside this function —
/// callers see only a plain integer).
///
/// # Safety
///
/// `OpenProcess` is `unsafe` only because the returned handle's validity
/// depends on the call succeeding. We check the return value for null
/// and `Result`-wrap. The caller MUST call `CloseHandle` on the returned
/// value via [`close_handle`] when done, or leak the handle. We close
/// it inside [`assign_pid_to_job`] before returning.
fn open_process_handle(pid: u32) -> io::Result<isize> {
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    // SAFETY: OpenProcess accepts any u32 PID and returns null on failure
    // (e.g. PID does not exist, insufficient privileges, restricted
    // session). We check for null before returning Ok.
    let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(handle as isize)
}

/// Close a HANDLE returned by [`open_process_handle`].
fn close_handle(handle: isize) {
    use windows_sys::Win32::Foundation::CloseHandle;
    // SAFETY: handle was returned by OpenProcess (non-null at construction)
    // and is closed exactly once by this single helper.
    unsafe {
        CloseHandle(handle as _);
    }
}

/// Open the process by PID, assign it to `job`, and close the handle.
/// Wraps the unsafe block surface so the `Sandbox` impl stays clean.
fn assign_pid_to_job(job: &Job, pid: u32) -> io::Result<()> {
    let handle = open_process_handle(pid)?;
    let assign_result = job
        .assign_process(handle as _)
        .map_err(|e| io::Error::other(format!("AssignProcessToJobObject failed: {e}")));
    close_handle(handle);
    assign_result
}

/// Apply the four kernel-enforced extended limits in a single
/// `SetInformationJobObject(JobObjectExtendedLimitInformation)` call.
///
/// LimitFlags assembled here:
/// - `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — retained (win32job already set
///   it, but the safe API does not let us read+merge LimitFlags so we
///   re-state every bit we want; the kernel does not mind redundant bits).
/// - `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION` — prevent WerFault
///   popup on tool crash.
/// - `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` — fork-bomb cap.
/// - `JOB_OBJECT_LIMIT_PROCESS_MEMORY` — per-process commit cap.
/// - `JOB_OBJECT_LIMIT_PROCESS_TIME` — per-process cumulative CPU time cap.
///
/// Explicitly NOT set: `JOB_OBJECT_LIMIT_BREAKAWAY_OK` and
/// `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` — a regression test pins these
/// off so a future refactor cannot accidentally enable child escape.
fn apply_extended_limits(job: &Job, limits: &WindowsJobLimits) -> io::Result<()> {
    use windows_sys::Win32::System::JobObjects::{
        JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOB_OBJECT_LIMIT_PROCESS_TIME,
    };

    // Build the struct in safe Rust. `..unsafe { std::mem::zeroed() }`
    // is the idiomatic way to zero-init the unused fields of a C-style
    // POD struct; the bytes are well-defined (all zero) and the struct
    // has no pointers or references inside.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation = JOBOBJECT_BASIC_LIMIT_INFORMATION {
        LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_PROCESS_TIME,
        ActiveProcessLimit: limits.max_processes,
        // `PerProcessUserTimeLimit` is a LARGE_INTEGER (i64) inside a C
        // union; windows-sys exposes it as i64. Saturating-cast the u64
        // (so a pathological 2^63+ config value clamps cleanly).
        PerProcessUserTimeLimit: i64::try_from(limits.process_cpu_time_100ns).unwrap_or(i64::MAX),
        ..unsafe { std::mem::zeroed() }
    };
    info.ProcessMemoryLimit = limits.process_memory_bytes as usize;

    // SAFETY: `job.handle()` returns a valid HANDLE from a successful
    // CreateJobObjectW. `info` is a Sized, fully-initialized POD with
    // no pointers; we pass it by `&info as *const _ as *const c_void`
    // along with its size. Win32 reads the bytes and validates against
    // the info class; on bad input the syscall returns 0 and we surface
    // the OS error via `GetLastError` (wrapped by `io::Error::last_os_error`).
    let ok = unsafe {
        SetInformationJobObject(
            job.handle() as _,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Apply basic UI restrictions: deny inheriting handles from the
/// initiating thread's desktop, and deny `SystemParametersInfo` mutation.
///
/// DESKTOP, EXITWINDOWS, DISPLAYSETTINGS, GLOBALATOMS, READCLIPBOARD,
/// WRITECLIPBOARD restrictions are intentionally NOT set in Phase 0 to
/// avoid compat risk with legitimate tools (notably the browser tool's
/// chromium spawn). Phase 0.5 may revisit.
fn apply_ui_restrictions(job: &Job) -> io::Result<()> {
    use windows_sys::Win32::System::JobObjects::{
        JobObjectBasicUIRestrictions, SetInformationJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS,
        JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
    };

    let info = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
    };

    // SAFETY: same contract as apply_extended_limits — Sized POD, valid
    // handle, size matches the info class. Errors surface via last_os_error.
    let ok = unsafe {
        SetInformationJobObject(
            job.handle() as _,
            JobObjectBasicUIRestrictions,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl Sandbox for WindowsJobObjectSandbox {
    /// Pre-spawn step: no-op. We assign the running process to the job in
    /// [`after_spawn`] rather than spawning suspended; see module docs for
    /// the rationale.
    fn wrap_command(&self, _cmd: &mut Command) -> io::Result<()> {
        Ok(())
    }

    /// Post-spawn step: open a handle to the freshly-spawned PID with
    /// the minimum rights required by `AssignProcessToJobObject`, assign
    /// it to our job, and close the transient handle. The job's
    /// extended limits + UI restrictions automatically apply to the
    /// newly-assigned process.
    fn after_spawn(&self, pid: u32) -> io::Result<()> {
        assign_pid_to_job(&self.job, pid)
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "windows-job-object"
    }

    fn description(&self) -> &str {
        "Windows Job Object (KILL_ON_JOB_CLOSE + process-memory cap + \
         active-process cap + CPU-time cap + UI restrictions; \
         no FS/network/token-IL isolation in Phase 0)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> WindowsJobLimits {
        WindowsJobLimits::built_in_defaults()
    }

    // ── Lifecycle (PR #63 baseline — unchanged behavior) ──────────────

    #[test]
    fn new_succeeds_and_reports_available() {
        let sandbox =
            WindowsJobObjectSandbox::new(defaults()).expect("CreateJobObject should succeed");
        assert!(sandbox.is_available());
        assert_eq!(sandbox.name(), "windows-job-object");
        assert!(sandbox.description().contains("KILL_ON_JOB_CLOSE"));
        // PR #77 honest-labels: surface the new caps explicitly.
        assert!(sandbox.description().contains("process-memory cap"));
        assert!(sandbox.description().contains("CPU-time cap"));
        assert!(sandbox.description().contains("UI restrictions"));
        // And honest about what's still NOT covered.
        assert!(sandbox.description().contains("no FS/network/token-IL"));
    }

    #[test]
    fn wrap_command_is_noop() {
        let sandbox = WindowsJobObjectSandbox::new(defaults()).unwrap();
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg("exit 0");
        let program_before = cmd.get_program().to_string_lossy().to_string();
        sandbox.wrap_command(&mut cmd).unwrap();
        assert_eq!(cmd.get_program().to_string_lossy(), program_before);
    }

    #[test]
    fn after_spawn_assigns_real_child_to_job() {
        let sandbox = WindowsJobObjectSandbox::new(defaults()).unwrap();

        // Spawn a long-enough child that we have time to inspect it.
        let mut child = Command::new("cmd.exe")
            .arg("/C")
            .arg("ping -n 2 127.0.0.1 >NUL")
            .spawn()
            .expect("spawn cmd.exe should succeed");
        let pid = child.id();

        sandbox
            .after_spawn(pid)
            .expect("assign should succeed on a live child");

        let _ = child.wait();
    }

    #[test]
    fn after_spawn_on_dead_pid_errors_but_does_not_panic() {
        let sandbox = WindowsJobObjectSandbox::new(defaults()).unwrap();
        // PID 0 (Idle process) cannot be opened with our requested rights;
        // OpenProcess returns null → we return an io::Error rather than
        // panicking.
        let err = sandbox
            .after_spawn(0)
            .expect_err("PID 0 must not be openable for these rights");
        assert!(err.raw_os_error().is_some(), "expected an OS error");
    }

    #[test]
    fn drop_terminates_assigned_children() {
        // End-to-end proof of the KILL_ON_JOB_CLOSE contract: spawn a
        // child that would otherwise live for ~10 seconds, assign it,
        // drop the sandbox, then verify the child has been killed.
        let mut child = Command::new("cmd.exe")
            .arg("/C")
            .arg("ping -n 10 127.0.0.1 >NUL")
            .spawn()
            .expect("spawn cmd.exe should succeed");
        let pid = child.id();

        {
            let sandbox = WindowsJobObjectSandbox::new(defaults()).unwrap();
            sandbox.after_spawn(pid).unwrap();
            // sandbox drops here → job handle closes → KILL_ON_JOB_CLOSE fires
        }

        // Give Windows a moment to terminate the child.
        std::thread::sleep(std::time::Duration::from_millis(500));

        let status = child.try_wait().expect("try_wait must succeed");
        assert!(
            status.is_some(),
            "child must have been terminated by KILL_ON_JOB_CLOSE"
        );
    }

    // ── PR #77 hardening additions ───────────────────────────────────

    #[test]
    fn from_config_uses_defaults_when_fields_absent() {
        let cfg = SandboxConfig::default();
        let limits = WindowsJobLimits::from_config(&cfg);
        let d = WindowsJobLimits::built_in_defaults();
        assert_eq!(limits.max_processes, d.max_processes);
        assert_eq!(limits.process_memory_bytes, d.process_memory_bytes);
        assert_eq!(limits.process_cpu_time_100ns, d.process_cpu_time_100ns);
    }

    #[test]
    fn from_config_applies_overrides_when_present() {
        let cfg = SandboxConfig {
            windows_max_processes: Some(8),
            windows_process_memory_mb: Some(128),
            windows_process_cpu_time_secs: Some(5),
            ..SandboxConfig::default()
        };
        let limits = WindowsJobLimits::from_config(&cfg);
        assert_eq!(limits.max_processes, 8);
        assert_eq!(limits.process_memory_bytes, 128 * 1024 * 1024);
        assert_eq!(limits.process_cpu_time_100ns, 5 * 10_000_000);
    }

    #[test]
    fn from_config_saturates_pathological_inputs() {
        // u64::MAX MB would overflow u64::MAX bytes — saturating_mul keeps
        // us in-range so the kernel sees a sane (very large) value.
        let cfg = SandboxConfig {
            windows_process_memory_mb: Some(u64::MAX),
            windows_process_cpu_time_secs: Some(u64::MAX),
            ..SandboxConfig::default()
        };
        let limits = WindowsJobLimits::from_config(&cfg);
        assert_eq!(limits.process_memory_bytes, u64::MAX);
        assert_eq!(limits.process_cpu_time_100ns, u64::MAX);
    }

    use windows_sys::Win32::System::JobObjects::{
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    };

    /// Test-only helper — read back the job's extended limit info via
    /// the raw `QueryInformationJobObject` syscall. `win32job 2.0`'s
    /// `ExtendedLimitInfo.0` is `pub(crate)` so we cannot inspect the
    /// underlying struct through its safe accessor; this helper keeps
    /// the test's unsafe surface scoped to the test module.
    fn query_limits(job: &Job) -> JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: job.handle() is a valid HANDLE from a successful
        // CreateJobObjectW. `info` is a Sized POD; we pass its size and
        // a raw pointer to the kernel which fills it with the current
        // limit set. No aliasing concerns; pointer not held past the call.
        let ok = unsafe {
            QueryInformationJobObject(
                job.handle() as _,
                JobObjectExtendedLimitInformation,
                std::ptr::from_mut::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(&mut info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "QueryInformationJobObject must succeed");
        info
    }

    #[test]
    fn no_breakaway_flags_set() {
        // Regression test against accidental future enablement of process
        // escape. Both BREAKAWAY_OK and SILENT_BREAKAWAY_OK bits MUST stay
        // clear in the LimitFlags the kernel reports.
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        };
        let sandbox = WindowsJobObjectSandbox::new(defaults()).unwrap();
        let info = query_limits(&sandbox.job);
        let flags = info.BasicLimitInformation.LimitFlags;
        assert_eq!(
            flags & JOB_OBJECT_LIMIT_BREAKAWAY_OK,
            0,
            "BREAKAWAY_OK must stay clear"
        );
        assert_eq!(
            flags & JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
            0,
            "SILENT_BREAKAWAY_OK must stay clear"
        );
    }

    #[test]
    fn extended_limits_query_back_correctly() {
        // Smoke test that the PR-#77 limits actually landed in the kernel
        // and round-trip through QueryInformationJobObject. Uses tiny test
        // values so we can assert exact equality rather than approximate.
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            JOB_OBJECT_LIMIT_PROCESS_TIME,
        };
        let limits = WindowsJobLimits {
            max_processes: 12,
            process_memory_bytes: 256 * 1024 * 1024,
            process_cpu_time_100ns: 30 * 10_000_000,
        };
        let sandbox = WindowsJobObjectSandbox::new(limits).unwrap();
        let info = query_limits(&sandbox.job);
        let basic = info.BasicLimitInformation;
        let flags = basic.LimitFlags;
        for bit in [
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            JOB_OBJECT_LIMIT_PROCESS_TIME,
        ] {
            assert_ne!(flags & bit, 0, "LimitFlags missing bit {bit:#x}");
        }
        assert_eq!(basic.ActiveProcessLimit, 12);
        assert_eq!(basic.PerProcessUserTimeLimit, 30 * 10_000_000);
        assert_eq!(info.ProcessMemoryLimit, 256 * 1024 * 1024);
    }

    /// CPU-burn test — marked `#[ignore]` because it deliberately spends
    /// real CPU time. Run via `cargo test -- --ignored`.
    ///
    /// Spawns a tight PowerShell loop with a 2-second per-process CPU
    /// cap and asserts the child gets killed inside an 8-second wall
    /// budget.
    #[test]
    #[ignore = "spends real CPU; run with --ignored"]
    fn cpu_time_cap_kills_burn_loop() {
        let limits = WindowsJobLimits {
            process_cpu_time_100ns: 2 * 10_000_000, // 2 s
            ..defaults()
        };
        let sandbox = WindowsJobObjectSandbox::new(limits).unwrap();
        let mut child = Command::new("powershell")
            .args(["-NoProfile", "-Command", "while($true){}"])
            .spawn()
            .expect("spawn powershell should succeed");
        sandbox.after_spawn(child.id()).unwrap();

        // Wait up to 8 s — kernel should kill it well before then.
        let start = std::time::Instant::now();
        let killed = loop {
            if let Some(_status) = child.try_wait().unwrap() {
                break true;
            }
            if start.elapsed() > std::time::Duration::from_secs(8) {
                let _ = child.kill();
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        assert!(
            killed,
            "child must have been killed by JOB_OBJECT_LIMIT_PROCESS_TIME"
        );
    }

    /// Memory-balloon test — marked `#[ignore]` because the kernel
    /// enforces `JOB_OBJECT_LIMIT_PROCESS_MEMORY` on COMMITTED pages,
    /// not on allocation requests, and PowerShell's `[byte[]]::new(...)`
    /// reserves virtual address space without necessarily committing the
    /// full backing store right away. Reliable manual verification is
    /// possible by touching every page (which forces commit) but the
    /// resulting test is slow and load-sensitive enough that we keep it
    /// out of CI. The CPU-time test below is the more reliable
    /// kernel-enforcement proof. Run manually via
    /// `cargo test -- --ignored process_memory_cap_kills_balloon`.
    #[test]
    #[ignore = "JOB_OBJECT_LIMIT_PROCESS_MEMORY enforced on commit not reserve; manual verification only"]
    fn process_memory_cap_kills_balloon() {
        let limits = WindowsJobLimits {
            process_memory_bytes: 64 * 1024 * 1024, // 64 MiB
            ..defaults()
        };
        let sandbox = WindowsJobObjectSandbox::new(limits).unwrap();
        // Touch every byte so the kernel actually commits the pages and
        // the limit triggers. Without the write loop, .NET reserves the
        // VAD entry without committing and the cap may not fire.
        let mut child = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "$x = [byte[]]::new(256MB); for ($i = 0; $i -lt $x.Length; $i += 4096) { $x[$i] = 1 }; Start-Sleep 5",
            ])
            .spawn()
            .expect("spawn powershell should succeed");
        sandbox.after_spawn(child.id()).unwrap();

        let start = std::time::Instant::now();
        let killed = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break !status.success();
            }
            if start.elapsed() > std::time::Duration::from_secs(10) {
                let _ = child.kill();
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        assert!(
            killed,
            "child must have been killed by JOB_OBJECT_LIMIT_PROCESS_MEMORY"
        );
    }
}
