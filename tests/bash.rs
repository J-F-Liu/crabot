//! Tests for the `bash` tool: the bashkit in-process interpreter route and
//! its `bash -c` fallback.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TempDir;
use tokio_util::sync::CancellationToken;

use crabot::tools::{ToolRegistry, tmp_host_dir};

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

/// Drive root of `path` and its lowercase VFS letter (`D:\` → (`D:\`, `d`)).
#[cfg(windows)]
fn drive_root_of(path: &Path) -> (PathBuf, char) {
    let std::path::Component::Prefix(prefix) = path.components().next().unwrap() else {
        panic!("no drive prefix: {}", path.display());
    };
    let (std::path::Prefix::Disk(d) | std::path::Prefix::VerbatimDisk(d)) = prefix.kind() else {
        panic!("no disk prefix: {}", path.display());
    };
    let mut root = PathBuf::from(prefix.as_os_str());
    root.push("\\");
    (root, (d as char).to_ascii_lowercase())
}

/// Probe file in the drive root of `path`: host path + VFS path (`/d/name`).
#[cfg(windows)]
fn drive_probe(path: &Path, name: &str) -> (PathBuf, String) {
    let (root, letter) = drive_root_of(path);
    (root.join(name), format!("/{letter}/{name}"))
}

/// The crabot repository root (used as the test workspace where git exists).
fn crabot_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Registered bash tool plus the JSON args for `command`.
fn bash_tool(
    command: &str,
    timeout_ms: Option<u64>,
) -> (crabot::tools::ToolRef, serde_json::Value) {
    fix_test_path();
    let registry = ToolRegistry::new();
    let bash = registry.find_tool("bash").expect("bash tool registered");
    let mut args = serde_json::json!({ "command": command });
    if let Some(ms) = timeout_ms {
        args["timeout"] = serde_json::json!(ms);
    }
    (bash, args)
}

/// Multi-thread test runtime (tool execution needs `spawn_blocking`).
fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

/// Join a spawned tool task like `llm::tool_call::await_tool` does.
async fn await_tool(
    handle: tokio::task::JoinHandle<Result<String, String>>,
) -> Result<String, String> {
    handle
        .await
        .unwrap_or_else(|e| Err(format!("Tool execution panicked: {e}")))
}

/// Run the `bash` tool exactly like `llm::tool_call::call_tool` does: on a blocking
/// thread with a tokio runtime context.
fn run_bash(command: &str, workspace: &Path, timeout_ms: Option<u64>) -> Result<String, String> {
    run_bash_with_cancel(command, workspace, timeout_ms, CancellationToken::new())
}

/// Like [`run_bash`], with a caller-controlled cancel token.
fn run_bash_with_cancel(
    command: &str,
    workspace: &Path,
    timeout_ms: Option<u64>,
    cancel: CancellationToken,
) -> Result<String, String> {
    let (bash, args) = bash_tool(command, timeout_ms);
    let workspace = workspace.to_path_buf();
    test_runtime().block_on(async {
        await_tool(tokio::task::spawn_blocking(move || {
            bash.execute(&args, &workspace, &cancel)
        }))
        .await
    })
}

/// Like [`run_bash`] but via `execute_streaming`, forwarding capped output
/// chunks to `tx` as they are produced.
fn run_bash_streaming(
    command: &str,
    workspace: &Path,
    timeout_ms: Option<u64>,
    tx: std::sync::mpsc::Sender<String>,
) -> Result<String, String> {
    run_bash_streaming_with_cancel(command, workspace, timeout_ms, CancellationToken::new(), tx)
}

/// Like [`run_bash_streaming`] with a caller-controlled cancel token.
fn run_bash_streaming_with_cancel(
    command: &str,
    workspace: &Path,
    timeout_ms: Option<u64>,
    cancel: CancellationToken,
    tx: std::sync::mpsc::Sender<String>,
) -> Result<String, String> {
    let (bash, args) = bash_tool(command, timeout_ms);
    let workspace = workspace.to_path_buf();
    // Same live-output cap as the UI path (`llm::tool_call::call_tool_streaming`).
    let sink = crabot::tools::capping_sink(move |out| {
        let _ = tx.send(out);
    });
    test_runtime().block_on(async {
        await_tool(tokio::task::spawn_blocking(move || {
            bash.execute_streaming(&args, &workspace, &cancel, &sink)
        }))
        .await
    })
}

/// Spawn `run_bash_streaming` on a thread; returns its handle and the chunk receiver.
fn spawn_stream(
    command: &str,
    workspace: &Path,
    timeout_ms: Option<u64>,
) -> (
    std::thread::JoinHandle<Result<String, String>>,
    std::sync::mpsc::Receiver<String>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let command = command.to_string();
    let workspace = workspace.to_path_buf();
    let handle =
        std::thread::spawn(move || run_bash_streaming(&command, &workspace, timeout_ms, tx));
    (handle, rx)
}

/// Run a streaming command to completion, collecting all chunks.
fn stream_and_collect(
    command: &str,
    workspace: &Path,
    timeout_ms: Option<u64>,
) -> (Result<String, String>, Vec<String>) {
    let (handle, rx) = spawn_stream(command, workspace, timeout_ms);
    let mut chunks = Vec::new();
    while let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
        chunks.push(chunk);
    }
    let result = handle.join().expect("streaming thread panicked");
    (result, chunks)
}

/// True when a host executable of `name` is resolvable (git-bash/Unix).
fn host_command_exists(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Assert `result` has at least `n` lines of 40-char hex SHA-1 (git hash-object).
fn assert_sha_lines(result: &str, n: usize) {
    let shas: Vec<&str> = result.lines().collect();
    assert!(shas.len() >= n, "unexpected: {result}");
    assert!(
        shas.iter()
            .all(|s| s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())),
        "unexpected: {result}"
    );
}

