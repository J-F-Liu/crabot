//! Incremental output forwarding and bounded capture: [`ChunkForwarder`] for
//! live streaming plus [`wait_with_timeout`] for polling child processes.

use interprocess::unnamed_pipe;
use std::io::Read;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use super::charset::{StreamDecoder, decode_bytes};
#[cfg(unix)]
use super::exec::is_would_block;
use super::exec::{kill_process_tree, set_pipe_nonblocking};
use super::limits::tool_limits;
use super::tool::{CANCEL_REASON, COALESCE_BYTES, COALESCE_MS, OutputSink};
use crate::BoundedCapture;

/// Timeout error message used by both bash routes.
pub(crate) fn timeout_message(timeout: Duration) -> String {
    format!("Command timed out after {}ms", timeout.as_millis())
}

/// Lock `m`, polling for up to `budget`; `None` if it stays held.
pub(crate) fn try_lock_for<T>(m: &Mutex<T>, budget: Duration) -> Option<MutexGuard<'_, T>> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        match m.try_lock() {
            Ok(guard) => return Some(guard),
            // Poisoned still holds the payload — recover it like `lock`.
            Err(std::sync::TryLockError::Poisoned(p)) => return Some(p.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Which output stream a chunk came from, for per-stream capture.
#[derive(Clone, Copy)]
pub enum OutStream {
    Stdout,
    Stderr,
}

/// Forwards pipe bytes to an [`OutputSink`] as text chunks: stdout/stderr
/// merged in arrival order, `\r\n` → `\n`, small chunks coalesced (the
/// live-output cap lives in [`capping_sink`](super::capping_sink)). Per-stream
/// windows are also captured for partial-output errors, even without a sink.
pub struct ChunkForwarder {
    /// Live streaming sink; `None` when only the partial capture is needed.
    sink: Option<OutputSink>,
    decoder: StreamDecoder,
    /// Coalesced text awaiting flush.
    pending: String,
    last_flush: std::time::Instant,
    /// Captured stdout/stderr for partial-output messages.
    stdout_cap: BoundedCapture,
    stderr_cap: BoundedCapture,
}

impl ChunkForwarder {
    pub fn new(sink: Option<OutputSink>) -> Self {
        let keep = per_stream_keep();
        Self {
            sink,
            decoder: StreamDecoder::new(),
            pending: String::new(),
            last_flush: std::time::Instant::now(),
            stdout_cap: BoundedCapture::new(keep),
            stderr_cap: BoundedCapture::new(keep),
        }
    }

    /// Capture `bytes` on `stream`, then coalesce them for the sink.
    pub fn push(&mut self, stream: OutStream, bytes: &[u8]) {
        match stream {
            OutStream::Stdout => self.stdout_cap.push(bytes),
            OutStream::Stderr => self.stderr_cap.push(bytes),
        }
        let mut texts = Vec::new();
        self.decoder.feed(bytes, &mut texts);
        for text in texts {
            self.pending.push_str(&text);
        }
        self.tick();
    }

    /// Flush carried/coalesced bytes at the end of the stream.
    pub fn finish(&mut self) {
        let mut texts = Vec::new();
        self.decoder.flush(&mut texts);
        for text in texts {
            self.pending.push_str(&text);
        }
        self.flush();
    }

    /// Flush the coalescing buffer once it is large or old enough. Called
    /// after each push and periodically from idle loops, so a quiet stretch
    /// after some output still streams promptly.
    pub fn tick(&mut self) {
        if self.pending.len() >= COALESCE_BYTES
            || (!self.pending.is_empty() && self.last_flush.elapsed() >= COALESCE_MS)
        {
            self.flush();
        }
    }

    /// Append the captured stdout/stderr to `msg`, like [`kill_and_error`].
    pub fn append_partial_output(&self, msg: &mut String) {
        append_partial_output(msg, &self.stdout_cap, &self.stderr_cap);
    }

    fn flush(&mut self) {
        let Some(sink) = self.sink.as_ref() else {
            self.pending.clear(); // no sink — nothing to forward
            return;
        };
        if self.pending.is_empty() {
            return;
        }
        let s = std::mem::take(&mut self.pending);
        (sink)(&s);
        self.last_flush = std::time::Instant::now();
    }
}

/// Failure from [`wait_with_timeout`], classified so callers can map to bash
/// exit-code conventions (124 = timeout, 130 = cancel) without mislabeling
/// pipe or `wait()` I/O failures as timeouts.
pub(crate) enum WaitError {
    /// The child outlived the deadline; it was killed and reaped.
    Timeout(String),
    /// The caller's cancel token was cancelled; the child was killed and reaped.
    Cancelled(String),
    /// Pipe setup or `wait()` failure — neither a timeout nor a cancel.
    Other(String),
}

impl WaitError {
    /// The message to surface to the caller (includes any partial output).
    pub(crate) fn into_message(self) -> String {
        match self {
            WaitError::Timeout(m) | WaitError::Cancelled(m) | WaitError::Other(m) => m,
        }
    }

    /// bash exit-code convention: 124 = timeout, 130 = cancel (SIGINT), 1 = other.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            WaitError::Timeout(_) => 124,
            WaitError::Cancelled(_) => 130,
            WaitError::Other(_) => 1,
        }
    }
}

