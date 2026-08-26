//! Child-process execution helpers: pipe plumbing, bounded stdin writes,
//! process-tree signaling, env sanitization, and exit/output formatting.

use interprocess::unnamed_pipe;
use std::io::Write;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use super::charset::decode_bytes;
use super::limits::truncate_output;
use super::tool::{STDIN_CHUNK, STDIN_POLL_INTERVAL};

/// Create an unnamed pipe pair for capturing child process output.
///
/// `label` is used in the error message (e.g. `"stdout"`, `"stderr"`).
pub(crate) fn create_pipe_pair(
    label: &str,
) -> Result<(unnamed_pipe::Sender, unnamed_pipe::Recver), String> {
    unnamed_pipe::pipe().map_err(|e| format!("Failed to create {label} pipe: {e}"))
}

/// Outcome of a bounded stdin write ([`write_stdin_bounded`]).
#[derive(Debug)]
pub(crate) enum StdinWriteError {
    /// `cancel` fired before all bytes were written.
    Cancelled,
    /// `deadline` passed before all bytes were written (pipe full).
    TimedOut,
    /// I/O failure writing or flushing (reader closed the pipe, ...).
    Io(std::io::Error),
}

/// Write `payload` to a child's stdin in [`STDIN_CHUNK`] chunks, bounded by
/// `cancel` and `deadline`; retries `WouldBlock`/`Interrupted` and flushes at
/// the end. Only a non-blocking pipe (see [`set_pipe_nonblocking`]) is truly
/// bounded — a blocking stream may block inside a single `write`.
pub(crate) fn write_stdin_bounded(
    stream: &mut impl Write,
    payload: &[u8],
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<(), StdinWriteError> {
    let mut written = 0;
    while written < payload.len() {
        if cancel.is_cancelled() {
            return Err(StdinWriteError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(StdinWriteError::TimedOut);
        }
        let end = (written + STDIN_CHUNK).min(payload.len());
        match stream.write(&payload[written..end]) {
            Ok(0) => {
                return Err(StdinWriteError::Io(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "stdin pipe closed",
                )));
            }
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if is_would_block(&e) => std::thread::sleep(STDIN_POLL_INTERVAL),
            Err(e) => return Err(StdinWriteError::Io(e)),
        }
    }
    stream.flush().map_err(StdinWriteError::Io)
}

/// A signal to deliver to a process tree (see [`signal_process_tree`]).
/// `Display` yields the schema name used by the `process` tool's `stop` action.
#[derive(Clone, Copy, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ProcessSignal {
    Terminate,
    Kill,
    Interrupt,
}