/// Write `name` into `dir` as a `.cmd` launcher (Windows) or an executable
/// shebang script (Unix) running `body`; returns its file name.
fn write_command(dir: &Path, name: &str, body: &str) -> String {
    #[cfg(windows)]
    let (file, script) = (format!("{name}.cmd"), format!("@echo off\n{body}\r\n"));
    #[cfg(unix)]
    let (file, script) = (name.to_string(), format!("#!/bin/sh\n{body}\n"));
    fs::write(dir.join(&file), script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.join(&file), fs::Permissions::from_mode(0o755)).unwrap();
    }
    file
}

/// Write a `name` shim into `dir` that echoes `message`; returns its file name.
fn write_shim(dir: &Path, name: &str, message: &str) -> String {
    write_command(dir, name, &format!("echo {message}"))
}

// ── bashkit syntax features ─────────────────────────────────

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

// ── embedded python (host python, or bashkit's Monty) ───────

/// `python` resolves either way: to the host interpreter when one exists,
/// otherwise to bashkit's embedded Monty.
#[test]
fn bashkit_python_builtin() {
    let result = run_bash("python -c 'print(1 + 2)'", &crabot_workspace(), None).unwrap();
    assert!(result.contains("3"), "unexpected: {result}");
}

/// `python3 --version` reports the interpreter in use: host CPython when one
/// exists, Monty otherwise (a lone `python` without `python3` fails like
/// real bash).
#[test]
fn bashkit_python3_version() {
    let result = run_bash("python3 --version", &crabot_workspace(), None).unwrap();
    match (
        host_command_exists("python"),
        host_command_exists("python3"),
    ) {
        (_, true) => assert!(result.contains("Python "), "unexpected: {result}"),
        (false, false) => assert!(result.contains("monty"), "unexpected: {result}"),
        (true, false) => assert!(!result.contains("monty"), "unexpected: {result}"),
    }
}

/// Python file I/O reaches the workspace in both modes (host cwd vs Monty's
/// VFS-bridged `open()`); `rb` keeps the assertion locale-independent.
#[test]
fn bashkit_python_vfs_open() {
    let result = run_bash(
        "python -c \"print(open('README.md', 'rb').readline().strip())\"",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("Crabot"), "unexpected: {result}");
}

