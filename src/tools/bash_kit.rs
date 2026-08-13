//! In-process bashkit interpreter for the `bash` tool.
//!
//! External commands are bridged to host executables via [`HostCommandBuiltin`]
//! — no real bash process, so the same path works natively on Windows. Scripts
//! the interpreter cannot faithfully handle (parse errors, dynamic command
//! names, `eval`/`exec`/`source`, path-based or glob-shaped names) make
//! [`collect_external_names`] return `Err`, falling back to real `bash -c`.
//!
//! Wrapper builtins (`timeout`, `xargs`, `find -exec`) hide commands in their
//! arguments; [`collect_external_names`] extracts those names from literal
//! arguments. `watch`/`parallel` stubs never run commands and `env` refuses
//! them, so scripts that would involve one fall back to real bash. Mirrors
//! bashkit 0.16.0 — re-verify when bumping.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bashkit::analysis::{AnalyzedCommand, analyze_with_limits};
use bashkit::{Bash, Builtin, BuiltinContext, ExecResult, ExecutionLimits, async_trait};

use super::{
    ChunkForwarder, OutputSink, WaitError, create_pipe_pair, is_would_block, pipe_to_stdio,
    set_sender_nonblocking, set_sender_noninheritable, wait_with_timeout,
};

/// Per-stream output cap (head-only backstop; crabot's own truncation is the visible limit).
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;

/// Cached set of every builtin this bashkit build can dispatch.
pub(crate) fn builtin_names() -> &'static HashSet<String> {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: HashSet<String> = Bash::new().builtin_names().into_iter().collect();
        // `Bash::new()` skips builder-registered builtins; `build_bash`
        // registers these explicitly via `.python()`.
        names.insert("python".to_string());
        names.insert("python3".to_string());
        names
    })
}

/// Statically collect the external command names a script executes.
///
/// Returns `Err(())` when the script cannot run faithfully in-process (parse
/// error, dynamic/path-based/glob-shaped command names, opaque builtins,
/// wrapper arguments that hide a command). Otherwise returns the names to
/// bridge (empty when only builtins are used).
pub(crate) fn collect_external_names(script: &str) -> Result<Vec<String>, ()> {
    let analysis = analyze_with_limits(script, 100, 100_000).map_err(|_| ())?;
    if analysis.is_opaque() {
        return Err(());
    }
    let builtins = builtin_names();
    let mut names = Vec::new();
    for command in &analysis.commands {
        let Some(name) = command.name.as_deref() else {
            continue; // dynamic names already made the analysis opaque
        };
        // Unsupported: opaque payload, path-based (`./s.sh`), glob-shaped (`$TOOL`).
        if is_unbridgeable(name) {
            return Err(());
        }
        // Wrappers hide a command in their arguments — extract it for bridging,
        // or fall back when it cannot run faithfully in-process.
        match name {
            "find" => collect_find_commands(command, &mut names, builtins)?,
            "timeout" => collect_timeout_command(command, &mut names, builtins)?,
            "xargs" => collect_xargs_command(command, &mut names, builtins)?,
            "watch" => return Err(()), // its stub never runs the wrapped command
            "parallel" => return Err(()), // its stub only reports a dry-run plan
            "env" if env_would_run_command(command)? => return Err(()), // stub refuses commands
            _ => {}
        }
        push_external(name, &mut names, builtins);
    }
    Ok(names)
}

/// Names that must never be bridged: opaque builtins (`command`, `exec`),
/// interpreter re-entries (`eval`/`source`/`.`/`bash`/`sh`), path-based
/// (`./s.sh`) and glob-shaped (`$TOOL`, `x*`) names. `[` is exempt — its
/// name is literally `[`.
///
/// The other wrappers in bashkit's `analysis::COMMAND_WRAPPERS` — `doas`,
/// `nice`, `nohup`, `setsid`, `stdbuf`, `sudo` — need no special handling:
/// they are not builtins, so bridging the wrapper itself runs the host
/// binary, which spawns the wrapped command exactly like real bash does.
fn is_unbridgeable(name: &str) -> bool {
    matches!(
        name,
        "command" | "exec" | "eval" | "source" | "." | "bash" | "sh"
    ) || name.contains('/')
        || (name != "[" && name.contains(['$', '`', '*', '?', '[']))
}

