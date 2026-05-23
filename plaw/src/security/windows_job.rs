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
//! 2. **Sealed process tree foundation.** A process inside a job cannot
//!    `CREATE_BREAKAWAY_FROM_JOB` unless `JOB_OBJECT_LIMIT_BREAKAWAY_OK`
//!    was set on the job. We do not set it, so spawned tools cannot
//!    escape the container. Future resource limits (memory cap, CPU cap,
//!    UI restrictions) attach to this same job via [`win32job`] without
//!    touching the spawn path.
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
//! tools, not adversarial code.
//!
//! # The `unsafe` exception
//!
//! The crate root carries `#![deny(unsafe_code)]`. This module overrides
//! that with `#![allow(unsafe_code)]` because `win32job::Job::assign_process`
//! takes a Win32 `HANDLE`, but `std::process::Child::id()` only gives us
//! a `u32` PID. The conversion requires `OpenProcess`, which is `unsafe
//! fn` in every Rust Win32 binding (the safety contract is "the returned
//! handle is valid only if the call succeeded"). The unsafe surface here
//! is exactly one call, scoped tightly, and the returned handle is
//! checked for null before use and dropped immediately after assignment.
//!
//! All higher-level Job Object operations (create, configure limits,
//! assign, drop) go through the safe [`win32job`] crate.
//!
//! # Platform gating
//!
//! Entire module is `#[cfg(target_os = "windows")]`. The factory in
//! [`crate::security::detect::create_sandbox`] handles cross-platform
//! dispatch and never references this module on non-Windows builds.

#![cfg(target_os = "windows")]
#![allow(unsafe_code)] // justified above — single OpenProcess call in `pid_to_handle`

use crate::security::traits::Sandbox;
use std::io;
use std::process::Command;
use win32job::{ExtendedLimitInfo, Job};

/// Job-Object-backed sandbox. One job per plaw process; every shell child
/// is assigned to it via [`Sandbox::after_spawn`].
pub struct WindowsJobObjectSandbox {
    /// The job handle. `win32job::Job` is `Send + Sync` (its handle is
    /// a raw HANDLE but the underlying kernel object is thread-safe,
    /// and assignment is the only mutation we perform).
    job: Job,
}

impl WindowsJobObjectSandbox {
    /// Create a new Job Object configured with `KILL_ON_JOB_CLOSE`.
    ///
    /// # Errors
    ///
    /// Returns `io::Error` when `CreateJobObjectW` or the limit-info
    /// configuration fails. Rare in practice — typically only on
    /// locked-down AppContainer sessions where `ERROR_ACCESS_DENIED`
    /// is returned.
    pub fn new() -> io::Result<Self> {
        let mut limit_info = ExtendedLimitInfo::new();
        limit_info.limit_kill_on_job_close();

        let job = Job::create_with_limit_info(&mut limit_info)
            .map_err(|e| io::Error::other(format!("CreateJobObject failed: {e}")))?;

        tracing::info!(
            "Windows Job Object sandbox active (KILL_ON_JOB_CLOSE — \
             child processes auto-terminate on plaw exit)"
        );

        Ok(Self { job })
    }

    /// Probe constructor — same behavior as `new` today; named distinctly
    /// so the factory's auto-detect path matches the convention used by
    /// other backends (Landlock/Firejail/Docker all have a `probe()`
    /// returning `Result`).
    pub fn probe() -> io::Result<Self> {
        Self::new()
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
///
/// # Safety
///
/// `handle` must be a valid value returned by `open_process_handle`
/// that has not already been closed.
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

impl Sandbox for WindowsJobObjectSandbox {
    /// Pre-spawn step: no-op. We assign the running process to the job in
    /// [`after_spawn`] rather than spawning suspended; see module docs for
    /// the rationale.
    fn wrap_command(&self, _cmd: &mut Command) -> io::Result<()> {
        Ok(())
    }

    /// Post-spawn step: open a handle to the freshly-spawned PID with
    /// the minimum rights required by `AssignProcessToJobObject`, assign
    /// it to our job, and close the transient handle. The job keeps its
    /// own internal reference for kill-on-close.
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
        "Windows Job Object (KILL_ON_JOB_CLOSE — child processes auto-terminate on plaw exit)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_succeeds_and_reports_available() {
        let sandbox = WindowsJobObjectSandbox::new().expect("CreateJobObject should succeed");
        assert!(sandbox.is_available());
        assert_eq!(sandbox.name(), "windows-job-object");
        assert!(sandbox.description().contains("KILL_ON_JOB_CLOSE"));
    }

    #[test]
    fn wrap_command_is_noop() {
        let sandbox = WindowsJobObjectSandbox::new().unwrap();
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg("exit 0");
        let program_before = cmd.get_program().to_string_lossy().to_string();
        sandbox.wrap_command(&mut cmd).unwrap();
        assert_eq!(cmd.get_program().to_string_lossy(), program_before);
    }

    #[test]
    fn after_spawn_assigns_real_child_to_job() {
        let sandbox = WindowsJobObjectSandbox::new().unwrap();

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

        // Wait for the child rather than orphaning it.
        let _ = child.wait();
    }

    #[test]
    fn after_spawn_on_dead_pid_errors_but_does_not_panic() {
        let sandbox = WindowsJobObjectSandbox::new().unwrap();
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
            let sandbox = WindowsJobObjectSandbox::new().unwrap();
            sandbox.after_spawn(pid).unwrap();
            // sandbox drops here → job handle closes → KILL_ON_JOB_CLOSE fires
        }

        // Give Windows a moment to terminate the child.
        std::thread::sleep(std::time::Duration::from_millis(500));

        // try_wait returns Ok(Some(_)) once the child has been reaped;
        // ping -n 10 would still be running if it hadn't been killed.
        let status = child.try_wait().expect("try_wait must succeed");
        assert!(
            status.is_some(),
            "child must have been terminated by KILL_ON_JOB_CLOSE"
        );
    }
}