/// Python output must arrive in live chunks in both modes: pipe drains for
/// the host interpreter, the interpreter callback for Monty.
#[test]
fn bashkit_python_streaming() {
    let (result, chunks) = stream_and_collect(
        "python -c 'for i in range(3): print(i)'",
        &crabot_workspace(),
        None,
    );
    result.unwrap();
    let joined = chunks.concat();
    assert!(
        joined.contains("0") && joined.contains("1") && joined.contains("2"),
        "unexpected: {joined}"
    );
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
    #[cfg(unix)]
    let probe_vfs = "/crabot_ro_probe.txt";
    #[cfg(windows)]
    let probe_vfs = drive_probe(&tmp.path, "crabot_ro_probe.txt").1;
    let result = run_bash(&format!("echo hello > {probe_vfs}"), &tmp.path, None).unwrap();
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
    // Probe outside the writable mounts: `/var/tmp` on Unix, the
    // workspace's drive root on Windows (via its drive-letter mount).
    let name = format!("crabot_ro_probe_{}", std::process::id());
    #[cfg(unix)]
    let (probe_host, probe_vfs) = (
        Path::new("/var/tmp").join(&name),
        format!("/var/tmp/{name}"),
    );
    #[cfg(windows)]
    let (probe_host, probe_vfs) = drive_probe(&crabot_workspace(), &name);

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

/// Windows: the drive root is also readable at its drive-letter VFS path
/// (`/d/...` for `D:\`), consistent with the workspace's own VFS form.
/// Read-only enforcement there is covered by the readonly tests above.
#[cfg(windows)]
#[test]
fn bashkit_windows_drive_root_mounted_at_drive_letter() {
    let name = format!("crabot_drive_probe_{}", std::process::id());
    let (probe_host, probe_vfs) = drive_probe(&crabot_workspace(), &name);
    // Create the host file directly; skip if the drive root isn't writable.
    if fs::write(&probe_host, b"probe").is_err() {
        return;
    }
    let result = run_bash(&format!("cat {probe_vfs}"), &crabot_workspace(), None).unwrap();
    assert!(result.contains("probe"), "unexpected: {result}");
    let _ = fs::remove_file(&probe_host);
}

/// Windows: every present drive (not just the workspace's) is readable at
/// its drive-letter VFS path (`/c`, `/d`, …). Drives bashkit cannot open
/// (empty card readers, stale network shares) are skipped the same way by
/// both the test and the mount table, so only canonicalizable drives assert.
#[cfg(windows)]
#[test]
fn bashkit_windows_all_drives_mounted_at_drive_letters() {
    let mut asserted = 0;
    for letter in 'A'..='Z' {
        if std::fs::canonicalize(format!("{letter}:\\")).is_err() {
            continue; // no media — bashkit skips this drive too
        }
        asserted += 1;
        let vfs = format!("/{}", letter.to_ascii_lowercase());
        let result = run_bash(&format!("ls {vfs}"), &crabot_workspace(), None).unwrap();
        assert!(!result.contains("Exit code:"), "ls {vfs} failed: {result}");
    }
    assert!(asserted > 0, "no readable drives on this host");
}

/// `/tmp` is a real read-write mount: writes persist to the shared tmp dir
/// ([`tmp_host_dir`]), visible to later host commands.
#[test]
fn bashkit_tmp_mount_writes_to_real_dir() {
    let tmp = TempDir::new("bash_tmp").unwrap();
    let probe = format!("crabot_tmp_probe_{}", std::process::id());
    let result = run_bash(&format!("echo hello > /tmp/{probe}"), &tmp.path, None).unwrap();
    assert!(!result.contains("Exit code"), "unexpected: {result}");
    let host = tmp_host_dir().join(&probe);
    assert_eq!(fs::read_to_string(&host).unwrap(), "hello\n");
    let _ = fs::remove_file(&host);
}

/// Regression: the `read` tool resolves `/tmp` to the same host dir
/// ([`tmp_host_dir`]) as the `bash` tool's mount, so bash-written `/tmp/...`
/// files are readable by the file tools. Windows-only: on Unix the file
/// tools treat `/tmp` as a real path, which differs from the mount on
/// macOS where `temp_dir()` is `$TMPDIR`.
#[cfg(windows)]
#[test]
fn bashkit_tmp_written_file_readable_by_read_tool() {
    let tmp = TempDir::new("bash_tmp_read").unwrap();
    let probe = format!("crabot_tmp_read_{}", std::process::id());
    let result = run_bash(&format!("echo hello > /tmp/{probe}"), &tmp.path, None).unwrap();
    assert!(!result.contains("Exit code"), "unexpected: {result}");

    let registry = ToolRegistry::new();
    let read = registry.find_tool("read").expect("read tool registered");
    let args = serde_json::json!({ "path": format!("/tmp/{probe}") });
    let out = read
        .execute(&args, &tmp.path, &CancellationToken::new())
        .unwrap();
    assert!(out.contains("hello"), "read tool could not see /tmp: {out}");

    let _ = fs::remove_file(tmp_host_dir().join(&probe));
}

/// `cd /tmp` maps host commands to the [`tmp_host_dir`] mount — outside the
/// workspace repo, so `git` reports `fatal:`.
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

// ── host command environment ────────────────────────────────

/// Run a script against bridged host `git` with `GIT_CONFIG_GLOBAL` pointing
/// at a probe config (path single-quoted for bash); returns the output.
fn run_git_config_probe(script: impl Fn(&str) -> String) -> Result<String, String> {
    let tmp = TempDir::new("bash_env_probe").unwrap();
    let config = tmp.join("probe.gitconfig");
    fs::write(&config, "[probe]\n\tkey = probe-value\n").unwrap();
    let path = format!("'{}'", config.display());
    run_bash(&script(&path), &tmp.path, None)
}

/// `export VAR=...` must reach bridged host commands: `git` reads
/// `GIT_CONFIG_GLOBAL` from its environment.
#[test]
fn bashkit_export_reaches_host_command() {
    let result = run_git_config_probe(|path| {
        format!("export GIT_CONFIG_GLOBAL={path}\ngit config --get probe.key")
    })
    .unwrap();
    assert!(result.contains("probe-value"), "unexpected: {result}");
}

/// Prefix assignments (`VAR=value cmd`) must reach bridged host commands.
#[test]
fn bashkit_prefix_assignment_reaches_host_command() {
    let result =
        run_git_config_probe(|path| format!("GIT_CONFIG_GLOBAL={path} git config --get probe.key"))
            .unwrap();
    assert!(result.contains("probe-value"), "unexpected: {result}");
}

/// `unset VAR` must hide the var from bridged host commands, not just the
/// interpreter: `git` must not see a config exported and unset in the script.
#[test]
fn bashkit_unset_hides_env_from_host_command() {
    let result = run_git_config_probe(|path| {
        format!(
            "export GIT_CONFIG_GLOBAL={path}\nunset GIT_CONFIG_GLOBAL\ngit config --get probe.key"
        )
    })
    .unwrap();
    assert!(
        !result.contains("probe-value"),
        "unset var leaked to host command: {result}"
    );
    assert!(result.contains("Exit code: 1"), "unexpected: {result}");
}

// ── command lookup (`which`, `type`) ────────────────────────

/// `which <host tool>` reports the executable the bridge spawns — a VFS path the
/// script can keep using (`[ -f "$p" ]`), not the bare name bashkit printed.
#[test]
fn bashkit_which_resolves_host_command() {
    let result = run_bash(
        r#"p=$(which cargo); case "$p" in /*) echo "vfs $p";; *) echo "not a path: $p";; esac; [ -f "$p" ] && echo file-ok"#,
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("vfs /"), "unexpected: {result}");
    assert!(result.contains("cargo"), "unexpected: {result}");
    assert!(result.contains("file-ok"), "unexpected: {result}");
    assert!(!result.contains("Exit code"), "unexpected: {result}");
}

/// A script the sandbox itself puts on `$PATH` resolves to its VFS path — the
/// OS spelling too (`cargo` → `cargo.exe`, `npx` → `npx.cmd`) — and runs.
#[test]
fn bashkit_which_searches_vfs_path() {
    let tmp = TempDir::new("which_vfs_path").unwrap();
    let file = write_shim(&tmp.path, "mytool", "tool-ran");
    let dir = crabot::tools::convert_path_to_unix_style(&tmp.path);
    let result = run_bash(
        &format!("export PATH='{dir}':$PATH; which mytool; mytool"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(
        result.contains(&format!("{dir}/{file}")),
        "unexpected: {result}"
    );
    assert!(result.contains("tool-ran"), "unexpected: {result}");
}

/// A name only the interpreter dispatches (`json`) is still reported by name, so
/// `which <builtin>` works as an availability check.
#[test]
fn bashkit_which_reports_interpreter_names() {
    let result = run_bash("which json", &crabot_workspace(), None).unwrap();
    assert_eq!(result.trim(), "json", "unexpected: {result}");
}

/// `-a` lists every match, in `$PATH` order.
#[test]
fn bashkit_which_all_lists_every_match() {
    let first = TempDir::new("which_all_first").unwrap();
    let second = TempDir::new("which_all_second").unwrap();
    let file = write_shim(&first.path, "mytool", "first");
    write_shim(&second.path, "mytool", "second");

    let (a, b) = (
        crabot::tools::convert_path_to_unix_style(&first.path),
        crabot::tools::convert_path_to_unix_style(&second.path),
    );
    let result = run_bash(
        &format!("export PATH='{a}':'{b}':$PATH; which -a mytool"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(
        result.lines().take(2).collect::<Vec<_>>(),
        [format!("{a}/{file}"), format!("{b}/{file}")],
        "unexpected: {result}"
    );
}

/// One file is listed once, however many roots the mount table reaches it from:
/// the host search's answer to `/tmp/x` is the file the VFS already reported.
#[test]
fn bashkit_which_all_reports_a_file_once() {
    let tmp = TempDir::new("which_all_alias").unwrap();
    let file = write_shim(&tmp.path, "mytool", "tool-ran");
    let name = tmp.path.file_name().unwrap().to_string_lossy().into_owned();
    let result = run_bash(
        // Through the `/tmp` mount, which is the same dir the host search lists.
        &format!("export PATH='/tmp/{name}'; which -a mytool"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [format!("/tmp/{name}/{file}")],
        "unexpected: {result}"
    );
}

/// `type` describes what runs: a file by path, a builtin and a function by
/// kind — `-t` prints the kind word, `-P`/`-p` the file alone.
#[test]
fn bashkit_type_resolves_files_builtins_and_functions() {
    let result = run_bash(
        "type cargo; type -t cargo; type -P cargo; type ls; type -t ls; type -t if; \
         f() { :; }; type f; type -t f",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("cargo is /"), "unexpected: {result}");
    assert!(
        result.contains("ls is a shell builtin"),
        "unexpected: {result}"
    );
    assert!(result.contains("f is a function"), "unexpected: {result}");
    // `-t` prints the kind word alone — a line of its own, not part of a description.
    let lines: Vec<&str> = result.lines().collect();
    for kind in ["file", "builtin", "keyword", "function"] {
        assert!(lines.contains(&kind), "missing {kind} line: {result}");
    }
}

/// `type -P <missing>` stays silent and exits 1, like bash; the plain form
/// reports the miss.
#[test]
fn bashkit_type_reports_missing_names() {
    let result = run_bash(
        "type -P nosuchtool; echo p=$?; type nosuchtool; echo t=$?",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(
        result.lines().next(),
        Some("p=1"),
        "-P must stay silent: {result}"
    );
    assert!(
        result.contains("bash: type: nosuchtool: not found"),
        "unexpected: {result}"
    );
    assert!(result.contains("t=1"), "unexpected: {result}");
}

/// Unix keeps honoring the executable bit: a file on `$PATH` that is not
/// executable is not a command.
#[cfg(unix)]
#[test]
fn bashkit_which_requires_the_executable_bit() {
    let tmp = TempDir::new("which_exec_bit").unwrap();
    fs::write(tmp.join("mytool"), "echo nope\n").unwrap();
    let dir = crabot::tools::convert_path_to_unix_style(&tmp.path);
    let result = run_bash(
        &format!("export PATH='{dir}':$PATH; which mytool; echo rc=$?"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(result.trim(), "rc=1", "unexpected: {result}");
}

/// Windows cannot trust the mode bit: an extensionless file that is not a real
/// executable image is not a command — the host resolver decides, not the VFS.
#[cfg(windows)]
#[test]
fn bashkit_which_skips_extensionless_non_executable() {
    let tmp = TempDir::new("which_plain").unwrap();
    fs::write(tmp.join("plain"), "not a program\n").unwrap();
    let dir = crabot::tools::convert_path_to_unix_style(&tmp.path);
    let result = run_bash(
        &format!("export PATH='{dir}':$PATH; which plain; echo rc=$?"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(result.trim(), "rc=1", "unexpected: {result}");
}

/// An extensionless file next to its `PATHEXT` spelling resolves to the one that
/// runs: the host resolver's own extension probe must not vouch for the bare name.
#[cfg(windows)]
#[test]
fn bashkit_which_prefers_the_runnable_spelling() {
    let tmp = TempDir::new("which_spelling").unwrap();
    fs::write(tmp.join("plain"), "not a program\n").unwrap();
    let file = write_shim(&tmp.path, "plain", "spelling-ran");
    let dir = crabot::tools::convert_path_to_unix_style(&tmp.path);
    let result = run_bash(
        &format!("export PATH='{dir}'; which plain; plain"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(
        result.lines().next(),
        Some(format!("{dir}/{file}").as_str()),
        "unexpected: {result}"
    );
    assert!(result.contains("spelling-ran"), "unexpected: {result}");
}

/// A native `C:\…` operand resolves like its VFS spelling: `type`'s args are not
/// rewritten to VFS form, so it used to answer "not found".
#[cfg(windows)]
#[test]
fn bashkit_type_resolves_native_path_operand() {
    let tmp = TempDir::new("type_native_path").unwrap();
    let file = write_shim(&tmp.path, "mytool", "shim-ran");
    let native = tmp.join(&file);
    let result = run_bash(
        &format!("type '{}'; which '{}'", native.display(), native.display()),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    // The temp dir is mounted at `/tmp` — the deepest match for its host path.
    let tmp_name = tmp.path.file_name().unwrap().to_string_lossy();
    let vfs = format!("/tmp/{tmp_name}/{file}");
    assert!(
        result.contains(&format!("{} is {vfs}", native.display())),
        "unexpected: {result}"
    );
    assert!(
        result.lines().any(|line| line == vfs),
        "unexpected: {result}"
    );
}

/// A plain `PATH=…` (no `export`) reaches the bridged command's own environment:
/// real bash keeps `PATH` exported, so the child resolves its subprocesses
/// through the same list the script's shell searched.
#[test]
fn bashkit_unexported_path_reaches_host_command() {
    let tmp = TempDir::new("path_child_env").unwrap();
    // A shim that echoes the `PATH` the child was handed.
    #[cfg(windows)]
    let body = "echo %PATH%";
    #[cfg(unix)]
    let body = "printf '%s\\n' \"$PATH\"";
    write_command(&tmp.path, "pathprobe", body);
    let dir = crabot::tools::convert_path_to_unix_style(&tmp.path);
    let result = run_bash(
        &format!("PATH='{dir}':$PATH; pathprobe"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    let name = tmp.path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        result.contains(&name),
        "child PATH lacks the script dir: {result}"
    );
    assert!(!result.contains("not found"), "unexpected: {result}");
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

/// `cd /` never falls back to the workspace: Unix `/` is the readonly host
/// root (git there is not in a repository), Windows `/` has no backend and
/// the host command errors instead.
#[test]
fn bashkit_cd_root_does_not_fall_back_to_workspace() {
    let result = run_bash(
        "cd / && git rev-parse --show-toplevel",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    #[cfg(unix)]
    assert!(result.contains("fatal:"), "unexpected: {result}");
    #[cfg(windows)]
    assert!(
        result.contains("outside mapped cwd"),
        "unexpected: {result}"
    );
}

/// GBK output (a Chinese-locale console program) is decoded, not mangled.
/// `eval` forces the real-`bash -c` route; `printf` octal escapes emit the
/// raw GBK bytes for 中文 (\326\320\316\304).
#[test]
fn gbk_output_is_decoded_not_garbled() {
    let result = run_bash(
        "eval \"printf '\\326\\320\\316\\304'\"",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("中文"), "unexpected: {result}");
    assert!(!result.contains('\u{FFFD}'), "unexpected: {result}");
}

/// The streaming route decodes GBK too: live chunks pin the encoding instead
/// of emitting raw lossy bytes.
#[test]
fn gbk_streaming_is_decoded() {
    let (result, chunks) = stream_and_collect(
        "eval \"printf '\\326\\320\\316\\304'\"",
        &crabot_workspace(),
        None,
    );
    result.unwrap();
    let streamed: String = chunks.concat();
    assert!(streamed.contains("中文"), "unexpected stream: {streamed:?}");
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

/// `git -C <vfs-workspace>` — VFS absolute args are rewritten to host paths
/// (MSYS2-style), so native git accepts them.
#[test]
fn bashkit_converts_vfs_path_args_for_host_git() {
    if !host_command_exists("git") {
        return;
    }
    let ws = crabot_workspace();
    let vfs = crabot::tools::convert_path_to_unix_style(&ws);
    let result = run_bash(
        &format!("git -C '{vfs}' rev-parse --show-toplevel"),
        &ws,
        None,
    )
    .unwrap();
    assert!(!result.contains("fatal:"), "unexpected: {result}");
    assert!(!result.contains("cannot change"), "unexpected: {result}");
    let name = ws.file_name().unwrap().to_string_lossy().into_owned();
    assert!(result.contains(&name), "unexpected: {result}");
}

/// `git --git-dir=<vfs>/.git` — the `--opt=<vfs-path>` form converts too.
#[test]
fn bashkit_converts_attached_vfs_path_args() {
    if !host_command_exists("git") {
        return;
    }
    let ws = crabot_workspace();
    let vfs = crabot::tools::convert_path_to_unix_style(&ws);
    let result = run_bash(
        &format!("git --git-dir='{vfs}/.git' rev-parse --is-bare-repository"),
        &ws,
        None,
    )
    .unwrap();
    assert!(result.contains("false"), "unexpected: {result}");
}

/// `git -C /tmp` — the [`tmp_host_dir`] mount, so git reports `not a git
/// repository` instead of `cannot change to '/tmp'`.
#[test]
fn bashkit_converts_tmp_vfs_path_args() {
    if !host_command_exists("git") {
        return;
    }
    let ws = crabot_workspace();
    let result = run_bash("git -C /tmp rev-parse --show-toplevel", &ws, None).unwrap();
    assert!(!result.contains("cannot change"), "unexpected: {result}");
    assert!(
        result.contains("not a git repository"),
        "unexpected: {result}"
    );
}

/// Windows: host-style drive paths in builtin arguments are rewritten to VFS
/// form (`E:/...` → `/e/...`) — the inverse of the bridged-command
/// conversion. `grep` (a bashkit builtin) reads both a drive-root probe file
/// via its drive-letter mount and a temp-dir file via the home mount.
#[cfg(windows)]
#[test]
fn bashkit_builtins_convert_host_path_args() {
    let ws = crabot_workspace();
    let needle = "crabot-host-path-probe-marker";

    // Probe file in the drive root (read-only drive-letter mount).
    let name = format!("crabot_hostpath_probe_{}.txt", std::process::id());
    let (probe_host, probe_vfs) = drive_probe(&ws, &name);
    if fs::write(&probe_host, format!("{needle}\n")).is_ok() {
        let host_form = probe_host.to_string_lossy().replace('\\', "/");
        let result = run_bash(&format!("grep '{needle}' '{host_form}'"), &ws, None).unwrap();
        assert!(result.contains(needle), "host form failed: {result}");
        assert!(!result.contains("No such file"), "unexpected: {result}");
        // The VFS form works the same (control).
        let result = run_bash(&format!("grep '{needle}' '{probe_vfs}'"), &ws, None).unwrap();
        assert!(result.contains(needle), "VFS form failed: {result}");
        let _ = fs::remove_file(&probe_host);
    }

    // File under the temp dir (read-write home mount path).
    let dir = TempDir::new("hostpath").unwrap();
    let file = dir.join("probe.txt");
    fs::write(&file, format!("{needle}\n")).unwrap();
    let host_form = file.to_string_lossy().replace('\\', "/");
    let result = run_bash(&format!("grep '{needle}' '{host_form}'"), &ws, None).unwrap();
    assert!(
        result.contains(needle),
        "host form under temp failed: {result}"
    );
}

/// Windows: when one builtin operand is a string prefix of another
/// (`.../probe` vs `.../probe.txt`), both still convert — the rewrite
/// replaces longer paths first, so the shorter one's occurrence inside the
/// longer path no longer inflates its census count.
#[cfg(windows)]
#[test]
fn bashkit_builtins_convert_prefix_overlapping_args() {
    let ws = crabot_workspace();
    let dir = TempDir::new("hostpath_prefix").unwrap();
    let short = dir.join("probe");
    let long = dir.join("probe.txt");
    fs::write(&short, "short\n").unwrap();
    fs::write(&long, "long\n").unwrap();
    let short_host = short.to_string_lossy().replace('\\', "/");
    let long_host = long.to_string_lossy().replace('\\', "/");
    // Longer path first in the command: without longest-first replacement the
    // shorter arg keeps its host form and `cat` fails to find it.
    let result = run_bash(&format!("cat '{long_host}' '{short_host}'"), &ws, None).unwrap();
    assert!(
        result.contains("short") && result.contains("long"),
        "unexpected: {result}"
    );
    assert!(!result.contains("No such file"), "unexpected: {result}");
}

/// Windows: output positions and bridged commands keep host-style paths
/// verbatim — the rewrite only touches builtin file operands.
#[cfg(windows)]
#[test]
fn bashkit_builtins_leave_output_args_verbatim() {
    let ws = crabot_workspace();
    let result = run_bash("echo 'E:/virtual/unchanged'", &ws, None).unwrap();
    assert!(
        result.contains("E:/virtual/unchanged"),
        "unexpected: {result}"
    );
    assert!(
        !result.contains("/e/virtual/unchanged"),
        "unexpected: {result}"
    );
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
/// convention) becomes the script's final exit code, with bash's wording.
#[test]
fn bashkit_spawn_failure_exit_code() {
    let result = run_bash("definitely-not-a-command-xyz", &crabot_workspace(), None).unwrap();
    assert!(result.contains("Exit code: 127"), "unexpected: {result}");
    assert!(
        result.contains("definitely-not-a-command-xyz: command not found"),
        "unexpected: {result}"
    );
}

/// Signal death is reported like real bash (`128 + signal`, SIGTERM → 143):
/// the inner `sh` kills its own PID via its builtin `kill`. Interpreter
/// reentry, so this runs through the real `bash -c` fallback.
#[test]
fn nested_sh_signal_death_exit_code() {
    let result = run_bash("sh -c 'kill -TERM $$'", &crabot_workspace(), None).unwrap();
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
    // Cancel mid-run: this must hit the outer `select!` (not the pre-execute
    // wrapper check), which drops the script future before `echo after` runs.
    let cancel = CancellationToken::new();
    let flipper = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        flipper.cancel();
    });
    let err = run_bash_with_cancel("sleep 100; echo after", &crabot_workspace(), None, cancel)
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

/// Partial output produced before the timeout survives into the error — the
/// builtin-only callback route feeds the script-level capture (sink or not).
#[test]
fn bashkit_timeout_includes_partial_builtin_output() {
    let err = run_bash("echo before; sleep 100", &crabot_workspace(), Some(3000)).unwrap_err();
    assert!(err.contains("timed out"), "unexpected: {err}");
    assert!(err.contains("--- partial stdout ---"), "unexpected: {err}");
    assert!(err.contains("before"), "unexpected: {err}");
}

/// Host-command output drained before the timeout also survives, like the
/// real-bash route.
#[test]
fn bashkit_timeout_includes_partial_host_output() {
    let err = run_bash(
        "git rev-parse HEAD; sleep 100",
        &crabot_workspace(),
        Some(3000),
    )
    .unwrap_err();
    assert!(err.contains("timed out"), "unexpected: {err}");
    assert!(err.contains("--- partial stdout ---"), "unexpected: {err}");
    assert!(
        err.lines()
            .any(|l| l.len() == 40 && l.chars().all(|c| c.is_ascii_hexdigit())),
        "host output missing from error: {err}"
    );
}

/// Cancellation mid-script keeps output produced before the cancel: the
/// cancel fires only after the first chunk proves the script started, so
/// build-time jitter can't cancel before anything was captured.
#[test]
fn bashkit_cancel_includes_partial_output() {
    let workspace = crabot_workspace();
    let cancel = CancellationToken::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn({
        let cancel = cancel.clone();
        move || {
            run_bash_streaming_with_cancel("echo before; sleep 100", &workspace, None, cancel, tx)
        }
    });
    // First chunk ⇒ the script is running; cancel mid-`sleep 100`.
    let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
    cancel.cancel();
    let err = handle
        .join()
        .expect("streaming thread panicked")
        .unwrap_err();
    assert!(err.contains("Cancelled by user"), "unexpected: {err}");
    assert!(err.contains("--- partial stdout ---"), "unexpected: {err}");
    assert!(err.contains("before"), "unexpected: {err}");
}

// ── streaming output ────────────────────────────────────────

/// External commands (`git` → HostCommandBuiltin) stream their output live
/// from the pipe drain, and the final result must still contain everything.
#[test]
fn bashkit_streaming_external_output() {
    // Three slow git invocations; each hash line streams while the script is
    // still running (the sleeps stretch the timeline).
    let script = "git rev-parse --short HEAD; sleep 0.5; git rev-parse --short HEAD; sleep 0.5; git rev-parse --short HEAD";
    let (result, chunks) = stream_and_collect(script, &crabot_workspace(), Some(20_000));
    let result = result.unwrap();
    assert!(chunks.len() >= 2, "expected live chunks, got: {chunks:?}");
    // Live stream equals the final result (modulo CRLF normalization); this
    // relies on `git rev-parse` writing only to stdout.
    let joined: String = chunks.concat();
    assert_eq!(
        result.replace("\r\n", "\n"),
        joined,
        "streamed output diverges from final result"
    );
}

/// `eval` forces the real `bash -c` fallback; its pipe drain must stream too.
#[test]
fn real_bash_streaming_output() {
    let tmp = TempDir::new("bash_stream_real").unwrap();
    let script = "eval 'for i in 1 2 3; do echo $i; sleep 0.3; done'";
    let (result, chunks) = stream_and_collect(script, &tmp.path, Some(10_000));
    let result = result.unwrap();
    assert!(chunks.len() >= 2, "expected live chunks, got: {chunks:?}");
    let joined: String = chunks.concat();
    assert!(
        joined.contains("1\n2\n3"),
        "lines not streamed in order: {joined:?}"
    );
    assert_eq!(result.lines().last(), Some("3"), "unexpected: {result}");
}

/// Output emitted before a quiet stretch must stream live via the
/// time-based flush — not wait for the command to finish.
#[test]
fn real_bash_streaming_quiet_stretch() {
    let tmp = TempDir::new("bash_stream_quiet").unwrap();
    // `eval` forces the real `bash -c` route; `early` must flush from the
    // poll-loop tick during the sleep, not at process exit.
    let start = std::time::Instant::now();
    let (handle, rx) = spawn_stream("eval 'echo early; sleep 1.5'", &tmp.path, Some(10_000));
    let first = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("no live chunk");
    let first_at = start.elapsed();
    let result = handle.join().expect("streaming thread panicked");
    let total = start.elapsed();
    assert!(result.unwrap().contains("early"));
    assert!(first.contains("early"), "unexpected first chunk: {first:?}");
    // The live flush lands well before the ~1.5s command finish.
    assert!(
        first_at + std::time::Duration::from_millis(300) < total,
        "first chunk at {first_at:?} was not live (total {total:?})"
    );
}

/// Runaway output must not flood the sink: forwarded bytes stay within the
/// configured cap (plus the marker) while the final result still carries the
/// truncation marker.
#[test]
fn bashkit_streaming_output_capped() {
    let tmp = TempDir::new("bash_stream_cap").unwrap();
    // `eval` forces the real `bash -c` route; pure builtins emit ~300KB.
    let script = "eval 'i=0; while [ $i -lt 20000 ]; do echo 0123456789-$i; i=$((i+1)); done'";
    let (result, chunks) = stream_and_collect(script, &tmp.path, Some(20_000));
    let result = result.unwrap();
    let forwarded: usize = chunks.iter().map(String::len).sum();
    let cap = crabot::tools::tool_limits().max_output_bytes;
    let marker_len = crabot::tools::streaming_truncation_marker(cap).len();
    assert!(
        forwarded <= cap + marker_len,
        "forwarded {forwarded} > cap {cap} + marker {marker_len}"
    );
    assert!(
        chunks.concat().contains("streaming output truncated"),
        "expected truncation marker in live output"
    );
    assert!(
        result.contains("truncated"),
        "expected truncation marker in: {result}"
    );
}

// ── wrapper commands (env, xargs, timeout, find -exec, watch) ──
//
// Commands hidden in wrapper arguments must be extracted or the script falls
// back to real bash.

/// `timeout 5 git --version` — the wrapped name must be bridged.
#[test]
fn bashkit_timeout_wraps_host_command() {
    let result = run_bash("timeout 5 git --version", &crabot_workspace(), None).unwrap();
    assert!(result.contains("git version"), "unexpected: {result}");
}

/// `timeout 10 sh -c …` — a wrapped interpreter re-entry falls back to real
/// bash, exactly like a top-level `sh -c`.
#[test]
fn bashkit_timeout_nested_sh_falls_back() {
    let result = run_bash("timeout 10 sh -c 'echo wrapped'", &crabot_workspace(), None).unwrap();
    assert!(result.contains("wrapped"), "unexpected: {result}");
}

/// `timeout 10 $CMD` — a dynamic wrapped name falls back to real bash.
#[test]
fn bashkit_timeout_dynamic_command_falls_back() {
    let result = run_bash(
        "CMD=git; timeout 10 $CMD --version",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("git version"), "unexpected: {result}");
}

/// Attached option values (`-n1`) parse like bashkit's; each chunk runs the
/// bridged host command.
#[test]
fn bashkit_xargs_attached_option_runs_host_command() {
    let result = run_bash(
        "printf '%s\\n' README.md Cargo.toml | xargs -n1 git hash-object",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_sha_lines(&result, 2);
}

/// `xargs git` — the wrapped name must be bridged; input items become the
/// trailing args (`git --version`).
#[test]
fn bashkit_xargs_runs_host_command() {
    let result = run_bash(
        "printf '%s\\n' --version | xargs git",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("git version"), "unexpected: {result}");
}

/// Wrapped bashkit builtins stay in-process (`echo` needs no bridge).
#[test]
fn bashkit_xargs_builtin_command_stays_in_process() {
    let result = run_bash("printf 'a b\\n' | xargs echo", &crabot_workspace(), None).unwrap();
    assert!(result.contains("a b"), "unexpected: {result}");
}

/// `xargs $CMD` — a dynamic wrapped name falls back to real bash.
#[test]
fn bashkit_xargs_dynamic_command_falls_back() {
    let result = run_bash(
        "CMD=git; printf '%s\\n' --version | xargs $CMD",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("git version"), "unexpected: {result}");
}

/// `xargs -r` (GNU) is unknown to bashkit's parser — fall back to real bash.
#[test]
fn bashkit_xargs_unknown_option_falls_back() {
    let result = run_bash(
        "printf '%s\\n' --version | xargs -r git",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("git version"), "unexpected: {result}");
}

/// `find -exec` — the wrapped name must be bridged.
#[test]
fn bashkit_find_exec_runs_host_command() {
    let result = run_bash(
        "find . -maxdepth 1 -name Cargo.toml -exec git hash-object {} \\;",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_sha_lines(&result, 1);
}

/// `find -exec … +` batch mode — one invocation with all matches; the count
/// is workspace-dependent, so just check the hashes are all valid.
#[test]
fn bashkit_find_exec_batch_runs_host_command() {
    let result = run_bash(
        "find . -maxdepth 1 -name '*.toml' -exec git hash-object {} +",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_sha_lines(&result, 1);
}

/// `env CMD` — the stub refuses commands, so the script falls back to real
/// bash, which runs the wrapped command natively.
#[test]
fn bashkit_env_runs_host_command() {
    let result = run_bash("env FOO=bar git --version", &crabot_workspace(), None).unwrap();
    assert!(result.contains("git version"), "unexpected: {result}");
}

/// `env` print mode stays in-process and still sees exported variables.
#[test]
fn bashkit_env_print_shows_interpreter_env() {
    let result = run_bash(
        "export CRABOT_ENV_TEST=hello; env",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(
        result.lines().any(|l| l == "CRABOT_ENV_TEST=hello"),
        "unexpected: {result}"
    );
}

/// `watch`'s stub never runs the command; the script falls back to real bash.
/// Host `watch` times out or exits early without a terminal, but its output
/// must show the wrapped command.
#[test]
fn bashkit_watch_runs_host_command() {
    if !host_command_exists("watch") {
        eprintln!("skipping: no host `watch` executable");
        return;
    }
    let (result, chunks) =
        stream_and_collect("watch -n 1 git --version", &crabot_workspace(), Some(3000));
    let text = format!("{}{}", chunks.concat(), result.unwrap_or_else(|e| e));
    assert!(text.contains("git version"), "unexpected: {text}");
}

// ── PATH translation ────────────────────────────────────────

/// The interpreter seeds `$PATH` in MSYS form — a `:`-separated list of POSIX
/// paths, like a real Git Bash — not the host's native `;`-separated value.
#[test]
fn bashkit_path_is_msys_style() {
    let result = run_bash("echo \"$PATH\"", &crabot_workspace(), None).unwrap();
    let expected = crabot::tools::convert_path_list_to_posix(&std::env::var("PATH").unwrap());
    assert_eq!(result.trim(), expected, "unexpected: {result}");
    #[cfg(windows)]
    assert!(
        !result.contains([';', '\\']),
        "native PATH leaked: {result}"
    );
}

/// Native `...\System32` path of this host, for `PATH` round trips.
#[cfg(windows)]
fn system32_native() -> String {
    std::env::var("SystemRoot")
        .map(|root| format!("{root}\\System32"))
        .unwrap_or_else(|_| "C:\\Windows\\System32".to_string())
}

/// MSYS spelling of the same dir (`C:\Windows\System32` → `/c/Windows/System32`).
#[cfg(windows)]
fn system32_posix() -> String {
    crabot::tools::convert_path_to_unix_style(Path::new(&system32_native()))
}

/// Assert `where cmd` finds `cmd.exe` with `path_value` as the script `PATH`:
/// `where` resolves through its own environment, so only a native list works.
#[cfg(windows)]
fn assert_where_finds_cmd(path_value: &str) {
    let result = run_bash(
        &format!("PATH='{path_value}' where cmd"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(
        result.to_lowercase().contains("cmd.exe"),
        "unexpected: {result}"
    );
}

/// The seeded MSYS `PATH` reaches a host child in native form.
#[cfg(windows)]
#[test]
fn bashkit_host_command_gets_native_path() {
    assert_where_finds_cmd(&system32_posix());
}

/// A native `PATH` a script assigns itself keeps its drive colon when handed to
/// a host child (`C:\x` must not split into `C` and `\x`).
#[cfg(windows)]
#[test]
fn bashkit_native_script_path_reaches_host_command() {
    assert_where_finds_cmd(&system32_native());
}

/// A mixed `PATH` — a multi-entry POSIX list plus a native entry appended —
/// converts entry-wise: the probe dir is reachable only through the POSIX half.
#[cfg(windows)]
#[test]
fn bashkit_mixed_separator_path_reaches_host_command() {
    let tmp = TempDir::new("path_mixed").unwrap();
    fs::write(tmp.join("crabot_probe.cmd"), "@echo off\r\necho probe\r\n").unwrap();
    let dir = crabot::tools::convert_path_to_unix_style(&tmp.path);
    let path = format!("{dir}:{};C:\\Windows", system32_posix());
    let result = run_bash(
        &format!("PATH=\"{path}\" where crabot_probe.cmd"),
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert!(result.contains("crabot_probe.cmd"), "unexpected: {result}");
}

/// On Unix the interpreter's `PATH` is the host's own value, so a child sees it
/// verbatim.
#[cfg(unix)]
#[test]
fn bashkit_host_command_gets_host_path() {
    let result = run_bash("printenv PATH", &crabot_workspace(), None).unwrap();
    assert_eq!(
        result.trim(),
        std::env::var("PATH").unwrap(),
        "unexpected: {result}"
    );
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

// ── plain-text output (ANSI stripping) ──────────────────────

/// Styling escapes are dropped; tabs and the text survive (`tools::ansi`).
#[test]
fn strips_ansi_styling() {
    let result = run_bash(
        r"printf '\033[31mred\033[0m\ttext\n'",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(result, "red\ttext\n");
}

/// Progress redraws collapse to the frame a terminal would leave on screen.
#[test]
fn collapses_progress_redraws() {
    let result = run_bash(
        r"printf '10%%\r50%%\r100%%\ndone\n'",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(result, "100%\ndone\n");
}

/// OSC strings (window title, hyperlink target) vanish with their payload.
#[test]
fn drops_osc_sequences() {
    let result = run_bash(
        r"printf '\033]0;title\007\033]8;;https://example.com\007link\033]8;;\007\n'",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(result, "link\n");
}

/// The live stream shown while a command runs is plain text too, even when a
/// sequence is split across reads.
#[test]
fn streaming_output_is_plain_text() {
    let (result, chunks) = stream_and_collect(
        r"printf '\033[32mstep 1\033[0m\n10%%\r20%%\r30%%\n'",
        &crabot_workspace(),
        None,
    );
    assert_eq!(result.unwrap(), "step 1\n30%\n");
    assert_eq!(chunks.concat(), "step 1\n30%\n");
}

/// The common progress pattern: `\r`, erase, restyle, reprint, newline.
#[test]
fn progress_erase_restyle_reprint() {
    let result = run_bash(
        r"printf '50%%\r\033[2K\033[32m100%%\033[0m\n'",
        &crabot_workspace(),
        None,
    )
    .unwrap();
    assert_eq!(result, "100%\n");
}