/// Append `name` once, unless it is a builtin — names often appear both
/// literally and wrapped.
fn push_external(name: &str, names: &mut Vec<String>, builtins: &HashSet<String>) {
    if !builtins.contains(name) && !names.iter().any(|n| n == name) {
        names.push(name.to_string());
    }
}

/// Register a wrapped command name; `None` (no command there) is fine, but a
/// non-literal or unbridgeable name forces a fallback.
fn push_wrapped_arg(
    arg: Option<&str>,
    names: &mut Vec<String>,
    builtins: &HashSet<String>,
) -> Result<(), ()> {
    let Some(cmd) = arg else {
        return Ok(()); // no command — wrapper default or bashkit's own error
    };
    if is_unbridgeable(cmd) {
        return Err(()); // cannot bridge — fall back
    }
    push_external(cmd, names, builtins);
    Ok(())
}

/// True when `env` would run a command — its stub refuses, so the script
/// falls back to real bash (print mode stays in-process).
fn env_would_run_command(command: &AnalyzedCommand) -> Result<bool, ()> {
    for arg in command.literal_args().ok_or(())? {
        if arg == "-u" {
            return Err(()); // bashkit's stub errors on `-u` — fall back
        }
        if !(arg == "-i" || arg == "--ignore-environment" || arg.contains('=')) {
            return Ok(true); // COMMAND
        }
    }
    Ok(false) // print/assignment mode — the stub is faithful
}

/// Option surface of a bashkit wrapper builtin, mirroring its parser.
struct WrapperOpts {
    /// Flags with a separate value (`-k 5`, `--max-procs 4`).
    with_value: &'static [&'static str],
    /// Prefixes of attached-value flags (`-n5`, `--max-procs=4`).
    attached: &'static [&'static str],
    /// Flags consumed as-is (`--preserve-status`, `-0`).
    plain: &'static [&'static str],
    /// Skip unknown flags (timeout); otherwise they make bashkit error out.
    lenient: bool,
}

const TIMEOUT_OPTS: WrapperOpts = WrapperOpts {
    with_value: &["-k", "-s"],
    attached: &[],
    plain: &["--preserve-status"],
    lenient: true,
};

const XARGS_OPTS: WrapperOpts = WrapperOpts {
    with_value: &["-I", "-n", "-d", "-P", "--max-procs", "--process-slot-var"],
    attached: &[
        "-I",
        "-n",
        "-d",
        "-P",
        "--max-procs=",
        "--process-slot-var=",
    ],
    plain: &["-0", "--help", "--version"],
    lenient: false,
};

/// Scan past the wrapper's option/value args; returns the COMMAND position.
/// `Err` when an unknown option would make bashkit fail before dispatching.
fn skip_options(args: &[&str], opts: &WrapperOpts) -> Result<usize, ()> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if opts.with_value.contains(&arg) {
            i += 2; // flag + value
        } else if opts.attached.iter().any(|f| arg.starts_with(f)) {
            i += 1; // attached value (`-n5`, `--max-procs=4`)
        } else if opts.plain.contains(&arg) {
            i += 1;
        } else if arg.len() > 1 && arg.starts_with('-') {
            if !opts.lenient {
                return Err(()); // unknown option — bashkit errors; fall back
            }
            if arg.as_bytes()[1].is_ascii_digit() {
                break; // negative-looking DURATION (timeout)
            }
            i += 1; // timeout skips unknown flags
        } else {
            break; // COMMAND position
        }
    }
    Ok(i)
}

/// `timeout [OPTION] DURATION COMMAND [ARG]...` — register the wrapped
/// COMMAND (a missing one is bashkit's error to report).
fn collect_timeout_command(
    command: &AnalyzedCommand,
    names: &mut Vec<String>,
    builtins: &HashSet<String>,
) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    let i = skip_options(&args, &TIMEOUT_OPTS)?;
    push_wrapped_arg(args.get(i + 1).copied(), names, builtins)
}

