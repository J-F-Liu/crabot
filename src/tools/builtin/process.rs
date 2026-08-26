//! `process` tool: manages long-running processes across tool calls.
//!
//! Spawns like the `bash` tool's host-command bridge (shell-words split,
//! direct exec, host env minus secrets plus `env` overrides) and keeps the
//! children in an app-global registry keyed by their OS pid.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};

use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use shell_words::split;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, RawHandle};

use crate::lock;
use crate::tools::{
    CANCEL_REASON, OutputSink, ProcessSignal, StdinWriteError, StreamDecoder, Tool, arg_str,
    arg_u64, detach_child, exit_code_of, resolve_path, sanitize_child_env, signal_process_tree,
    tool_limits, write_stdin_bounded,
};

/// Cap on retained exited process entries; the oldest are dropped on `start`.
const MAX_RETAINED_EXITED: usize = 64;
/// How long `stop` waits for a signaled process to be reaped before reporting.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// Poll interval for `wait`/`logs --follow`/`stop`.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Unix reader poll timeout: lets the loop notice an exit even when EOF never arrives.
#[cfg(unix)]
const READER_IDLE_POLL_MS: i32 = 100;
/// Windows reader idle sleep between poll attempts.
#[cfg(windows)]
const READER_IDLE_SLEEP: Duration = Duration::from_millis(100);
/// Drain window after an exit is first noticed: readers keep pulling final
/// output for this long and then stop, even if a daemonised grandchild still
/// holds the write end open (output written later is dropped).
const DRAIN_GRACE: Duration = Duration::from_millis(250);
/// How long non-follow `logs` waits for a fresh process's exit to be recorded.
const SETTLE_GRACE_MS: u64 = 250;
/// Timeout for `input` when the child stops reading (pipe full); cancel aborts sooner.
const INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Default and maximum number of log lines returned by `logs`.
const DEFAULT_LINES: u64 = 100;
const MAX_LINES: u64 = 2000;

// ── Global process registry ────────────────────────────────────────

/// App-global registry of agent-managed processes, keyed by OS pid.
/// Shared across session tabs, like the MCP connection cache.
static PROCESSES: LazyLock<Mutex<HashMap<u32, Arc<ProcessEntry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Pinged on every process start/exit; the UI refreshes its cached process
/// list once per tick instead of querying the registry on every frame.
static PROCESS_EVENTS: LazyLock<broadcast::Sender<()>> = LazyLock::new(|| broadcast::channel(1).0);

/// Ping the change channel. Best-effort: a tick only triggers a registry re-read.
fn notify_process_changed() {
    let _ = PROCESS_EVENTS.send(());
}

pub struct ProcessTool;

impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "process"
    }

    fn description(&self) -> &str {
        "Manage long-running processes started by the agent, including starting, monitoring, interacting with, and stopping processes."
    }

    fn instruction(&self) -> &str {
        "Use the process tool to start, monitor, interact with, and stop long-running processes such as servers, watchers, and REPLs. `start` returns the operating system `pid`; pass it to every later operation. Prefer this over the `bash` tool for anything that does not exit on its own, and remember to stop processes you no longer need."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "start",
                        "list",
                        "status",
                        "logs",
                        "input",
                        "wait",
                        "stop",
                        "restart"
                    ],
                    "description": "The process operation to perform."
                },
                "pid": {
                    "type": "integer",
                    "description": "The operating system process id returned by start. Required for operations on an existing process."
                },
                "command": {
                    "type": "string",
                    "description": "Command to start. Only required for start action."
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the process. Relative to workspace if not absolute."
                },
                "env": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "string"
                    },
                    "description": "Additional environment variables as name-value pairs; a JSON-encoded object string in strict mode."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Maximum time to wait in milliseconds. Used by the wait action and by logs with follow."
                },
                "input": {
                    "type": "string",
                    "description": "Text sent to the process stdin. Used by input action."
                },
                "lines": {
                    "type": "integer",
                    "description": "Number of recent log lines to return."
                },
                "follow": {
                    "type": "boolean",
                    "description": "Whether to continue streaming logs until timeout or process exit."
                },
                "signal": {
                    "type": "string",
                    "enum": [
                        "terminate",
                        "kill",
                        "interrupt"
                    ],
                    "description": "Termination method used by stop action."
                }
            },
            "required": [
                "action"
            ]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        run(args, workspace, cancel, None)
    }

    fn execute_streaming_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
        sink: &OutputSink,
    ) -> Result<String, String> {
        run(args, workspace, cancel, Some(Arc::clone(sink)))
    }
}

