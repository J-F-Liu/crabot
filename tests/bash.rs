//! Tests for the `bash` tool: the bashkit in-process interpreter route and
//! its `bash -c` fallback.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crabot::tools::ToolRegistry;

/// Helper: create a temp workspace dir that is cleaned up on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> io::Result<Self> {
        let mut dir = std::env::temp_dir();
        dir.push(format!("crabot_test_{}_{}", prefix, std::process::id()));
        let _ = fs::remove_dir_all(&dir); // clean any left‑over
        fs::create_dir_all(&dir)?;
        Ok(Self { path: dir })
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// ── helpers ─────────────────────────────────────────────────

/// Normalize Git Bash's POSIX-style PATH to Windows form (once per process) so
/// children like `bash`, `cargo`, and `git` resolve under `CreateProcess`.
#[cfg(windows)]
fn fix_test_path() {
    use std::sync::OnceLock;
    static FIXED: OnceLock<()> = OnceLock::new();
    FIXED.get_or_init(|| {
        let raw = std::env::var("PATH").unwrap_or_default();
        let windows: Vec<String> = if raw.contains(';') {
            raw.split(';')
                .filter(|e| !e.is_empty())
                .map(str::to_owned)
                .collect()
        } else {
            raw.split(':')
                .filter_map(|entry| {
                    if entry.is_empty() {
                        return None;
                    }
                    let entry = entry.replace('/', "\\");
                    if let Some(rest) = entry.strip_prefix('\\') {
                        if let Some((drive, tail)) = rest.split_once('\\') {
                            let drive = drive.to_ascii_uppercase();
                            if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
                                return Some(format!("{drive}:\\{tail}"));
                            }
                        }
                        return None; // unmapped POSIX path (e.g. /mingw64)
                    }
                    Some(entry)
                })
                .collect()
        };
        // SAFETY: write-once via OnceLock, and every child-spawning path calls
        // `fix_test_path` first, so no PATH lookup can race the write.
        unsafe {
            std::env::set_var("PATH", windows.join(";"));
        }
    });
}

/// No-op on non-Windows: PATH is already in the correct format.
#[cfg(not(windows))]
fn fix_test_path() {}

/// The crabot repository root (used as the test workspace where git exists).
fn crabot_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run the `bash` tool exactly like `llm.rs::exec_tool` does: on a blocking
/// thread with a tokio runtime context.
fn run_bash(command: &str, workspace: &Path, timeout_ms: Option<u64>) -> Result<String, String> {
    run_bash_with_cancel(command, workspace, timeout_ms, AtomicBool::new(false))
}

/// Like [`run_bash`], with a caller-controlled cancel flag.
fn run_bash_with_cancel(
    command: &str,
    workspace: &Path,
    timeout_ms: Option<u64>,
    cancel: AtomicBool,
) -> Result<String, String> {
    fix_test_path();
    let registry = ToolRegistry::new();
    let bash = registry.find_tool("bash").expect("bash tool registered");
    let mut args = serde_json::json!({ "command": command });
    if let Some(ms) = timeout_ms {
        args["timeout"] = serde_json::json!(ms);
    }
    let workspace = workspace.to_path_buf();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    rt.block_on(async {
        tokio::task::spawn_blocking(move || bash.execute(&args, &workspace, &cancel))
            .await
            .unwrap_or_else(|e| Err(format!("Tool execution panicked: {e}")))
    })
}

// ── external command bridge ─────────────────────────────────

/// `cargo --version && git --version` — external-command bridge + `&&` list.
#[test]
fn bashkit_external_bridge_and_list() {
    let result = run_bash(
        "cargo --version && git --version",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("cargo "), "unexpected: {result}");
    assert!(result.contains("git version"), "unexpected: {result}");
}

/// `if cargo --version; then echo ok; fi` — exit code drives control flow.
#[test]
fn bashkit_if_control_flow() {
    let result = run_bash(
        "if cargo --version; then echo ok; fi",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("ok"), "unexpected: {result}");
}

// ── bashkit syntax features ─────────────────────────────────

/// Heredoc + `$(...)` command substitution — full syntax through bashkit.
#[test]
fn bashkit_heredoc_and_substitution() {
    let result = run_bash(
        "cat <<'EOF'\nhello heredoc\nEOF\necho \"cargo: $(cargo --version)\"",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("hello heredoc"), "unexpected: {result}");
    assert!(result.contains("cargo: cargo "), "unexpected: {result}");
}

