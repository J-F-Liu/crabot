//! `process` tool: manages long-running processes across tool calls.
//!
//! Spawns like the `bash` tool's host-command bridge (shell-words split,
//! direct exec, host env minus secrets plus `env` overrides) and keeps the
//! children in an app-global registry under stable ids (`proc-1`, …) instead
//! of OS pids.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use shell_words::split;
use tokio_util::sync::CancellationToken;

use super::{
    CANCEL_REASON, OutputSink, ProcessSignal, StreamDecoder, Tool, arg_str, arg_u64, detach_child,
    exit_code_of, lock, resolve_path, sanitize_child_env, signal_process_tree, tool_limits,
};

/// Cap on retained exited process entries; the oldest are dropped on `start`.
const MAX_RETAINED_EXITED: usize = 64;
/// How long `stop` waits for a signaled process to be reaped before reporting.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// Poll interval for `wait`/`logs --follow`/`stop`.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// How long non-follow `logs` waits for a fresh process's exit to be recorded.
const SETTLE_GRACE_MS: u64 = 250;
/// Default and maximum number of log lines returned by `logs`.
const DEFAULT_LINES: u64 = 100;
const MAX_LINES: u64 = 2000;

// ── Global process registry ────────────────────────────────────────

/// App-global registry of agent-managed processes, keyed by `process_id`.
/// Shared across session tabs, like the MCP connection cache.
static PROCESSES: LazyLock<Mutex<HashMap<String, Arc<ProcessEntry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct ProcessTool;

impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "process"
    }

    fn description(&self) -> &str {
        "Manage long-running processes started by the agent, including starting, monitoring, interacting with, and stopping processes."
    }

    fn instruction(&self) -> &str {
        "Use the process tool to start, monitor, interact with, and stop long-running processes such as servers, watchers, and REPLs. `start` returns an agent-managed `process_id`; pass it to every later operation. Prefer this over the `bash` tool for anything that does not exit on its own, and remember to stop processes you no longer need."
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
                "process_id": {
                    "type": "string",
                    "description": "The process identifier returned by start. Required for operations on an existing process."
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
                    "description": "Additional environment variables for the process."
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
        "input" => input(args),
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
    let env = parse_env(args)?;
    let entry = start_command(command, cwd, env)?;
    Ok(format!(
        "Started process {} (os pid {}): {}\ncwd: {}",
        entry.id,
        entry.pid,
        entry.command,
        entry.cwd.display()
    ))
}

fn list() -> Result<String, String> {
    let procs = lock(&PROCESSES);
    if procs.is_empty() {
        return Ok("No managed processes.".into());
    }
    let mut entries: Vec<&Arc<ProcessEntry>> = procs.values().collect();
    entries.sort_by_key(|e| e.n);
    let mut lines = Vec::new();
    for e in entries {
        let (state, detail) = describe_status(e);
        let detail = detail.map(|d| format!(" ({d})")).unwrap_or_default();
        lines.push(format!("{}: {} — {}{}", e.id, state, e.command, detail));
    }
    Ok(lines.join("\n"))
}

fn status(args: &Value) -> Result<String, String> {
    let e = entry_for(args)?;
    let (state, detail) = describe_status(&e);
    let detail = detail.map(|d| format!("\ndetail: {d}")).unwrap_or_default();
    Ok(format!(
        "process_id: {}\nstatus: {}\nos pid: {}\ncommand: {}\ncwd: {}\nlog bytes: {}{}",
        e.id,
        state,
        e.pid,
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
        if matches!(*lock(&e.status), ProcessStatus::Exited(_)) {
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

fn input(args: &Value) -> Result<String, String> {
    let e = entry_for(args)?;
    let text = arg_str(args, "input").ok_or("Missing 'input' argument")?;
    let mut stdin = lock(&e.stdin);
    let Some(stdin) = stdin.as_mut() else {
        return Err(format!(
            "Process {} is not accepting input (it has exited or been stopped)",
            e.id
        ));
    };
    stdin
        .write_all(text.as_bytes())
        .and_then(|_| stdin.write_all(b"\n"))
        .and_then(|_| stdin.flush())
        .map_err(|err| format!("Failed to write to stdin of process {}: {err}", e.id))?;
    Ok(format!(
        "Sent {} bytes to process {} stdin",
        text.len() + 1,
        e.id
    ))
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
            let mut msg = format!("Process {} exited with code {code}", e.id);
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
            e.id, timeout_ms
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
    let cwd = match arg_str(args, "cwd") {
        Some(cwd) => {
            resolve_path(cwd, workspace).map_err(|err| format!("Invalid cwd '{cwd}': {err}"))?
        }
        None => e.cwd.clone(),
    };
    let env = match args.get("env") {
        Some(_) => parse_env(args)?,
        None => e.env.clone(),
    };
    let note = stop_for_restart(&e);
    let entry = start_command(&command, cwd, env)?;
    Ok(format!(
        "Process {} {note}; replacement started with process_id {} (os pid {})",
        e.id, entry.id, entry.pid
    ))
}

// ── Core helpers ───────────────────────────────────────────────────

fn entry_for(args: &Value) -> Result<Arc<ProcessEntry>, String> {
    let process_id = arg_str(args, "process_id").ok_or("Missing 'process_id' argument")?;
    let procs = lock(&PROCESSES);
    procs
        .get(process_id)
        .cloned()
        .ok_or_else(|| format!("Unknown process_id: {process_id}"))
}

fn parse_env(args: &Value) -> Result<HashMap<String, String>, String> {
    let mut env = HashMap::new();
    let Some(env_val) = args.get("env") else {
        return Ok(env);
    };
    let obj = env_val.as_object().ok_or("'env' must be an object")?;
    for (key, value) in obj {
        let value = value
            .as_str()
            .ok_or_else(|| format!("env value for '{key}' must be a string"))?;
        env.insert(key.clone(), value.to_string());
    }
    Ok(env)
}

/// Spawn `command` (split like the `bash` tool's host-command bridge: no
/// platform shell), register it, and start reader + reaper threads.
fn start_command(
    command: &str,
    cwd: PathBuf,
    env: HashMap<String, String>,
) -> Result<Arc<ProcessEntry>, String> {
    let parts = split(command).map_err(|e| format!("Failed to parse command: {e}"))?;
    let (exe, exe_args) = parts.split_first().ok_or("Empty command")?;

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

    let n = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let id = format!("proc-{n}");
    let entry = Arc::new(ProcessEntry {
        n,
        id,
        command: command.to_string(),
        cwd,
        env,
        pid,
        started_at: Instant::now(),
        status: Mutex::new(ProcessStatus::Running),
        stdin: Mutex::new(stdin),
        logs: ProcessLogs::new(tool_limits().max_output_bytes),
        pending_readers: AtomicUsize::new(2),
    });

    {
        let mut procs = lock(&PROCESSES);
        prune_exited(&mut procs);
        procs.insert(entry.id.clone(), Arc::clone(&entry));
    }

    spawn_reader(Arc::clone(&entry), stdout);
    spawn_reader(Arc::clone(&entry), stderr);
    spawn_reaper(Arc::clone(&entry), child);
    Ok(entry)
}

fn spawn_reader(entry: Arc<ProcessEntry>, mut reader: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    decoder.feed(&buf[..n], &mut out);
                    for text in out.drain(..) {
                        entry.logs.push(text);
                    }
                }
                Err(_) => break,
            }
        }
        decoder.flush(&mut out);
        for text in out {
            entry.logs.push(text);
        }
        entry.pending_readers.fetch_sub(1, Ordering::SeqCst);
    });
}

fn spawn_reaper(entry: Arc<ProcessEntry>, mut child: Child) {
    std::thread::spawn(move || {
        let code = match child.wait() {
            Ok(status) => exit_code_of(&status),
            Err(_) => -1,
        };
        *lock(&entry.status) = ProcessStatus::Exited(code);
        // Close stdin so a later `input` fails cleanly.
        entry.stdin.lock().unwrap_or_else(|e| e.into_inner()).take();
    });
}

/// Stop a process for `restart`, escalating terminate → kill; returns the
/// note describing what happened.
fn stop_for_restart(e: &ProcessEntry) -> String {
    if let ProcessStatus::Exited(code) = *lock(&e.status) {
        return format!("had already exited (code {code})");
    }
    let _ = signal_and_wait(e, ProcessSignal::Terminate);
    if matches!(*lock(&e.status), ProcessStatus::Exited(_)) {
        return "stopped".into();
    }
    let _ = signal_and_wait(e, ProcessSignal::Kill);
    if matches!(*lock(&e.status), ProcessStatus::Exited(_)) {
        "stopped (kill after terminate ignored)".into()
    } else {
        "still running (kill sent)".into()
    }
}