/// `xargs [OPTION]... [COMMAND [ARG]...]` — register the wrapped COMMAND
/// (bashkit defaults to the `echo` builtin when absent).
fn collect_xargs_command(
    command: &AnalyzedCommand,
    names: &mut Vec<String>,
    builtins: &HashSet<String>,
) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    let i = skip_options(&args, &XARGS_OPTS)?;
    push_wrapped_arg(args.get(i).copied(), names, builtins)
}

/// `find [PATH]... [EXPRESSION]` — register the command of every
/// `-exec`/`-execdir` template (first template arg, up to `;`/`\;`/`+`).
/// Unknown predicates make bashkit fail before dispatching → fall back.
fn collect_find_commands(
    command: &AnalyzedCommand,
    names: &mut Vec<String>,
    builtins: &HashSet<String>,
) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        match arg {
            "-name" | "-path" | "-type" | "-maxdepth" | "-mindepth" | "-printf" => i += 2,
            "-print" | "-print0" | "-not" | "!" => i += 1,
            "-exec" | "-execdir" => {
                let mut cmd = None;
                i += 1;
                while i < args.len() && !matches!(args[i], ";" | "\\;" | "+") {
                    cmd.get_or_insert(args[i]);
                    i += 1;
                }
                if let Some(cmd) = cmd {
                    push_wrapped_arg(Some(cmd), names, builtins)?;
                }
                i += 1; // past the terminator
            }
            _ if arg.len() > 1 && arg.starts_with('-') => {
                return Err(()); // unknown predicate (`-delete`, `-ok`, …)
            }
            _ => i += 1, // search path
        }
    }
    Ok(())
}

/// Execute `command` through the in-process bashkit interpreter.
///
/// `external_names` are bridged to host executables. The whole script shares
/// one deadline (`timeout`) plus the caller's cancel flag. When `sink` is
/// set, output streams live: host commands via their pipe drains, and
/// builtin-only scripts via the interpreter callback plus a flush ticker
/// (which also covers quiet stretches after output).
///
/// # Panics
///
/// Uses `Handle::block_on`, so it must run on a blocking thread inside the
/// runtime context (`tokio::task::spawn_blocking`), never from an async task
/// or outside a runtime. Keep every call site inside `spawn_blocking`.
pub(crate) fn execute(
    command: &str,
    workspace: &Path,
    timeout: Duration,
    cancel: &AtomicBool,
    external_names: Vec<String>,
    sink: Option<OutputSink>,
) -> Result<String, String> {
    let shared_cancel = Arc::new(AtomicBool::new(false));
    let deadline_ms = Arc::new(AtomicU64::new(
        now_ms().saturating_add(timeout.as_millis() as u64),
    ));

    // One script-level forwarder, so the byte cap applies per script, not per command.
    let forwarder = sink.map(|s| Arc::new(Mutex::new(ChunkForwarder::new(s))));

    let mut bash = build_bash(
        workspace,
        timeout + Duration::from_secs(1), // backstop; outer select fires first
        &external_names,
        Arc::clone(&deadline_ms),
        Arc::clone(&shared_cancel),
        forwarder.clone(),
    );

    let handle = tokio::runtime::Handle::current();
    let result = handle
        .block_on(async {
            tokio::select! {
                result = run_script(&mut bash, command, &external_names, &forwarder) => {
                    result.map_err(|e| e.to_string())
                }
                _ = tokio::time::sleep(timeout) => {
                    Err(format!("Command timed out after {}ms", timeout.as_millis()))
                }
                _ = sync_cancel(cancel, &shared_cancel) => Err("Cancelled by user".to_string()),
            }
        })
        .map(|result| format_exec_result(&result));

    // Flush carried/coalesced bytes now that the script is done.
    if let Some(f) = &forwarder {
        super::lock(f).finish();
    }
    result
}

