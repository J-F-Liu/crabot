//! In-process bashkit interpreter for the `bash` tool.
//!
//! External commands are bridged to host executables via [`HostCommandBuiltin`]
//! — no real bash process, so the same path works natively on Windows. Scripts
//! the interpreter cannot faithfully handle (parse errors, dynamic command
//! names, `eval`/`exec`/`source`, path-based or glob-shaped names) make
//! [`collect_external_names`] return `Err`, falling back to real `bash -c`.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bashkit::analysis::analyze_with_limits;
use bashkit::{Bash, Builtin, BuiltinContext, ExecResult, ExecutionLimits, async_trait};

use super::{
    WaitError, create_pipe_pair, is_would_block, pipe_to_stdio, set_sender_nonblocking,
    set_sender_noninheritable, wait_with_timeout,
};

/// Per-stream output cap (head-only backstop; crabot's own truncation is the visible limit).
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;

/// Builtins whose payload is opaque to static analysis (`command cmd`, `exec cmd`).
/// `eval`/`source`/`.`/`bash`/`sh` are handled by bashkit's `is_interpreter_reentry`.
/// Re-verify when bumping the pinned bashkit version (0.15.0).
const OPAQUE_BUILTINS: &[&str] = &["command", "exec"];

/// Cached set of every builtin this bashkit build can dispatch.
pub(crate) fn builtin_names() -> &'static HashSet<String> {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| Bash::new().builtin_names().into_iter().collect())
}