/// Send `signal` to a process and its whole descendant tree.
///
/// Unix: the child must be started with `process_group(0)`; the `kill`
/// syscall is used directly because some `kill` binaries (e.g. util-linux ≥
/// 2.42) misparse `-<pid>` and never deliver. Windows: `interrupt` has no
/// portable Ctrl+C equivalent and degrades to a graceful terminate; `kill`
/// also passes `/F`.
pub(crate) fn signal_process_tree(pid: u32, signal: ProcessSignal) {
    #[cfg(unix)]
    {
        let sig = match signal {
            ProcessSignal::Terminate => libc::SIGTERM,
            ProcessSignal::Kill => libc::SIGKILL,
            ProcessSignal::Interrupt => libc::SIGINT,
        };
        // SAFETY: sending a signal to the child's own process group is
        // exactly this helper's purpose.
        unsafe {
            libc::kill(-(pid as i32), sig);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut args = vec!["/T".to_string()];
        if matches!(signal, ProcessSignal::Kill) {
            args.push("/F".to_string());
        }
        args.extend(["/PID".to_string(), pid.to_string()]);
        let _ = std::process::Command::new("taskkill")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
}

/// Forcibly kill a process and its entire descendant tree.
pub(crate) fn kill_process_tree(pid: u32) {
    signal_process_tree(pid, ProcessSignal::Kill);
}

/// Start the child as a process-group leader (Unix) so its whole tree can be
/// killed on timeout, and suppress the console window (Windows).
pub(crate) fn detach_child(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
}

/// Whether an env var name is a secret that must not reach bash execution
/// (names ending in `API_KEY`, e.g. `OPENAI_API_KEY`).
pub(crate) fn is_secret_env_key(key: &str) -> bool {
    key.ends_with("API_KEY")
}

/// Strip secrets from a child command's inherited env: every variable whose
/// name ends in `API_KEY`, plus rustup's recursion counter which aborts
/// proxies past their max.
pub(crate) fn sanitize_child_env(cmd: &mut std::process::Command) {
    cmd.env_remove("RUST_RECURSION_COUNT");
    for key in std::env::vars().map(|(k, _)| k) {
        if is_secret_env_key(&key) {
            cmd.env_remove(&key);
        }
    }
}

/// Convert an unnamed pipe end (`Sender` or `Recver`) to `std::process::Stdio`.
#[cfg(unix)]
pub(crate) fn pipe_to_stdio<E: Into<std::os::unix::io::OwnedFd>>(end: E) -> std::process::Stdio {
    std::process::Stdio::from(end.into())
}

/// Convert an unnamed pipe end (`Sender` or `Recver`) to `std::process::Stdio`.
#[cfg(windows)]
pub(crate) fn pipe_to_stdio<E: Into<std::os::windows::io::OwnedHandle>>(
    end: E,
) -> std::process::Stdio {
    std::process::Stdio::from(end.into())
}

/// Set a pipe half to non-blocking mode.
///
/// No-op on Windows: `PIPE_NOWAIT` fails on read handles and is undefined on
/// write ends. Reads poll via [`peek_pipe_available`] instead, and a blocking
/// `WriteFile` errors once the read end closes, so the feeder cannot hang.
#[cfg(unix)]
pub(crate) fn set_pipe_nonblocking<E: interprocess::os::unix::unnamed_pipe::UnnamedPipeExt>(
    end: &E,
) -> Result<(), String> {
    end.set_nonblocking(true)
        .map_err(|e| format!("Failed to set non-blocking mode: {e}"))
}

/// Set a pipe half to non-blocking mode; no-op on Windows.
#[cfg(windows)]
pub(crate) fn set_pipe_nonblocking<E>(end: &E) -> Result<(), String> {
    let _ = end;
    Ok(())
}

/// Bytes currently buffered on a pipe read handle, or `None` when the peek
/// failed (broken pipe = EOF). Uses `PeekNamedPipe`, which needs only
/// `GENERIC_READ` — unlike `SetNamedPipeHandleState`.
#[cfg(windows)]
pub(crate) fn peek_pipe_available(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    let mut bytes_avail = 0u32;
    // SAFETY: `handle` is a live pipe read end; the out-pointers are valid.
    let ok = unsafe {
        PeekNamedPipe(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut bytes_avail,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(bytes_avail)
}

/// Prevent a pipe sender's handle from being inherited by spawned children.
///
/// On both platforms a child holding the write end of its own stdin pipe would
/// never see EOF, so we explicitly mark the sender end non-inheritable.
pub(crate) fn set_sender_noninheritable(sender: &unnamed_pipe::Sender) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = sender.as_raw_fd();
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(format!(
                "Failed to set FD_CLOEXEC: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        // SAFETY: `sender`'s handle is live; clears its inherit flag.
        let ok = unsafe { SetHandleInformation(sender.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) };
        if ok == 0 {
            return Err(format!(
                "Failed to clear handle inheritance: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

/// Map an `ExitStatus` to a bash-style exit code (`128 + signal` for signal
/// death). Falls back to -1 when neither is available.
pub(crate) fn exit_code_of(status: &std::process::ExitStatus) -> i32 {
    exit_code_of_impl(status, false)
}

/// Like [`exit_code_of`], but for MSYS/Cygwin statuses also decodes signal
/// death encoded as `code == signal << 8` (SIGTERM → 3840 → 143). This form
/// collides with native exit codes that are multiples of 256, so it must only
/// be used for output of a real MSYS/Cygwin `bash`, never native commands.
fn exit_code_of_impl(status: &std::process::ExitStatus, msys: bool) -> i32 {
    if let Some(code) = status.code() {
        // `code()` is ≤ 255 on Unix, so the `sig << 8` form can't occur there.
        if msys {
            let sig = code >> 8;
            if (1..=64).contains(&sig) && sig << 8 == code {
                return 128 + sig;
            }
        }
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    -1
}

/// Combine stdout, stderr, and exit code into one string, then truncate.
/// Output is decoded with charset detection (see [`decode_bytes`]); pass
/// `msys = true` only for output of a real MSYS/Cygwin `bash`.
pub(crate) fn format_command_output(output: &std::process::Output, msys: bool) -> String {
    let stdout = decode_bytes(&output.stdout);
    let stderr = decode_bytes(&output.stderr);
    truncate_output(combine_output(
        &stdout,
        &stderr,
        exit_code_of_impl(&output.status, msys),
    ))
}

/// Combine stdout, `STDERR:`-prefixed stderr, and a non-zero exit code into
/// one untruncated string. Shared by the real-bash and bashkit paths.
pub(crate) fn combine_output(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("STDERR:\n");
        result.push_str(stderr);
    }
    if exit_code != 0 {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("Exit code: {exit_code}"));
    }
    result
}

/// True when `e` means "no data available right now" (`WouldBlock`).
pub(crate) fn is_would_block(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::WouldBlock
}