/// Streaming callback route, used only for builtin-only scripts: bridged host
/// commands already stream via their pipe drains, and the callback would
/// re-emit their output a second time at command end.
async fn run_script(
    bash: &mut Bash,
    command: &str,
    external_names: &[String],
    forwarder: &Option<Arc<Mutex<ChunkForwarder>>>,
) -> Result<ExecResult, bashkit::Error> {
    if external_names.is_empty()
        && let Some(f) = forwarder
    {
        let cb_forwarder = Arc::clone(f);
        // Flush time-due chunks during quiet stretches (this route has no
        // pipe drains to drive the flush).
        let _ticker = FlushTicker(tokio::spawn(flush_ticker(Arc::clone(f))));
        bash.exec_streaming(
            command,
            Box::new(move |stdout, stderr| {
                let mut guard = super::lock(&cb_forwarder);
                guard.push(stdout.as_bytes());
                guard.push(stderr.as_bytes());
            }),
        )
        .await
    } else {
        bash.exec(command).await
    }
}

/// Periodically tick the forwarder while a builtin-only script runs.
async fn flush_ticker(forwarder: Arc<Mutex<ChunkForwarder>>) {
    loop {
        tokio::time::sleep(super::COALESCE_MS).await;
        super::lock(&forwarder).tick();
    }
}

/// Abort-on-drop guard so the ticker never outlives the script (including
/// when the enclosing future is dropped on timeout/cancel).
struct FlushTicker(tokio::task::JoinHandle<()>);

impl Drop for FlushTicker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Poll the tool's cancel flag; when set, mirror it into `shared` and complete.
async fn sync_cancel(cancel: &AtomicBool, shared: &Arc<AtomicBool>) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            shared.store(true, Ordering::Relaxed);
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Build a bashkit `Bash` wired to the real workspace.
///
/// Applies mount table (`mounts`), seeds env, and bridges external command names.
fn build_bash(
    workspace: &Path,
    timeout: Duration,
    external_names: &[String],
    deadline_ms: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    forwarder: Option<Arc<Mutex<ChunkForwarder>>>,
) -> Bash {
    let home_mount = real_home_mount();
    let workspace_vfs = super::convert_path_to_unix_style(workspace);
    let mount_specs = mounts(workspace, home_mount.as_ref());
    let shared_mounts: Arc<[RealMount]> = Arc::from(mount_specs.clone());

    let mut builder = Bash::builder()
        .cwd(PathBuf::from(&workspace_vfs))
        .allowed_mount_paths(
            mount_specs
                .iter()
                .map(|m| m.host_path.clone())
                .collect::<Vec<_>>(),
        );
    for m in mount_specs {
        builder = if m.writable {
            builder.mount_real_readwrite_at(m.host_path, m.vfs_path)
        } else {
            builder.mount_real_readonly_at(m.host_path, m.vfs_path)
        };
    }

    builder = builder.limits(
        ExecutionLimits::default()
            .timeout(timeout)
            .max_commands(1_000_000)
            .max_stdout_bytes(MAX_STREAM_BYTES)
            .max_stderr_bytes(MAX_STREAM_BYTES),
    );

    // Seed env from host; HOME comes from the VFS home mount so `~` expands to
    // a clean POSIX path (the host's is often a Windows `C:\` form). Secret
    // vars (names ending in `API_KEY`) are withheld from the interpreter.
    for (key, value) in std::env::vars() {
        if key != "HOME" && !super::is_secret_env_key(&key) {
            builder = builder.env(key, value);
        }
    }
    if let Some(home) = &home_mount {
        builder = builder.env("HOME", home.vfs_path.to_string_lossy().into_owned());
    }

    // Embedded Python (Monty) registers `python`/`python3` builtins and is
    // runtime-gated in bashkit; Must come after the host-env seeding above.
    builder = builder.python().env("BASHKIT_ALLOW_INPROCESS_PYTHON", "1");

    for name in external_names {
        builder = builder.builtin(
            name.clone(),
            Box::new(HostCommandBuiltin {
                name: name.clone(),
                mounts: Arc::clone(&shared_mounts),
                cancel: Arc::clone(&cancel),
                deadline_ms: Arc::clone(&deadline_ms),
                timeout,
                forwarder: forwarder.clone(),
                home: home_mount.clone(),
            }),
        );
    }
    // bashkit warns on stderr per read-write mount; silence stderr while
    // building (locked so concurrent builds can't swap each other's handles).
    let _lock = SILENCER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _silence = stderr_silencer::StderrSilencer::new();
    builder.build()
}