/// Close stdin, send `signal` to the process tree, and wait up to
/// [`STOP_GRACE`] for the reaper to record the exit.
fn signal_and_wait(entry: &ProcessEntry, signal: ProcessSignal) -> Result<String, String> {
    entry.stdin.lock().unwrap_or_else(|e| e.into_inner()).take();

    if let ProcessStatus::Exited(code) = *lock(&entry.status) {
        return Ok(format!(
            "Process {} already exited with code {code}",
            entry.id
        ));
    }

    // Tiny pid-reuse window between the check and the signal; closing it fully
    // would need the reaper to hold the status lock across `child.wait()`.
    signal_process_tree(entry.pid, signal);
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if let ProcessStatus::Exited(code) = *lock(&entry.status) {
            return Ok(format!("Process {} stopped (exit code {code})", entry.id));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Ok(format!(
        "{} signal sent to process {} (os pid {}); still running",
        signal.name(),
        entry.id,
        entry.pid
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
                Ok(format!("Process {} exited with code {code}", entry.id))
            } else {
                Ok(format!(
                    "Process {} exited with code {code}\n\n{tail}",
                    entry.id
                ))
            }
        }
        WaitOutcome::Timeout => {
            let note = format!(
                "[process {} still running after {}ms]",
                entry.id, timeout_ms
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

fn describe_status(e: &ProcessEntry) -> (&'static str, Option<String>) {
    match *lock(&e.status) {
        ProcessStatus::Running => (
            "running",
            Some(format!("os pid {}, up {}s", e.pid, e.started_secs())),
        ),
        ProcessStatus::Exited(code) => ("exited", Some(format!("exit code {code}"))),
    }
}

fn exit_code(e: &ProcessEntry) -> i32 {
    match *lock(&e.status) {
        ProcessStatus::Exited(code) => code,
        ProcessStatus::Running => -1,
    }
}

/// Drop the oldest exited entries once more than [`MAX_RETAINED_EXITED`] remain.
fn prune_exited(procs: &mut HashMap<String, Arc<ProcessEntry>>) {
    let mut exited: Vec<(u64, String)> = procs
        .values()
        .filter(|e| matches!(*lock(&e.status), ProcessStatus::Exited(_)))
        .map(|e| (e.n, e.id.clone()))
        .collect();
    exited.sort_unstable();
    let excess = exited.len().saturating_sub(MAX_RETAINED_EXITED);
    for (_, id) in exited.into_iter().take(excess) {
        procs.remove(&id);
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
        if matches!(*lock(&e.status), ProcessStatus::Exited(_)) {
            continue;
        }
        // Close stdin so well-behaved children exit on EOF.
        e.stdin.lock().unwrap_or_else(|p| p.into_inner()).take();
        signal_process_tree(e.pid, ProcessSignal::Terminate);
    }
    // Give them a short grace period to exit, then force-kill stragglers.
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline
        && entries
            .iter()
            .any(|e| !matches!(*lock(&e.status), ProcessStatus::Exited(_)))
    {
        std::thread::sleep(POLL_INTERVAL);
    }
    for e in entries {
        if !matches!(*lock(&e.status), ProcessStatus::Exited(_)) {
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
    /// Numeric id backing the stable `process_id` (used for ordering/pruning).
    n: u64,
    id: String,
    command: String,
    cwd: PathBuf,
    env: HashMap<String, String>,
    pid: u32,
    started_at: Instant,
    status: Mutex<ProcessStatus>,
    stdin: Mutex<Option<ChildStdin>>,
    logs: ProcessLogs,
    /// Reader threads still draining stdout/stderr.
    pending_readers: AtomicUsize,
}

impl ProcessEntry {
    fn started_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    fn is_done(&self) -> bool {
        matches!(*lock(&self.status), ProcessStatus::Exited(_))
            && self.pending_readers.load(Ordering::SeqCst) == 0
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
}

/// Merged stdout/stderr log buffer in arrival order, bounded to a byte cap
/// (drop-oldest).
pub struct ProcessLogs {
    inner: Mutex<LogState>,
    seq: AtomicU64,
    cap: usize,
}

impl ProcessLogs {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(LogState {
                chunks: VecDeque::new(),
                bytes: 0,
            }),
            seq: AtomicU64::new(0),
            cap: cap.max(1),
        }
    }

    pub fn push(&self, text: String) {
        if text.is_empty() {
            return;
        }
        let mut state = lock(&self.inner);
        state.bytes += text.len();
        state.chunks.push_back(LogChunk {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            text,
        });
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

    /// Sequence of the newest chunk in the buffer (0 when empty).
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