/// Wait for a child process to finish, with a hard timeout.
///
/// Pipes are drained in non-blocking polling mode (no reader threads), so a
/// surviving grandchild holding a pipe open cannot leak a thread. Output is
/// captured with a bounded head/tail window (`tool_limits().max_output_bytes`
/// total, split evenly), so runaway output (e.g. `yes`) cannot exhaust memory.
///
/// On timeout the process — and its whole tree if `kill_tree` — is killed and
/// reaped without blocking on pipe EOF. `kill_tree` must be `true` only when
/// the child was started as a process-group leader.
///
/// `remaining` is the budget actually waited; `timeout_total` is reported in
/// the timeout message (they differ when the caller already spent budget).
/// `forwarder` also gets each drained chunk for live streaming and partial
/// capture.
#[allow(clippy::too_many_arguments)] // 8 params; a context struct would churn 3 call sites
pub(crate) fn wait_with_timeout(
    mut child: std::process::Child,
    mut stdout: Option<unnamed_pipe::Recver>,
    mut stderr: Option<unnamed_pipe::Recver>,
    remaining: Duration,
    timeout_total: Duration,
    kill_tree: bool,
    cancel: &CancellationToken,
    mut forwarder: Option<&mut ChunkForwarder>,
) -> Result<std::process::Output, WaitError> {
    let pid = child.id();

    let keep = per_stream_keep();
    let mut stdout_cap = BoundedCapture::new(keep);
    let mut stderr_cap = BoundedCapture::new(keep);

    // Non-blocking pipes let the polling loop drain them without reader threads.
    // A setup failure here must not leak the already-spawned child.
    for pipe in [&stdout, &stderr].into_iter().flatten() {
        if let Err(e) = set_pipe_nonblocking(pipe) {
            return Err(WaitError::Other(kill_and_error(
                &mut child,
                pid,
                kill_tree,
                &stdout_cap,
                &stderr_cap,
                &format!("Failed to set pipe non-blocking mode: {e}"),
            )));
        }
    }

    let deadline = Instant::now() + remaining;
    let mut tmp = [0u8; 8192];

    // Drain pipes while polling, or the child blocks on a full pipe buffer.
    let status = loop {
        drain_pipe(
            stdout.as_mut(),
            &mut stdout_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stdout,
        );
        drain_pipe(
            stderr.as_mut(),
            &mut stderr_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stderr,
        );

        // Time-based flush so coalesced bytes stream while the child is quiet.
        if let Some(f) = forwarder.as_deref_mut() {
            f.tick();
        }

        // Check for user cancellation before trying the child.
        if cancel.is_cancelled() {
            return Err(WaitError::Cancelled(kill_and_error(
                &mut child,
                pid,
                kill_tree,
                &stdout_cap,
                &stderr_cap,
                CANCEL_REASON,
            )));
        }

        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Err(WaitError::Timeout(kill_and_error(
                        &mut child,
                        pid,
                        kill_tree,
                        &stdout_cap,
                        &stderr_cap,
                        &timeout_message(timeout_total),
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(WaitError::Other(kill_and_error(
                    &mut child,
                    pid,
                    kill_tree,
                    &stdout_cap,
                    &stderr_cap,
                    &format!("Failed to wait on command: {e}"),
                )));
            }
        }
    };

    // The process exited, but a grandchild may still hold the write-ends
    // open; drain for up to 2s and return whatever was collected.
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let stdout_done = drain_pipe(
            stdout.as_mut(),
            &mut stdout_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stdout,
        );
        let stderr_done = drain_pipe(
            stderr.as_mut(),
            &mut stderr_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stderr,
        );
        if (stdout_done && stderr_done) || Instant::now() >= drain_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(std::process::Output {
        status,
        stdout: stdout_cap.into_bytes(),
        stderr: stderr_cap.into_bytes(),
    })
}

/// Drain all currently-available bytes from `reader` into `cap`; `true` on
/// EOF (or no reader), `false` when there is no more data right now. Chunks
/// are also pushed to `forwarder` for live streaming and partial capture.
fn drain_pipe(
    reader: Option<&mut unnamed_pipe::Recver>,
    cap: &mut BoundedCapture,
    tmp: &mut [u8],
    forwarder: Option<&mut ChunkForwarder>,
    stream: OutStream,
) -> bool {
    let Some(reader) = reader else {
        return true;
    };
    drain_pipe_impl(reader, cap, tmp, forwarder, stream)
}

/// Append one chunk to `cap` and forward it for live streaming.
fn append_chunk(
    cap: &mut BoundedCapture,
    forwarder: &mut Option<&mut ChunkForwarder>,
    stream: OutStream,
    bytes: &[u8],
) {
    cap.push(bytes);
    if let Some(f) = forwarder.as_mut() {
        f.push(stream, bytes);
    }
}

/// Unix: the receiver is non-blocking, so read until `WouldBlock`.
#[cfg(unix)]
fn drain_pipe_impl(
    reader: &mut unnamed_pipe::Recver,
    cap: &mut BoundedCapture,
    tmp: &mut [u8],
    mut forwarder: Option<&mut ChunkForwarder>,
    stream: OutStream,
) -> bool {
    loop {
        match reader.read(tmp) {
            Ok(0) => return true, // EOF
            Ok(n) => append_chunk(cap, &mut forwarder, stream, &tmp[..n]),
            Err(ref e) if is_would_block(e) => return false,
            Err(_) => return true, // unexpected error ~ EOF
        }
    }
}

/// Windows: peek first, then read only the bytes already buffered so a
/// blocking `ReadFile` can never hang.
#[cfg(windows)]
fn drain_pipe_impl(
    reader: &mut unnamed_pipe::Recver,
    cap: &mut BoundedCapture,
    tmp: &mut [u8],
    mut forwarder: Option<&mut ChunkForwarder>,
    stream: OutStream,
) -> bool {
    use std::os::windows::io::AsRawHandle;
    loop {
        match super::exec::peek_pipe_available(reader.as_raw_handle()) {
            Some(0) => return false, // no data right now
            None => return true,     // broken pipe — EOF
            Some(_) => match reader.read(tmp) {
                Ok(0) => return true, // EOF
                Ok(n) => append_chunk(cap, &mut forwarder, stream, &tmp[..n]),
                Err(_) => return true, // unexpected error ~ EOF
            },
        }
    }
}

/// Per-stream capture window: half the output budget, rounded up.
fn per_stream_keep() -> usize {
    tool_limits().max_output_bytes.div_ceil(2)
}

/// Append partial stdout/stderr content to an error message.
fn append_partial_output(msg: &mut String, stdout: &BoundedCapture, stderr: &BoundedCapture) {
    append_stream(msg, "stdout", stdout);
    append_stream(msg, "stderr", stderr);
}

fn append_stream(msg: &mut String, name: &str, cap: &BoundedCapture) {
    if cap.total() == 0 {
        return;
    }
    msg.push_str(&format!("\n--- partial {name} ---\n"));
    msg.push_str(&decode_bytes(&cap.materialize()));
    let skipped = cap.skipped();
    if skipped > 0 {
        msg.push_str(&crate::truncation_marker(skipped, cap.total(), None));
    }
}

/// Kill and reap the child (its whole tree if `kill_tree`) and build an error
/// message with the reason and any partial output.
fn kill_and_error(
    child: &mut std::process::Child,
    pid: u32,
    kill_tree: bool,
    stdout: &BoundedCapture,
    stderr: &BoundedCapture,
    reason: &str,
) -> String {
    if kill_tree {
        kill_process_tree(pid);
    } else {
        let _ = child.kill();
    }
    let _ = child.wait();
    let mut msg = reason.to_string();
    append_partial_output(&mut msg, stdout, stderr);
    msg
}