/// Serializes `Bash::build` — the stderr swap is process-wide, so concurrent
/// silencers would save and restore each other's handles.
static SILENCER_LOCK: Mutex<()> = Mutex::new(());

/// Builtin that executes a host command directly (no bash involved).
struct HostCommandBuiltin {
    name: String,
    /// VFS mount table, shared with `build_bash` via Arc.
    mounts: Arc<[RealMount]>,
    cancel: Arc<AtomicBool>,
    deadline_ms: Arc<AtomicU64>,
    /// Total user-visible timeout for the whole script (used in error messages).
    timeout: Duration,
    /// Script-level output forwarder; chunks stream live via the pipe drains.
    forwarder: Option<Arc<Mutex<ChunkForwarder>>>,
    /// Seeded home mount; maps the interpreter's VFS HOME to the host path.
    home: Option<RealMount>,
}

/// Mirror the script env (`export`, prefix assignments, `unset`) into the
/// child, remapping the seeded VFS HOME to its host path and dropping
/// secrets + rustup's recursion counter.
fn apply_child_env(
    cmd: &mut std::process::Command,
    env: &HashMap<String, String>,
    home: Option<&RealMount>,
) {
    cmd.env_clear();
    for (key, value) in env {
        // Remap the seeded VFS HOME to the host path (script-assigned HOME passes through).
        if key == "HOME"
            && let Some(home) = home
            && value.as_str() == home.vfs_path.to_string_lossy()
        {
            cmd.env("HOME", &home.host_path);
        } else if key != "RUST_RECURSION_COUNT" && !super::is_secret_env_key(key) {
            cmd.env(key, value);
        }
    }
    // No host home: keep the inherited HOME as a fallback.
    if home.is_none()
        && let Some(home_dir) = std::env::var_os("HOME")
    {
        cmd.env("HOME", home_dir);
    }
}

#[async_trait]
impl Builtin for HostCommandBuiltin {
    async fn execute(&self, ctx: BuiltinContext<'_>) -> bashkit::Result<ExecResult> {
        if self.cancel.load(Ordering::Relaxed) {
            // Record cancel as a failed command (130), NOT a script abort:
            // `ExecResult::err` keeps the script running, like real bash after
            // a non-zero exit. The whole-script abort comes from the outer
            // `select!` in `execute`, which drops the interpreter within ~50ms.
            return Ok(ExecResult::err("Cancelled by user", 130));
        }
        let remaining = remaining_timeout(self.deadline_ms.load(Ordering::Relaxed));
        if remaining.is_zero() {
            return Ok(ExecResult::err(
                format!("Command timed out after {}ms", self.timeout.as_millis()),
                124,
            ));
        }

        let prepared = (|| -> Result<_, String> {
            let mut cmd = std::process::Command::new(&self.name);
            cmd.args(ctx.args);
            cmd.current_dir(resolve_cwd(ctx.cwd, &self.mounts)?);
            apply_child_env(&mut cmd, ctx.env, self.home.as_ref());
            let stdin_writer = match ctx.stdin {
                Some(data) => {
                    let (stdin_tx, stdin_rx) = create_pipe_pair("stdin")?;
                    set_sender_nonblocking(&stdin_tx)?;
                    set_sender_noninheritable(&stdin_tx)?;
                    cmd.stdin(pipe_to_stdio(stdin_rx));
                    Some((data.as_bytes().to_vec(), stdin_tx))
                }
                None => {
                    cmd.stdin(Stdio::null());
                    None
                }
            };

            let (stdout_tx, stdout_rx) = create_pipe_pair("stdout")?;
            let (stderr_tx, stderr_rx) = create_pipe_pair("stderr")?;
            cmd.stdout(pipe_to_stdio(stdout_tx));
            cmd.stderr(pipe_to_stdio(stderr_tx));

            super::detach_child(&mut cmd);
            Ok((cmd, stdin_writer, stdout_rx, stderr_rx))
        })();
        let (mut cmd, stdin_writer, stdout_rx, stderr_rx) = match prepared {
            Ok(prepared) => prepared,
            Err(e) => return Ok(ExecResult::err(e, 1)),
        };

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => return Ok(ExecResult::err(format!("bash: {}: {e}", self.name), 127)),
        };