// ── Entry points ───────────────────────────────────────────────────

fn run(
    args: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
    sink: Option<OutputSink>,
) -> Result<String, String> {
    let action = arg_str(args, "action").ok_or("Missing 'action' argument")?;
    match action {
        "start" => start(args, workspace),
        "list" => list(),
        "status" => status(args),
        "logs" => logs(args, cancel, sink),
        "input" => input(args, cancel),
        "wait" => wait_action(args, cancel, sink),
        "stop" => stop(args),
        "restart" => restart(args, workspace),
        other => Err(format!("Unknown action '{other}'")),
    }
}

fn start(args: &Value, workspace: &Path) -> Result<String, String> {
    let command = arg_str(args, "command").ok_or("Missing 'command' argument for start")?;
    let cwd = match arg_str(args, "cwd") {
        Some(cwd) => {
            resolve_path(cwd, workspace).map_err(|e| format!("Invalid cwd '{cwd}': {e}"))?
        }
        None => workspace.to_path_buf(),
    };
    let env = args
        .get("env")
        .map(parse_env)
        .transpose()?
        .unwrap_or_default();
    let parts = parse_command(command)?;
    // Tag the entry with the calling session tab (None in the tool playground).
    let entry = start_command(command, parts, cwd, env, crate::tools::current_tab_number())?;
    Ok(format!(
        "Started process {}: {}\ncwd: {}",
        entry.pid,
        entry.command,
        entry.cwd.display()
    ))
}

/// Snapshot of one running managed process, for the right-pane UI.
pub struct RunningProcess {
    pub pid: u32,
    /// Owning session tab number, or `None` outside the LLM loop (tool playground).
    pub tab: Option<usize>,
    pub command: String,
}

/// Currently running managed processes, ordered by start time.
pub fn running_processes() -> Vec<RunningProcess> {
    let procs = lock(&PROCESSES);
    let mut entries: Vec<&Arc<ProcessEntry>> = procs.values().filter(|e| !e.has_exited()).collect();
    entries.sort_by_key(|e| e.started_at);
    entries
        .into_iter()
        .map(|e| RunningProcess {
            pid: e.pid,
            tab: e.tab,
            command: e.command.clone(),
        })
        .collect()
}

/// Stream yielding one tick on subscribe plus one tick per registry change,
/// so a subscriber is always consistent with the registry even if it missed a
/// ping while unsubscribed. Lagged pings coalesce since each tick only
/// re-reads the registry. Safe to subscribe permanently.
pub fn events() -> BoxStream<'static, ()> {
    let changes = futures::stream::unfold(PROCESS_EVENTS.subscribe(), |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(()) => return Some(((), rx)),
                // Coalesce lagged pings instead of ending the stream.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    futures::stream::iter([()]).chain(changes).boxed()
}

fn list() -> Result<String, String> {
    let procs = lock(&PROCESSES);
    if procs.is_empty() {
        return Ok("No managed processes.".into());
    }
    let mut entries: Vec<&Arc<ProcessEntry>> = procs.values().collect();
    entries.sort_by_key(|e| e.started_at);
    let mut lines = Vec::new();
    for e in entries {
        let (state, detail) = describe_status(e);
        let detail = detail.map(|d| format!(" ({d})")).unwrap_or_default();
        lines.push(format!("{}: {} — {}{}", e.pid, state, e.command, detail));
    }
    Ok(lines.join("\n"))
}

fn status(args: &Value) -> Result<String, String> {
    let e = entry_for(args)?;
    let (state, detail) = describe_status(&e);
    let detail = detail.map(|d| format!("\ndetail: {d}")).unwrap_or_default();
    Ok(format!(
        "pid: {}\nstatus: {}\ncommand: {}\ncwd: {}\nlog bytes: {}{}",
        e.pid,
        state,
        e.command,
        e.cwd.display(),
        e.logs.len(),
        detail
    ))
}

fn logs(
    args: &Value,
    cancel: &CancellationToken,
    sink: Option<OutputSink>,
) -> Result<String, String> {
    let e = entry_for(args)?;
    let lines = arg_u64(args, "lines")
        .unwrap_or(DEFAULT_LINES)
        .clamp(1, MAX_LINES) as usize;
    let follow = args.get("follow").and_then(Value::as_bool).unwrap_or(false);
    if !follow {
        // Give a fresh, just-exited process a moment to settle so the tail is complete.
        if !e.is_done() && e.started_at.elapsed() < Duration::from_millis(SETTLE_GRACE_MS * 2) {
            let _ = wait_for_exit(&e, false, SETTLE_GRACE_MS, cancel, None);
        }
        // Once the exit is recorded, let the readers drain the final bytes.
        if e.has_exited() {
            wait_for_drain(&e);
        }
        return Ok(e.logs.tail(lines));
    }
    follow_logs(
        &e,
        lines,
        arg_u64(args, "timeout").unwrap_or(0),
        cancel,
        sink.as_ref(),
    )
}