/// `cargo --version 2>&1 | tail -1` — pipeline + stderr redirect.
#[test]
fn bashkit_pipe_and_stderr_redirect() {
    // git's usage output goes to stderr; 2>&1 merges it into the pipe.
    let full = run_bash(
        "git status --definitely-not-a-flag 2>&1",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(full.contains("usage: git status"), "unexpected: {full}");

    // Keep exactly the last two lines, minus the "Exit code: N" trailer.
    let body = full
        .rsplit_once("\nExit code: ")
        .map(|(head, _)| head)
        .unwrap_or(&full);
    let mut expected: Vec<&str> = body.lines().rev().take(2).collect();
    expected.reverse();
    let tail = run_bash(
        "git status --definitely-not-a-flag 2>&1 | tail -2",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(
        tail.lines().collect::<Vec<_>>(),
        expected,
        "unexpected: {tail}"
    );
}

/// Pipeline stdin reaches the host command (`echo x | git hash-object --stdin`).
#[test]
fn bashkit_host_stdin_pipe() {
    let result = run_bash(
        "echo hello stdin | git hash-object --stdin",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    // 40-hex sha1 of "hello stdin\n" (computed via `git hash-object --stdin`)
    assert!(
        result.trim() == "be898d232ab8a1d6a4f526cce1728fa15f46d81d",
        "unexpected sha: {result}"
    );
}

// ── real filesystem writes ──────────────────────────────────

/// `echo > file` and `cp` must write through to the REAL host filesystem.
#[test]
fn bashkit_real_file_writes() {
    let tmp = TempDir::new("bash_writes").unwrap();
    let result = run_bash(
        "echo hello > write_test.txt && mkdir sub && cp write_test.txt sub/copy.txt",
        &tmp.path,
        None,
    )
    .unwrap();
    assert!(!result.contains("Exit code"), "unexpected: {result}");
    let written = fs::read_to_string(tmp.join("write_test.txt")).unwrap();
    assert_eq!(written, "hello\n");
    let copied = fs::read_to_string(tmp.join("sub/copy.txt")).unwrap();
    assert_eq!(copied, "hello\n");
}

// ── read-only root enforcement ──────────────────────────────

/// Writes outside the workspace, home, and `/tmp` mounts must FAIL with a
/// readonly error — never fake-succeed into a throwaway overlay.
#[test]
fn bashkit_readonly_zone_write_fails() {
    let tmp = TempDir::new("bash_ro_write").unwrap();
    let result = run_bash("echo hello > /crabot_ro_probe.txt", &tmp.path, None).unwrap();
    assert!(
        result.contains("readonly"),
        "expected readonly error, got: {result}"
    );
    assert!(result.contains("Exit code:"), "unexpected: {result}");
}

/// `rm` on an EXISTING host file in the readonly zone fails (readonly error)
/// and the host file survives — no whiteout swallowing.
#[test]
fn bashkit_readonly_zone_rm_fails() {
    // Host probe path: `/var/tmp` on Unix (outside the mounts), the
    // workspace's drive root on Windows. The VFS form must NOT carry the
    // drive letter — the `/` mount's backend is already the drive.
    let name = format!("crabot_ro_probe_{}", std::process::id());
    #[cfg(unix)]
    let (probe_host, probe_vfs) = (
        Path::new("/var/tmp").join(&name),
        format!("/var/tmp/{name}"),
    );
    #[cfg(windows)]
    let (probe_host, probe_vfs) = {
        let mut root = PathBuf::from(crabot_workspace().components().next().unwrap().as_os_str());
        root.push("\\");
        (root.join(&name), format!("/{name}"))
    };

    // Create the host file directly; skip if this dir isn't writable by us.
    if fs::write(&probe_host, b"probe").is_err() {
        return;
    }
    let result = run_bash(&format!("rm {probe_vfs}"), &crabot_workspace(), None).unwrap();
    assert!(
        result.contains("readonly"),
        "expected readonly error, got: {result}"
    );
    assert!(result.contains("Exit code:"), "unexpected: {result}");
    // The host file must survive the failed `rm`.
    assert_eq!(fs::read_to_string(&probe_host).unwrap(), "probe");
    let _ = fs::remove_file(&probe_host);
}

/// `/tmp` is a real read-write mount: writes persist to the host temp dir,
/// visible to later host commands.
#[test]
fn bashkit_tmp_mount_writes_to_real_temp() {
    let tmp = TempDir::new("bash_tmp").unwrap();
    let probe = format!("crabot_tmp_probe_{}", std::process::id());
    let result = run_bash(&format!("echo hello > /tmp/{probe}"), &tmp.path, None).unwrap();
    assert!(!result.contains("Exit code"), "unexpected: {result}");
    let host = std::env::temp_dir().join(&probe);
    assert_eq!(fs::read_to_string(&host).unwrap(), "hello\n");
    let _ = fs::remove_file(&host);
}

/// `cd /tmp` maps host commands to the real host temp dir, not the workspace:
/// `git` there is not inside a repository (the workspace is).
#[test]
fn bashkit_cd_tmp_does_not_fall_back_to_workspace() {
    let result = run_bash(
        "cd /tmp && git rev-parse --show-toplevel",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("fatal:"), "unexpected: {result}");
}

// ── cwd mapping ─────────────────────────────────────────────

/// `cd src && cargo check` — cwd persists via ctx.cwd, mapped to the host.
#[test]
fn bashkit_cd_then_cargo_check() {
    let result = run_bash(
        "cd src && cargo check --quiet",
        &crabot_workspace(),
        Some(120_000),
    )
    .unwrap();
    assert!(!result.contains("Exit code"), "unexpected: {result}");
}

/// `cd /` runs host commands at the read-only root mount, not silently back
/// in the workspace: `git` at the filesystem root is not inside a repository.
#[test]
fn bashkit_cd_root_does_not_fall_back_to_workspace() {
    let result = run_bash(
        "cd / && git rev-parse --show-toplevel",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("fatal:"), "unexpected: {result}");
}

// ── exit codes & timeouts ───────────────────────────────────

/// Non-zero exit codes are reported like the bash -c path.
#[test]
fn bashkit_exit_code_reporting() {
    let tmp = TempDir::new("bash_exit").unwrap();
    let result = run_bash("git status", &tmp.path, None).unwrap();
    assert!(result.contains("fatal:"), "unexpected: {result}");
    assert!(result.contains("Exit code: 128"), "unexpected: {result}");
}

/// A builtin returning `ExecResult::err` does NOT stop the script: like real
/// bash, the next `;`-separated command still runs and its exit code wins.
#[test]
fn bashkit_err_does_not_stop_script() {
    let result = run_bash(
        "definitely-not-a-command-xyz; echo after",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("after"), "unexpected: {result}");
    assert!(
        result.contains("definitely-not-a-command-xyz"),
        "spawn error not surfaced: {result}"
    );
    assert!(!result.contains("Exit code"), "unexpected: {result}");
}

/// A failing builtin as the last command: its 127 (bash's command-not-found
/// convention) becomes the script's final exit code.
#[test]
fn bashkit_spawn_failure_exit_code() {
    let result = run_bash("definitely-not-a-command-xyz", &crabot_workspace(), None).unwrap();
    assert!(result.contains("Exit code: 127"), "unexpected: {result}");
}

/// Signal death is reported like real bash (`128 + signal`, SIGTERM → 143) —
/// `exit_code_of` normalizes both the Unix and MSYS/Cygwin encodings.
/// `perl -e 'kill 15,$$'` self-terminates with SIGTERM (perl ships with Git
/// for Windows and every Unix).
#[test]
fn bashkit_signal_death_exit_code() {
    let result = run_bash("perl -e 'kill 15,$$'", &crabot_workspace(), None).unwrap();
    assert!(result.contains("Exit code: 143"), "unexpected: {result}");
}

/// The real-bash fallback reports the same 128+SIG convention (`eval` forces
/// the fallback route). The command after `kill` never runs.
#[test]
fn real_bash_signal_death_exit_code() {
    let result = run_bash(
        "eval \"kill -TERM $$\"; echo unreachable",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("Exit code: 143"), "unexpected: {result}");
    assert!(!result.contains("unreachable"), "unexpected: {result}");
}

/// Cancellation aborts the WHOLE script: the outer `select!` in
/// `bash_kit::execute` drops the interpreter, so no command after the
/// cancelled one runs — same as real bash, where the process group is killed.
#[test]
fn bashkit_cancel_aborts_whole_script() {
    let err = run_bash_with_cancel(
        "sleep 100; echo after",
        &crabot_workspace(),
        None,
        AtomicBool::new(true),
    )
    .unwrap_err();
    assert!(err.contains("Cancelled by user"), "unexpected: {err}");
    assert!(
        !err.contains("after"),
        "script continued after cancel: {err}"
    );
}

/// `sleep 100` with a 3s timeout — the outer tokio select fires.
#[test]
fn bashkit_sleep_timeout() {
    let err = run_bash("sleep 100", &crabot_workspace(), Some(3000)).unwrap_err();
    assert!(err.contains("timed out"), "unexpected: {err}");
}

// ── fallback to real bash ───────────────────────────────────

/// `eval "cargo --version"` — opaque payload falls back to real `bash -c`.
#[test]
fn bashkit_eval_falls_back_to_real_bash() {
    let result = run_bash("eval \"cargo --version\"", &crabot_workspace(), None).unwrap();
    assert!(result.contains("cargo "), "unexpected: {result}");
}

/// Dynamic command name (`$TOOL`) falls back to real `bash -c`.
#[test]
fn bashkit_dynamic_name_falls_back_to_real_bash() {
    let result = run_bash("TOOL=cargo; $TOOL --version", &crabot_workspace(), None).unwrap();
    assert!(result.contains("cargo "), "unexpected: {result}");
}

/// Syntax bashkit cannot parse falls back to real `bash -c` unchanged.
#[test]
fn bashkit_parse_error_falls_back() {
    let result = run_bash("echo 'unterminated", &crabot_workspace(), None).unwrap();
    assert!(
        result.contains("unexpected EOF") || result.contains("EOF"),
        "unexpected: {result}"
    );
}