        // Feed stdin in a side thread: non-blocking writes with a 5s deadline,
        // so a surviving grandchild holding the pipe open can't hang the script.
        if let Some((data, mut stdin_tx)) = stdin_writer {
            std::thread::spawn(move || {
                use std::io::Write;
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut written = 0;
                while written < data.len() && Instant::now() < deadline {
                    match stdin_tx.write(&data[written..]) {
                        Ok(0) => break,
                        Ok(n) => written += n,
                        Err(e) if is_would_block(&e) => {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break, // EPIPE — reader gone
                    }
                }
            });
        }

        let cancel = Arc::clone(&self.cancel);
        let timeout = self.timeout;
        let forwarder = self.forwarder.clone();
        // spawn_blocking so the outer select can still observe timeout/cancel.
        let result = tokio::task::spawn_blocking(move || {
            // No contention: callback/ticker routes never coexist with host
            // commands, so holding the script-level lock is safe.
            let mut guard = forwarder.as_ref().map(|f| super::lock(f));
            wait_with_timeout(
                child,
                Some(stdout_rx),
                Some(stderr_rx),
                remaining,
                timeout,
                true,
                cancel.as_ref(),
                guard.as_deref_mut(),
            )
        })
        .await
        .unwrap_or_else(|e| Err(WaitError::Other(format!("Host command task panicked: {e}"))));

        match result {
            Ok(output) => Ok(ExecResult {
                stdout: output.stdout.into(),
                stderr: output.stderr.into(),
                exit_code: super::exit_code_of(&output.status),
                ..Default::default()
            }),
            Err(e) => {
                let code = e.exit_code();
                Ok(ExecResult::err(e.into_message(), code))
            }
        }
    }
}

/// Match a VFS path against a mount's VFS prefix; returns the corresponding
/// host path when the prefix matches, `None` otherwise.
fn match_mount(vfs_cwd: &Path, vfs_prefix: &str, host_root: &Path) -> Option<PathBuf> {
    let mut components = vfs_cwd.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let prefix_parts: Vec<&str> = vfs_prefix.split('/').filter(|p| !p.is_empty()).collect();
    for expected in &prefix_parts {
        match components.next() {
            Some(Component::Normal(actual)) if actual == *expected => continue,
            _ => return None,
        }
    }
    Some(host_root.join(components.as_path()))
}

/// Translate the bashkit VFS cwd to the real host path via the mount table.
fn resolve_cwd(vfs_cwd: &Path, mount_specs: &[RealMount]) -> Result<PathBuf, String> {
    for m in mount_specs {
        if let Some(host) = match_mount(vfs_cwd, &m.vfs_path.to_string_lossy(), &m.host_path) {
            return Ok(host);
        }
    }
    Err(format!(
        "host command outside mapped cwd: {}",
        vfs_cwd.display()
    ))
}

/// VFS mount table in match order (specific mounts before broad ones).
/// Single source of truth for `build_bash` and `HostCommandBuiltin`.
fn mounts(workspace: &Path, home: Option<&RealMount>) -> Vec<RealMount> {
    let mut list = vec![
        RealMount::rw(workspace, super::convert_path_to_unix_style(workspace)),
        RealMount::rw(std::env::temp_dir(), "/tmp"),
    ];
    if let Some(home) = home {
        list.insert(1, home.clone());
    }
    list.extend(readonly_roots());
    list
}

/// Read-only fallback mounts, appended last: `/` on Unix, every present
/// drive at its drive-letter path (`/c`, `/d`, …) on Windows — no `/`
/// catch-all there, so unmapped paths error instead of being mangled.
#[cfg(unix)]
fn readonly_roots() -> Vec<RealMount> {
    vec![RealMount::ro("/", "/")]
}

/// Every present drive mounted read-only at its drive-letter VFS path
/// (`C:\` → `/c`), like `convert_path_to_unix_style` produces.
#[cfg(windows)]
fn readonly_roots() -> Vec<RealMount> {
    present_drive_letters()
        .into_iter()
        .map(|letter| {
            RealMount::ro(
                format!("{letter}:\\"),
                format!("/{}", letter.to_ascii_lowercase()),
            )
        })
        .collect()
}