fn input(args: &Value, cancel: &CancellationToken) -> Result<String, String> {
    let e = entry_for(args)?;
    let text = arg_str(args, "input").ok_or("Missing 'input' argument")?;

    if e.has_exited() {
        return Err(not_accepting_input(&e, "it has exited or been stopped"));
    }
    if e.writing.swap(true, Ordering::SeqCst) {
        return Err(not_accepting_input(
            &e,
            "an earlier input is still being written",
        ));
    }
    let Some(mut stdin) = lock(&e.stdin).take() else {
        e.writing.store(false, Ordering::SeqCst);
        if e.stdin_abandoned.load(Ordering::SeqCst) {
            return Err(not_accepting_input(
                &e,
                "its stdin was closed because an earlier input was cancelled",
            ));
        }
        return Err(not_accepting_input(&e, "it has exited or been stopped"));
    };

    // Non-blocking stdin (Unix) so `write_stdin_bounded` polls instead of
    // hanging on a full pipe; on Windows the abandoned-write path covers it.
    if let Err(err) = crate::tools::set_raw_fd_nonblocking(&stdin) {
        e.writing.store(false, Ordering::SeqCst);
        *lock(&e.stdin) = Some(stdin);
        return Err(format!(
            "Failed to prepare stdin of process {}: {err}",
            e.pid
        ));
    }
    // Write on a helper thread so a full pipe cannot block this call; stdin
    // is taken out of the mutex, so `stop` never waits on the write.
    let payload = [text.as_bytes(), b"\n"].concat();
    let sent = payload.len();
    let deadline = Instant::now() + INPUT_WRITE_TIMEOUT;
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let entry = Arc::clone(&e);
    let cancel_for_write = cancel.clone();
    std::thread::spawn(move || {
        let res = write_payload(&entry, &mut stdin, &payload, &cancel_for_write, deadline);
        // Drop stdin when exited or abandoned, closing the pipe so the child
        // sees EOF instead of a surprise late delivery.
        if !entry.has_exited() && !entry.stdin_abandoned.load(Ordering::SeqCst) {
            *lock(&entry.stdin) = Some(stdin);
        }
        entry.writing.store(false, Ordering::SeqCst);
        let _ = tx.send(res); // the caller may have given up (cancel/timeout)
    });

    // Poll so cancel/timeout return promptly even if the thread is stuck in
    // a blocking `write` the child never drains.
    loop {
        if cancel.is_cancelled() {
            // Un-block future inputs; the writer thread drops stdin later.
            e.abandon_write();
            return Err(CANCEL_REASON.into());
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(res) => {
                res?;
                return Ok(format!(
                    "Sent {} bytes to process {} stdin: {}",
                    sent,
                    e.pid,
                    describe(&e)
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                e.abandon_write();
                return Err(stdin_timeout_err(&e));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Writer thread died without restoring state.
                e.abandon_write();
                return Err(format!(
                    "Stdin writer for process {} ({}) failed unexpectedly",
                    e.pid, e.command
                ));
            }
        }
    }
}

/// Timeout message shared by `input`'s wait loop and [`write_payload`].
fn stdin_timeout_err(e: &ProcessEntry) -> String {
    format!(
        "Timed out writing to stdin of process {} ({}): pipe full; \
         the input may still be delivered once the process reads",
        e.pid, e.command
    )
}

/// Write `payload` to the process's stdin via [`write_stdin_bounded`],
/// formatting failures with process context. Bytes already in the pipe may
/// still be delivered on cancel/timeout.
fn write_payload(
    entry: &ProcessEntry,
    stdin: &mut ChildStdin,
    payload: &[u8],
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<(), String> {
    let io_err = |err: std::io::Error| {
        format!(
            "Failed to write to stdin of process {} ({}): {err}",
            entry.pid, entry.command
        )
    };
    match write_stdin_bounded(stdin, payload, cancel, deadline) {
        Ok(()) => Ok(()),
        Err(StdinWriteError::Cancelled) => Err(CANCEL_REASON.into()),
        Err(StdinWriteError::TimedOut) => Err(stdin_timeout_err(entry)),
        Err(StdinWriteError::Io(e)) => Err(io_err(e)),
    }
}

fn not_accepting_input(e: &ProcessEntry, why: &str) -> String {
    format!(
        "Process {} ({}) is not accepting input ({why})",
        e.pid, e.command
    )
}

fn wait_action(
    args: &Value,
    cancel: &CancellationToken,
    sink: Option<OutputSink>,
) -> Result<String, String> {
    let e = entry_for(args)?;
    let timeout_ms = arg_u64(args, "timeout").unwrap_or(0);
    let follow = args.get("follow").and_then(Value::as_bool).unwrap_or(false);
    match wait_for_exit(&e, follow, timeout_ms, cancel, sink.as_ref())? {
        WaitOutcome::Exited(code) => {
            let mut msg = format!("Process {} exited with code {code}", e.pid);
            if !follow {
                let tail = e.logs.tail(DEFAULT_LINES as usize);
                if !tail.is_empty() {
                    msg.push_str("\n\nRecent output:\n");
                    msg.push_str(&tail);
                }
            }
            Ok(msg)
        }
        WaitOutcome::Timeout => Ok(format!(
            "Process {} still running after {}ms",
            e.pid, timeout_ms
        )),
    }
}

fn stop(args: &Value) -> Result<String, String> {
    let e = entry_for(args)?;
    let signal = match arg_str(args, "signal").unwrap_or("terminate") {
        "terminate" => ProcessSignal::Terminate,
        "kill" => ProcessSignal::Kill,
        "interrupt" => ProcessSignal::Interrupt,
        other => return Err(format!("Unknown signal '{other}'")),
    };
    signal_and_wait(&e, signal)
}

fn restart(args: &Value, workspace: &Path) -> Result<String, String> {
    let e = entry_for(args)?;
    // Validate command/cwd/env up front so a bad override never stops the
    // still-running process first; each falls back to the entry's own values.
    let command = arg_str(args, "command").unwrap_or(&e.command).to_string();
    let parts = parse_command(&command)?;
    let cwd = match arg_str(args, "cwd") {
        Some(cwd) => {
            resolve_path(cwd, workspace).map_err(|err| format!("Invalid cwd '{cwd}': {err}"))?
        }
        None => e.cwd.clone(),
    };
    // "No override" (null/blank/"null"; strict mode sends "" for unset)
    // inherits the entry's env; an explicit object — even empty — replaces it.
    let env = match args.get("env") {
        Some(v) if env_is_no_override(v) => e.env.clone(),
        Some(v) => parse_env(v)?,
        None => e.env.clone(),
    };
    let note = stop_for_restart(&e);
    // A replacement keeps the original entry's owning tab.
    let entry = start_command(&command, parts, cwd, env, e.tab)?;
    Ok(format!(
        "Process {} ({}) {note}; replacement started with pid {}",
        e.pid, e.command, entry.pid
    ))
}

// ── Core helpers ───────────────────────────────────────────────────

fn entry_for(args: &Value) -> Result<Arc<ProcessEntry>, String> {
    let pid = arg_pid(args)?;
    let procs = lock(&PROCESSES);
    procs
        .get(&pid)
        .cloned()
        .ok_or_else(|| format!("Unknown pid: {pid}"))
}

/// Parse `pid` as an integer or numeric string (strict mode may coerce it to a string).
/// Missing and unparseable/out-of-range values are reported separately so agent
/// failures are easier to diagnose.
fn arg_pid(args: &Value) -> Result<u32, String> {
    let Some(v) = args.get("pid") else {
        return Err("Missing 'pid' argument".into());
    };
    let n: u64 = v
        .as_u64()
        .or_else(|| v.as_str()?.trim().parse().ok())
        .ok_or_else(|| "Invalid 'pid' argument".to_string())?;
    u32::try_from(n).map_err(|_| "Invalid 'pid' argument".to_string())
}

/// True for "no override" env values: null, blank, or the string "null".
fn env_is_no_override(val: &Value) -> bool {
    val.is_null()
        || val
            .as_str()
            .is_some_and(|s| matches!(s.trim(), "" | "null"))
}

/// Parse an `env` value: an object of name-value pairs, or a JSON-encoded
/// object string (strict-mode schemas coerce `env` to a string).
/// Null and blank strings yield an empty map.
pub fn parse_env(val: &Value) -> Result<HashMap<String, String>, String> {
    const ERR: &str = "'env' must be an object or a JSON-encoded object string";
    let decoded;
    let val = match val {
        Value::String(s) if s.trim().is_empty() => return Ok(HashMap::new()),
        Value::String(s) => {
            decoded = serde_json::from_str(s).map_err(|_| ERR.to_string())?;
            &decoded
        }
        other => other,
    };
    if val.is_null() {
        return Ok(HashMap::new());
    }
    let obj = val.as_object().ok_or_else(|| ERR.to_string())?;
    let mut env = HashMap::with_capacity(obj.len());
    for (key, value) in obj {
        let value = value
            .as_str()
            .ok_or_else(|| format!("env value for '{key}' must be a string"))?;
        env.insert(key.clone(), value.to_string());
    }
    Ok(env)
}

/// Split and validate a command into argv (no platform shell), separate from
/// [`start_command`] so `restart` can reject a bad override before stopping.
fn parse_command(command: &str) -> Result<Vec<String>, String> {
    let parts = split(command).map_err(|e| format!("Failed to parse command: {e}"))?;
    if parts.is_empty() {
        return Err("Empty command".into());
    }
    Ok(parts)
}

/// Spawn validated argv (see [`parse_command`]), register the entry, and
/// start reader + reaper threads.
fn start_command(
    command: &str,
    parts: Vec<String>,
    cwd: PathBuf,
    env: HashMap<String, String>,
    tab: Option<usize>,
) -> Result<Arc<ProcessEntry>, String> {
    let (exe, exe_args) = parts
        .split_first()
        .expect("command was validated non-empty");

    let mut cmd = Command::new(exe);
    cmd.args(exe_args).current_dir(&cwd);
    sanitize_child_env(&mut cmd);
    for (key, value) in &env {
        cmd.env(key, value);
    }
    detach_child(&mut cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start process: {e}"))?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().ok_or("stdout pipe missing")?;
    let stderr = child.stderr.take().ok_or("stderr pipe missing")?;

    let entry = Arc::new(ProcessEntry {
        command: command.to_string(),
        cwd,
        env,
        tab,
        pid,
        started_at: Instant::now(),
        status: Mutex::new(ProcessStatus::Running),
        stdin: Mutex::new(stdin),
        writing: AtomicBool::new(false),
        stdin_abandoned: AtomicBool::new(false),
        logs: ProcessLogs::new(tool_limits().max_output_bytes),
        pending_readers: AtomicUsize::new(2),
    });

    {
        let mut procs = lock(&PROCESSES);
        prune_exited(&mut procs);
        // Pid reuse: the new process takes the key from any retained exited entry.
        procs.insert(pid, Arc::clone(&entry));
    }
    notify_process_changed();

    let stdout_raw = raw_pipe(&stdout);
    spawn_reader(Arc::clone(&entry), stdout, stdout_raw);
    let stderr_raw = raw_pipe(&stderr);
    spawn_reader(Arc::clone(&entry), stderr, stderr_raw);
    spawn_reaper(Arc::clone(&entry), child);
    Ok(entry)
}

/// True once the process has exited and [`DRAIN_GRACE`] has passed since the
/// exit was first noticed (the window is not extended by later output).
fn drain_should_stop(entry: &ProcessEntry, exit_seen_at: &mut Option<Instant>) -> bool {
    if !entry.has_exited() {
        return false;
    }
    let seen = *exit_seen_at.get_or_insert_with(Instant::now);
    seen.elapsed() >= DRAIN_GRACE
}

/// Raw fd of a child pipe read end (for polling).
#[cfg(unix)]
fn raw_pipe<T: AsRawFd>(r: &T) -> i32 {
    r.as_raw_fd()
}

/// Raw handle of a child pipe read end (for `PeekNamedPipe` polling).
#[cfg(windows)]
fn raw_pipe<T: AsRawHandle>(r: &T) -> RawHandle {
    r.as_raw_handle()
}

/// Result of one read attempt on a child output pipe.
enum ReadStep {
    Data(usize),
    Eof,
    /// No data right now; retry after a short idle.
    Retry,
    Err,
}

/// Drain a child output pipe into the shared log buffer: until EOF, or for
/// [`DRAIN_GRACE`] after the process has exited (see [`drain_should_stop`]).
fn spawn_reader(
    entry: Arc<ProcessEntry>,
    mut reader: impl Read + Send + 'static,
    #[cfg(unix)] raw: i32,
    #[cfg(windows)] raw: RawHandle,
) {
    #[cfg(windows)]
    let raw = raw as isize; // RawHandle is !Send; the handle stays owned by `reader`.
    std::thread::spawn(move || {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        let mut exit_seen_at: Option<Instant> = None;
        loop {
            let step = read_step(&mut reader, &mut buf, raw);
            let data = match step {
                ReadStep::Data(n) => Some(n),
                ReadStep::Eof | ReadStep::Err => break,
                ReadStep::Retry => None,
            };
            if let Some(n) = data {
                decoder.feed(&buf[..n], &mut out);
                for text in out.drain(..) {
                    entry.logs.push(text);
                }
            }
            // Stop after the drain grace even without EOF: a daemonised
            // grandchild may keep the pipe open and write forever.
            if drain_should_stop(&entry, &mut exit_seen_at) {
                break;
            }
        }
        decoder.flush(&mut out);
        for text in out {
            entry.logs.push(text);
        }
        entry.pending_readers.fetch_sub(1, Ordering::SeqCst);
    });
}

/// Map one blocking `read` result onto [`ReadStep`].
fn classify_read(result: std::io::Result<usize>) -> ReadStep {
    match result {
        Ok(0) => ReadStep::Eof,
        Ok(n) => ReadStep::Data(n),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => ReadStep::Retry,
        Err(_) => ReadStep::Err,
    }
}

/// One read attempt: poll first so an exit is noticed even when the write end
/// stays open.
#[cfg(unix)]
fn read_step(reader: &mut impl Read, buf: &mut [u8], fd: i32) -> ReadStep {
    if !poll_readable(fd, READER_IDLE_POLL_MS) {
        return ReadStep::Retry;
    }
    classify_read(reader.read(buf))
}

/// One read attempt: peek first, then read only the bytes already buffered so
/// a blocking `ReadFile` can never hang.
#[cfg(windows)]
fn read_step(reader: &mut impl Read, buf: &mut [u8], raw: isize) -> ReadStep {
    match crate::tools::peek_pipe_available(raw as RawHandle) {
        Some(0) => {
            std::thread::sleep(READER_IDLE_SLEEP);
            ReadStep::Retry
        }
        None => ReadStep::Eof, // broken pipe — EOF
        Some(_) => classify_read(reader.read(buf)),
    }
}

/// True when a read on `fd` won't block (data, EOF, or error); false on
/// timeout.
#[cfg(unix)]
fn poll_readable(fd: i32, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `fd` is the child's pipe read end; poll only observes it.
    let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    ready > 0 && (pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0
}

fn spawn_reaper(entry: Arc<ProcessEntry>, mut child: Child) {
    std::thread::spawn(move || {
        let code = match child.wait() {
            Ok(status) => exit_code_of(&status),
            Err(_) => -1,
        };
        *lock(&entry.status) = ProcessStatus::Exited(code);
        // Close stdin so a later `input` fails cleanly.
        lock(&entry.stdin).take();
        notify_process_changed();
    });
}

/// Stop a process for `restart`, escalating terminate → kill; returns the
/// note describing what happened.
fn stop_for_restart(e: &ProcessEntry) -> String {
    if let Some(code) = e.exit_code_opt() {
        return format!("had already exited (code {code})");
    }
    let _ = signal_and_wait(e, ProcessSignal::Terminate);
    if e.has_exited() {
        return "stopped".into();
    }
    let _ = signal_and_wait(e, ProcessSignal::Kill);
    if e.has_exited() {
        "stopped (kill after terminate ignored)".into()
    } else {
        "still running (kill sent)".into()
    }
}

/// Close stdin, send `signal` to the process tree, and wait up to
/// [`STOP_GRACE`] for the reaper to record the exit.
fn signal_and_wait(entry: &ProcessEntry, signal: ProcessSignal) -> Result<String, String> {
    lock(&entry.stdin).take();

    if let Some(code) = entry.exit_code_opt() {
        return Ok(format!(
            "Process {} ({}) already exited with code {code}",
            entry.pid, entry.command
        ));
    }

    // Tiny pid-reuse window between the check and the signal; closing it fully
    // would need the reaper to hold the status lock across `child.wait()`.
    signal_process_tree(entry.pid, signal);
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if let Some(code) = entry.exit_code_opt() {
            return Ok(format!(
                "Process {} ({}) stopped (exit code {code})",
                entry.pid, entry.command
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Ok(format!(
        "{signal} signal sent to process {} ({}, up {}s); still running",
        entry.pid,
        entry.command,
        entry.started_secs()
    ))
}

/// Wait (bounded) for the reader threads to drain, so a tail read issued
/// right after exit is complete.
fn wait_for_drain(entry: &ProcessEntry) {
    if entry.pending_readers.load(Ordering::SeqCst) == 0 {
        return;
    }
    let deadline = Instant::now() + STOP_GRACE;
    while entry.pending_readers.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Outcome of [`wait_for_exit`].
enum WaitOutcome {
    Exited(i32),
    Timeout,
}

/// Poll until the process exits (status recorded and readers drained) or
/// `timeout_ms` elapses, streaming new chunks to `sink` when `follow`.
fn wait_for_exit(
    e: &ProcessEntry,
    follow: bool,
    timeout_ms: u64,
    cancel: &CancellationToken,
    sink: Option<&OutputSink>,
) -> Result<WaitOutcome, String> {
    let deadline = (timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    let mut last_seq = e.logs.last_seq();
    loop {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        if follow {
            let (texts, seq) = e.logs.drain_since(last_seq);
            last_seq = seq;
            if let Some(sink) = sink {
                for text in texts {
                    sink(&text);
                }
            }
        }
        if e.is_done() {
            return Ok(WaitOutcome::Exited(exit_code(e)));
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(WaitOutcome::Timeout);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Stream new chunks until the process exits or `timeout_ms` elapses, then
/// return the recent tail plus the exit code (or a still-running note).
fn follow_logs(
    entry: &ProcessEntry,
    lines: usize,
    timeout_ms: u64,
    cancel: &CancellationToken,
    sink: Option<&OutputSink>,
) -> Result<String, String> {
    match wait_for_exit(entry, true, timeout_ms, cancel, sink)? {
        WaitOutcome::Exited(code) => {
            let tail = entry.logs.tail(lines);
            if tail.is_empty() {
                Ok(format!("Process {} exited with code {code}", entry.pid))
            } else {
                Ok(format!(
                    "Process {} exited with code {code}\n\n{tail}",
                    entry.pid
                ))
            }
        }
        WaitOutcome::Timeout => {
            let note = format!(
                "[process {} still running after {}ms]",
                entry.pid, timeout_ms
            );
            let tail = entry.logs.tail(lines);
            Ok(if tail.is_empty() {
                note
            } else {
                format!("{tail}\n\n{note}")
            })
        }
    }
}

/// Human-readable identity of a managed process: command plus uptime.
/// Surfaced in operation results so a pid-reuse alias becomes visible to the
/// agent (the pid alone would silently point at the new process).
fn describe(e: &ProcessEntry) -> String {
    format!("{} (up {}s)", e.command, e.started_secs())
}

fn describe_status(e: &ProcessEntry) -> (&'static str, Option<String>) {
    let status = *lock(&e.status);
    match status {
        ProcessStatus::Running => ("running", Some(format!("up {}s", e.started_secs()))),
        ProcessStatus::Exited(code) => ("exited", Some(format!("exit code {code}"))),
    }
}

fn exit_code(e: &ProcessEntry) -> i32 {
    e.exit_code_opt().unwrap_or(-1)
}

/// Drop the oldest exited entries once more than [`MAX_RETAINED_EXITED`] remain.
fn prune_exited(procs: &mut HashMap<u32, Arc<ProcessEntry>>) {
    let mut exited: Vec<(Instant, u32)> = procs
        .values()
        .filter(|e| e.has_exited())
        .map(|e| (e.started_at, e.pid))
        .collect();
    exited.sort_unstable();
    let excess = exited.len().saturating_sub(MAX_RETAINED_EXITED);
    for (_, pid) in exited.into_iter().take(excess) {
        procs.remove(&pid);
    }
}

/// Stop every managed process (app exit / restart): close stdin, send
/// `terminate`, then escalate to `kill` for any still running after
/// [`STOP_GRACE`]. Fire-and-forget — the app is exiting, so there is no
/// caller to report to.
pub fn shutdown() {
    // Clone the entries and drop the global lock before signaling, so a
    // concurrent `start` in another tab is not blocked by the grace wait.
    let entries: Vec<Arc<ProcessEntry>> = lock(&PROCESSES).values().cloned().collect();
    if entries.is_empty() {
        return;
    }
    for e in &entries {
        if e.has_exited() {
            continue;
        }
        // Close stdin so well-behaved children exit on EOF.
        lock(&e.stdin).take();
        signal_process_tree(e.pid, ProcessSignal::Terminate);
    }
    // Give them a short grace period to exit, then force-kill stragglers.
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline && entries.iter().any(|e| !e.has_exited()) {
        std::thread::sleep(POLL_INTERVAL);
    }
    for e in entries {
        if !e.has_exited() {
            signal_process_tree(e.pid, ProcessSignal::Kill);
        }
    }
}

// ── Managed process state ──────────────────────────────────────────

#[derive(Clone, Copy)]
enum ProcessStatus {
    Running,
    Exited(i32),
}

struct ProcessEntry {
    /// OS pid of the managed child (also the registry key).
    pid: u32,
    command: String,
    cwd: PathBuf,
    env: HashMap<String, String>,
    /// Owning session tab number, `None` when started outside the LLM loop.
    tab: Option<usize>,
    started_at: Instant,
    status: Mutex<ProcessStatus>,
    stdin: Mutex<Option<ChildStdin>>,
    /// Set while an `input` write is in flight; guards against concurrent writes.
    writing: AtomicBool,
    /// Set when an in-flight input write was cancelled/timed out: the writer
    /// thread discards stdin on completion instead of returning it.
    stdin_abandoned: AtomicBool,
    logs: ProcessLogs,
    /// Reader threads still draining stdout/stderr.
    pending_readers: AtomicUsize,
}

impl ProcessEntry {
    fn started_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    fn has_exited(&self) -> bool {
        self.exit_code_opt().is_some()
    }

    fn exit_code_opt(&self) -> Option<i32> {
        let status = *lock(&self.status);
        match status {
            ProcessStatus::Exited(code) => Some(code),
            ProcessStatus::Running => None,
        }
    }

    fn is_done(&self) -> bool {
        self.has_exited() && self.pending_readers.load(Ordering::SeqCst) == 0
    }

    /// Mark the in-flight input write as abandoned: the writer thread drops
    /// stdin when its write completes (closing the pipe), and the writing
    /// flag clears immediately so a wedged writer can't block future inputs.
    fn abandon_write(&self) {
        self.stdin_abandoned.store(true, Ordering::SeqCst);
        self.writing.store(false, Ordering::SeqCst);
    }
}

// ── Bounded, merged log buffer ─────────────────────────────────────

struct LogChunk {
    seq: u64,
    text: String,
}

struct LogState {
    chunks: VecDeque<LogChunk>,
    bytes: usize,
    /// Next chunk sequence; starts at 1 so `drain_since(0)` yields every chunk.
    seq: u64,
}

/// Merged stdout/stderr log buffer in arrival order, bounded to a byte cap
/// (drop-oldest).
pub struct ProcessLogs {
    inner: Mutex<LogState>,
    cap: usize,
}

impl ProcessLogs {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(LogState {
                chunks: VecDeque::new(),
                bytes: 0,
                seq: 1,
            }),
            cap: cap.max(1),
        }
    }

    pub fn push(&self, text: String) {
        if text.is_empty() {
            return;
        }
        let mut state = lock(&self.inner);
        let seq = state.seq;
        state.seq += 1;
        state.bytes += text.len();
        state.chunks.push_back(LogChunk { seq, text });
        while state.bytes > self.cap {
            if let Some(front) = state.chunks.pop_front() {
                state.bytes = state.bytes.saturating_sub(front.text.len());
            } else {
                break;
            }
        }
    }

    fn len(&self) -> usize {
        lock(&self.inner).bytes
    }

    /// Sequence of the newest chunk (0 when empty; seqs start at 1).
    fn last_seq(&self) -> u64 {
        lock(&self.inner).chunks.back().map(|c| c.seq).unwrap_or(0)
    }

    /// Return chunks with `seq > after_seq` plus the new last sequence.
    fn drain_since(&self, after_seq: u64) -> (Vec<String>, u64) {
        let state = lock(&self.inner);
        let texts: Vec<String> = state
            .chunks
            .iter()
            .filter(|c| c.seq > after_seq)
            .map(|c| c.text.clone())
            .collect();
        let last = state.chunks.back().map(|c| c.seq).unwrap_or(after_seq);
        (texts, last)
    }

    /// The last `lines` lines of the merged buffer, joined with `\n`.
    pub fn tail(&self, lines: usize) -> String {
        if lines == 0 {
            return String::new();
        }
        let state = lock(&self.inner);
        let mut combined = String::with_capacity(state.bytes);
        for chunk in &state.chunks {
            combined.push_str(&chunk.text);
        }
        let all: Vec<&str> = combined.lines().collect();
        let start = all.len().saturating_sub(lines);
        all[start..].join("\n")
    }
}
