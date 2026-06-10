//! Print the invoking process's Token Integrity Level SID to a file
//! path passed as `args[1]`.
//!
//! Used by `plaw::security::windows_token_il`'s integration tests to
//! validate that a child process spawned via the Phase 1a-2 spawn
//! primitives actually runs at the requested IL. Output is the
//! locale-invariant well-known SID string (`S-1-16-XXXX`) — NOT the
//! localized "Mandatory Label\Low Mandatory Level" string that
//! `whoami /groups` prints, because that varies by Windows UI locale.
//!
//! On non-Windows, this is a 1-line stub so the workspace stays
//! buildable cross-platform.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("plaw-il-probe is Windows-only");
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
fn main() {
    use std::io::Write;

    let args: Vec<String> = std::env::args().collect();
    let Some(output_path) = args.get(1) else {
        eprintln!("usage: plaw-il-probe <output-path | --stdout>");
        std::process::exit(2);
    };

    let sid = match query_current_il_sid() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("plaw-il-probe: failed to query current IL: {e}");
            std::process::exit(3);
        }
    };

    // `--stdout` mode: write the SID to stdout instead of a file. Used
    // by the Phase 1c.2a piped-spawn tests, which spawn the probe
    // DIRECTLY (no cmd.exe wrapper) at a lowered IL and read the SID
    // back through the captured stdout pipe — proving the lowered token
    // and the IOCP pipe capture work together. Spawning the probe
    // directly avoids the cmd→grandchild handle-propagation quirk.
    if output_path == "--stdout" {
        println!("{sid}");
        std::process::exit(0);
    }

    match std::fs::File::create(output_path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{sid}") {
                eprintln!("plaw-il-probe: failed to write output file: {e}");
                std::process::exit(4);
            }
        }
        Err(e) => {
            // Permission-denied here is the EXPECTED outcome when the
            // probe was spawned at Low/Untrusted IL and the test
            // pointed it at a Medium-IL output path. Exit with a
            // distinct code so the test can DISTINGUISH this case from
            // a syscall failure (exit 3) or arg-missing (exit 2).
            // Exit 5 → "ran but couldn't write" — the test reads exit
            // code to confirm IL-based filesystem deny worked.
            eprintln!("plaw-il-probe: failed to open output file: {e}");
            std::process::exit(5);
        }
    }
}

/// Minimal duplicate of `plaw::security::windows_token_il::current_process_integrity()`
/// — we cannot depend on plaw here because plaw depends on us (cyclic
/// workspace dependency would fail). Keep the implementation byte-
/// identical in spirit so the probe + the consumer agree on the
/// SID format.
#[cfg(target_os = "windows")]
fn query_current_il_sid() -> std::io::Result<String> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let token_handle: HANDLE = {
        let mut raw: HANDLE = INVALID_HANDLE_VALUE;
        // SAFETY: GetCurrentProcess is a pseudo-handle constant; raw
        // is a Sized POD held by &mut. CloseHandle fires via OwnedHandle
        // Drop below.
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) };
        if ok == 0 {
            return Err(std::io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        raw
    };
    // SAFETY: token_handle is a fresh, unaliased handle.
    let _token_owner = unsafe { OwnedHandle::from_raw_handle(token_handle as _) };

    let mut cb_needed: u32 = 0;
    // SAFETY: documented MSDN sizing call (null buffer + len 0).
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
        return Err(std::io::Error::other(
            "zero-size buffer from GetTokenInformation",
        ));
    }
    let mut buf: Vec<u8> = vec![0; cb_needed as usize];
    // SAFETY: buf.as_mut_ptr() is u8-aligned (alignment 1); buf has
    // cb_needed writable bytes.
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
        return Err(std::io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ));
    }
    // SAFETY: GetTokenInformation populated the buffer with a valid
    // TOKEN_MANDATORY_LABEL; Vec<u8> heap alignment is ≥ 16 on
    // Windows so the cast is well-aligned for u64.
    let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    let sid = label.Label.Sid;
    if sid.is_null() {
        return Err(std::io::Error::other("null SID"));
    }
    // SAFETY: GetSidSubAuthorityCount returns a pointer into a SID
    // we know is well-formed (kernel-provided via GetTokenInformation).
    let sub_count: u8 = unsafe { *GetSidSubAuthorityCount(sid) };
    if sub_count == 0 {
        return Err(std::io::Error::other("zero sub-authorities"));
    }
    // SAFETY: index is in [0, sub_count); GetSidSubAuthority returns a
    // pointer into the SID's sub-authority array.
    let il_value: u32 = unsafe { *GetSidSubAuthority(sid, (sub_count - 1) as u32) };
    // SECURITY_MANDATORY_LABEL_AUTHORITY is the well-known authority
    // value 16. The SID format is fixed: S-1-16-<il_value>.
    Ok(format!("S-1-16-{il_value}"))
}