/// Letters of every drive present on this host with a usable root.
/// `GetDriveTypeW` never probes media, so drives that still fail to open
/// (empty card readers, stale network shares) are filtered by bashkit's
/// build-time canonicalize.
#[cfg(windows)]
fn present_drive_letters() -> Vec<char> {
    const DRIVE_UNKNOWN: u32 = 0;
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(lp_root_path_name: *const u16) -> u32;
    }
    // SAFETY: GetLogicalDrives takes no arguments; bit i is drive 'A' + i.
    let mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|&i| mask & (1 << i) != 0)
        .map(|i| char::from(b'A' + i as u8))
        .filter(|&letter| {
            let root = format!("{letter}:\\");
            let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
            // SAFETY: `root` is a valid drive root; the result is a plain u32.
            let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
            !matches!(kind, DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR)
        })
        .collect()
}

/// A real host directory mounted into the VFS (see [`mounts`]).
#[derive(Clone)]
struct RealMount {
    host_path: PathBuf,
    vfs_path: PathBuf,
    writable: bool,
}

impl RealMount {
    fn rw(host_path: impl Into<PathBuf>, vfs_path: impl Into<PathBuf>) -> Self {
        Self {
            host_path: host_path.into(),
            vfs_path: vfs_path.into(),
            writable: true,
        }
    }

    fn ro(host_path: impl Into<PathBuf>, vfs_path: impl Into<PathBuf>) -> Self {
        Self {
            host_path: host_path.into(),
            vfs_path: vfs_path.into(),
            writable: false,
        }
    }
}

/// Resolve the real host home directory and its POSIX VFS mount path.
fn real_home_mount() -> Option<RealMount> {
    let host_home = home_host_path()?;
    let vfs_path = super::convert_path_to_unix_style(&host_home);
    Some(RealMount::rw(host_home, vfs_path))
}

/// Resolve the real host home directory, preferring `$HOME` then `USERPROFILE`.
fn home_host_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        // Git Bash exports HOME as an MSYS path like `/c/Users/...`.
        #[cfg(windows)]
        if let Some(win) = super::convert_path_to_windows_style(&home)
            && win.is_dir()
        {
            return Some(win);
        }
        let p = PathBuf::from(&home);
        if p.is_absolute() && p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(&profile);
        if p.is_absolute() && p.is_dir() {
            return Some(p);
        }
    }
    None
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn remaining_timeout(deadline_ms: u64) -> Duration {
    Duration::from_millis(deadline_ms.saturating_sub(now_ms()))
}

/// Format an `ExecResult` like `format_command_output`, surfacing bashkit's
/// head-only truncation explicitly so the final truncation marker stays accurate.
fn format_exec_result(result: &ExecResult) -> String {
    let mut output = super::combine_output(&result.stdout, &result.stderr, result.exit_code);
    if result.stdout_truncated || result.stderr_truncated {
        if !output.is_empty() {
            output.push('\n');
        }
        let _ = std::fmt::Write::write_fmt(
            &mut output,
            format_args!(
                "[bashkit truncated output at {MAX_STREAM_BYTES} bytes per stream — tail lost]"
            ),
        );
    }
    super::truncate_output(output)
}

/// Drain a captured-stderr pipe, re-emitting everything except bashkit
/// warnings expected by design: writable mounts (cannot be silenced) and
/// drive roots that failed to mount (unreadable media probed every build).
fn drain_and_reemit(mut file: std::fs::File) {
    const EXPECTED: [&str; 2] = [
        "bashkit: warning: writable mount",
        "bashkit: warning: failed to canonicalize mount path",
    ];
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut file, &mut buf);
    for line in String::from_utf8_lossy(&buf).lines() {
        if !EXPECTED.iter().any(|p| line.starts_with(*p)) {
            eprintln!("{line}");
        }
    }
}