/// Statically collect the external command names a script executes.
///
/// Returns `Err(())` when the script cannot run faithfully in-process (parse
/// error, dynamic/path-based/glob-shaped command names, opaque builtins).
/// Otherwise returns the names to bridge (empty when only builtins are used).
pub(crate) fn collect_external_names(script: &str) -> Result<Vec<String>, ()> {
    let analysis = analyze_with_limits(script, 100, 100_000).map_err(|_| ())?;
    if analysis.is_opaque() {
        return Err(());
    }
    let builtins = builtin_names();
    let mut names = Vec::new();
    for name in analysis.command_names() {
        // Unsupported: opaque payload, path-based (`./script.sh`), or glob-shaped
        // (`$TOOL`, `tool*`). `[` is exempt — its name is literally `[`.
        if OPAQUE_BUILTINS.contains(&name)
            || name.contains('/')
            || (name != "[" && name.contains(['$', '`', '*', '?', '[']))
        {
            return Err(());
        }
        if !builtins.contains(name) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Execute `command` through the in-process bashkit interpreter.
///
/// `external_names` are bridged to host executables. The whole script shares one
/// deadline (`timeout`) plus the caller's cancel flag.
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
) -> Result<String, String> {
    let shared_cancel = Arc::new(AtomicBool::new(false));
    let deadline_ms = Arc::new(AtomicU64::new(
        now_ms().saturating_add(timeout.as_millis() as u64),
    ));

    let mut bash = build_bash(
        workspace,
        timeout + Duration::from_secs(1), // backstop; outer select fires first
        &external_names,
        Arc::clone(&deadline_ms),
        Arc::clone(&shared_cancel),
    );

    let handle = tokio::runtime::Handle::current();
    handle
        .block_on(async {
            tokio::select! {
                result = bash.exec(command) => result.map_err(|e| e.to_string()),
                _ = tokio::time::sleep(timeout) => {
                    Err(format!("Command timed out after {}ms", timeout.as_millis()))
                }
                _ = sync_cancel(cancel, &shared_cancel) => Err("Cancelled by user".to_string()),
            }
        })
        .map(|result| format_exec_result(&result))
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
/// Mounts: host filesystem root read-only (copy-on-write overlay), real home
/// read-write with `$HOME` pointed at it, and workspace read-write directly
/// (the overlay's upper layer would swallow `echo > file` writes).
fn build_bash(
    workspace: &Path,
    timeout: Duration,
    external_names: &[String],
    deadline_ms: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
) -> Bash {
    let workspace_root = host_filesystem_root(workspace);
    let home_mount = real_home_mount();
    let workspace_vfs = super::convert_path_to_unix_style(workspace);

    let mut allowed = vec![workspace_root.clone(), workspace.to_path_buf()];
    if let Some(home) = &home_mount {
        allowed.push(home.host_path.clone());
    }

    let mut builder = Bash::builder()
        .cwd(PathBuf::from(&workspace_vfs))
        .allowed_mount_paths(allowed)
        .mount_real_readonly(workspace_root);

    if let Some(home) = &home_mount {
        builder = builder.mount_real_readwrite_at(home.host_path.clone(), home.vfs_path.clone());
    }

    builder = builder
        .mount_real_readwrite_at(workspace.to_path_buf(), workspace_vfs)
        .limits(
            ExecutionLimits::default()
                .timeout(timeout)
                .max_commands(1_000_000)
                .max_stdout_bytes(MAX_STREAM_BYTES)
                .max_stderr_bytes(MAX_STREAM_BYTES),
        );

    // Seed env from host; HOME comes from the VFS home mount so `~` expands to
    // a clean POSIX path (the host's is often a Windows `C:\` form).
    for (key, value) in std::env::vars() {
        if key != "HOME" {
            builder = builder.env(key, value);
        }
    }
    if let Some(home) = &home_mount {
        builder = builder.env("HOME", home.vfs_path.to_string_lossy().into_owned());
    }

    for name in external_names {
        builder = builder.builtin(
            name.clone(),
            Box::new(HostCommandBuiltin {
                name: name.clone(),
                workspace: workspace.to_path_buf(),
                home: home_mount.clone(),
                cancel: Arc::clone(&cancel),
                deadline_ms: Arc::clone(&deadline_ms),
                timeout,
            }),
        );
    }
    builder.build()
}

/// Builtin that executes a host command directly (no bash involved).
struct HostCommandBuiltin {
    name: String,
    workspace: PathBuf,
    /// Real home mount (host + VFS paths), for mapping `cd ~` back to the host.
    home: Option<RealMount>,
    cancel: Arc<AtomicBool>,
    deadline_ms: Arc<AtomicU64>,
    /// Total user-visible timeout for the whole script (used in error messages).
    timeout: Duration,
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
            cmd.env_remove("RUST_RECURSION_COUNT"); // rustup proxies abort past their counter max
            cmd.current_dir(vfs_cwd_to_host(
                ctx.cwd,
                &self.workspace,
                self.home.as_ref(),
            )?);

            let stdin_writer = match ctx.stdin {
                Some(data) => {
                    let (stdin_tx, stdin_rx) = create_pipe_pair("stdin")?;
                    set_sender_nonblocking(&stdin_tx)?;
                    set_sender_noninheritable(&stdin_tx)?;
                    cmd.stdin(pipe_to_stdio(stdin_rx));
                    Some((data.to_owned(), stdin_tx))
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
                    match stdin_tx.write(&data.as_bytes()[written..]) {
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
        // spawn_blocking so the outer select can still observe timeout/cancel.
        let result = tokio::task::spawn_blocking(move || {
            wait_with_timeout(
                child,
                Some(stdout_rx),
                Some(stderr_rx),
                remaining,
                timeout,
                true,
                cancel.as_ref(),
            )
        })
        .await
        .unwrap_or_else(|e| Err(WaitError::Other(format!("Host command task panicked: {e}"))));

        match result {
            Ok(output) => Ok(ExecResult {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
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

/// Map a bashkit VFS cwd back to the real host path.
///
/// The cwd is matched against the workspace and home mounts first, then the
/// read-only root mount. A cwd matching none is an error: silently falling
/// back to the workspace would run the command in the wrong directory.
///
/// `pub` only so `tests/bash.rs` can exercise the mapping.
pub fn vfs_cwd_to_host(
    vfs_cwd: &Path,
    workspace: &Path,
    home: Option<&RealMount>,
) -> Result<PathBuf, String> {
    let workspace_vfs = super::convert_path_to_unix_style(workspace);
    if let Some(host) = match_mount(vfs_cwd, &workspace_vfs, workspace) {
        return Ok(host);
    }
    if let Some(home) = home {
        let home_vfs = home.vfs_path.to_string_lossy();
        if let Some(host) = match_mount(vfs_cwd, &home_vfs, &home.host_path) {
            return Ok(host);
        }
    }
    // Root mount: strip the leading `/` and join onto the host filesystem root
    // (identity on Unix; the workspace's drive root on Windows).
    if vfs_cwd.components().next() == Some(Component::RootDir) {
        let rest = vfs_cwd.strip_prefix("/").unwrap_or(vfs_cwd);
        return Ok(host_filesystem_root(workspace).join(rest));
    }
    Err(format!(
        "host command outside mapped cwd: {}",
        vfs_cwd.display()
    ))
}

/// Host filesystem root backing the VFS root overlay: `/` on Unix; the workspace's
/// drive root on Windows so the whole drive is reachable.
fn host_filesystem_root(workspace: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/")
    }
    #[cfg(windows)]
    {
        for comp in workspace.components() {
            if let Component::Prefix(prefix) = comp {
                let mut root = PathBuf::from(prefix.as_os_str());
                root.push("\\");
                return root;
            }
        }
        PathBuf::from("\\")
    }
}

/// A real host directory mounted into the VFS.
///
/// `pub` only so `tests/bash.rs` can build a fake home mount.
#[derive(Clone)]
pub struct RealMount {
    pub host_path: PathBuf,
    pub vfs_path: PathBuf,
}

/// Resolve the real host home directory and its POSIX VFS mount path.
fn real_home_mount() -> Option<RealMount> {
    let host_home = home_host_path()?;
    let vfs_path = PathBuf::from(super::convert_path_to_unix_style(&host_home));
    Some(RealMount {
        host_path: host_home,
        vfs_path,
    })
}

/// Resolve the real host home directory, preferring `$HOME` then `USERPROFILE`.
fn home_host_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        #[cfg(windows)]
        if let Some(win) = msys_to_windows_path(&home)
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

/// Convert an MSYS-style POSIX path (`/c/Users/liujf`) to a Windows path (`C:\Users\liujf`).
#[cfg(windows)]
fn msys_to_windows_path(msys: &str) -> Option<PathBuf> {
    let trimmed = msys.trim_start_matches('/');
    let mut parts = trimmed.split('/');
    let first = parts.next()?;
    if first.len() == 1
        && first
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    {
        let mut win = PathBuf::from(format!("{}:\\", first.to_ascii_uppercase()));
        for p in parts {
            win.push(p);
        }
        return Some(win);
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