/// Swap stderr to a pipe during `Bash::build`; on drop, restore stderr and
/// re-emit the captured output via `drain_and_reemit`.
#[cfg(unix)]
mod stderr_silencer {
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};

    pub(super) struct StderrSilencer {
        saved: Option<OwnedFd>,
        read_end: Option<OwnedFd>,
    }

    impl StderrSilencer {
        pub(super) fn new() -> Self {
            let mut fds = [0i32; 2];
            // SAFETY: pipe() creates two fresh fds, wrapped in OwnedFd at once.
            let (read_end, write_end) = if unsafe { libc::pipe(fds.as_mut_ptr()) } == 0 {
                unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
            } else {
                return Self {
                    saved: None,
                    read_end: None,
                };
            };
            // SAFETY: dup() duplicates the stderr fd.
            let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
            if saved < 0 {
                return Self {
                    saved: None,
                    read_end: None,
                }; // write_end closed here
            }
            // SAFETY: saved is a fresh fd, now owned.
            let saved = unsafe { OwnedFd::from_raw_fd(saved) };
            // SAFETY: dup2 on raw fds; STDERR_FILENO then holds a dup of the
            // write end, so dropping `write_end` keeps the pipe open.
            unsafe { libc::dup2(write_end.as_raw_fd(), libc::STDERR_FILENO) };
            drop(write_end);
            Self {
                saved: Some(saved),
                read_end: Some(read_end),
            }
        }
    }

    impl Drop for StderrSilencer {
        fn drop(&mut self) {
            // Restore stderr — this dup2 also closes the pipe write end held
            // in STDERR_FILENO, so the drain below reaches EOF.
            if let Some(saved) = self.saved.take() {
                // SAFETY: saved is the fd captured in `new`.
                unsafe { libc::dup2(saved.as_raw_fd(), libc::STDERR_FILENO) };
                // saved (OwnedFd) dropped here → closes the duplicate.
            }
            if let Some(read_end) = self.read_end.take() {
                super::drain_and_reemit(std::fs::File::from(read_end));
            }
        }
    }
}

/// Same as the Unix variant via `SetStdHandle` — std re-queries
/// `STD_ERROR_HANDLE` per write, so the swap takes effect immediately.
#[cfg(windows)]
mod stderr_silencer {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    unsafe extern "system" {
        fn CreatePipe(
            hreadpipe: *mut isize,
            hwritepipe: *mut isize,
            lppipeattributes: *mut std::ffi::c_void,
            nsize: u32,
        ) -> i32;
        fn GetStdHandle(n_std_handle: u32) -> isize;
        fn SetStdHandle(n_std_handle: u32, handle: isize) -> i32;
    }
    const STD_ERROR_HANDLE: u32 = (-12i32) as u32;

    pub(super) struct StderrSilencer {
        saved: Option<isize>,
        read_end: Option<OwnedHandle>,
        /// Held open during `build()`; closed on drop before reading the pipe.
        write_end: Option<OwnedHandle>,
    }

    impl StderrSilencer {
        pub(super) fn new() -> Self {
            let mut read = 0isize;
            let mut write = 0isize;
            // SAFETY: CreatePipe with valid out-pointers; null attrs → defaults.
            if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null_mut(), 0) } == 0 {
                return Self {
                    saved: None,
                    read_end: None,
                    write_end: None,
                };
            }
            // SAFETY: handles are freshly created, wrapped in OwnedHandle.
            let read_end = unsafe { OwnedHandle::from_raw_handle(read as *mut _) };
            let write_end = unsafe { OwnedHandle::from_raw_handle(write as *mut _) };
            // SAFETY: GetStdHandle/SetStdHandle with valid constants.
            let saved = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
            unsafe { SetStdHandle(STD_ERROR_HANDLE, write_end.as_raw_handle() as isize) };
            Self {
                saved: Some(saved),
                read_end: Some(read_end),
                write_end: Some(write_end),
            }
        }
    }

    impl Drop for StderrSilencer {
        fn drop(&mut self) {
            // Restore stderr first, then close the write end so reads reach EOF.
            if let Some(saved) = self.saved.take() {
                // SAFETY: restores the handle captured in `new`.
                unsafe { SetStdHandle(STD_ERROR_HANDLE, saved) };
            }
            self.write_end.take();
            if let Some(read_end) = self.read_end.take() {
                super::drain_and_reemit(std::fs::File::from(read_end));
            }
        }
    }
}
