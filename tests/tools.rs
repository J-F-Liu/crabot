use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crabot::tools::{resolve_path, resolve_path_partial};

/// Helper: create a temp workspace dir that is cleaned up on drop.
struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> io::Result<Self> {
        let mut dir = std::env::temp_dir();
        dir.push(format!("crabot_test_{}_{}", prefix, std::process::id()));
        let _ = fs::remove_dir_all(&dir); // clean any left‑over
        fs::create_dir_all(&dir)?;
        Ok(Self { path: dir })
    }

    fn join(&self, name: &str) -> std::path::PathBuf {
        self.path.join(name)
    }

    fn mkfile(&self, name: &str) -> io::Result<std::path::PathBuf> {
        let p = self.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&p, b"")?;
        Ok(p)
    }

    fn mkdir(&self, name: &str) -> io::Result<std::path::PathBuf> {
        let p = self.join(name);
        fs::create_dir_all(&p)?;
        Ok(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// ── resolve_path ────────────────────────────────────────────

#[test]
fn resolve_absolute_existing() {
    let tmp = TempDir::new("abs").unwrap();
    let f = tmp.mkfile("foo.txt").unwrap();
    let result = resolve_path(&f.to_string_lossy(), &tmp.path);
    assert_eq!(result.unwrap(), dunce::canonicalize(&f).unwrap());
}

#[test]
fn resolve_absolute_nonexistent() {
    let tmp = TempDir::new("abs_miss").unwrap();
    let ghost = tmp.join("does_not_exist.txt");
    let result = resolve_path(&ghost.to_string_lossy(), &tmp.path);
    assert!(result.is_err());
}

#[test]
fn resolve_relative_existing() {
    let tmp = TempDir::new("rel").unwrap();
    let f = tmp.mkfile("sub/dir/file.txt").unwrap();
    let result = resolve_path("sub/dir/file.txt", &tmp.path);
    assert_eq!(result.unwrap(), dunce::canonicalize(&f).unwrap());
}

#[test]
fn resolve_relative_with_dot_dot() {
    let tmp = TempDir::new("dotdot").unwrap();
    let f = tmp.mkfile("target.txt").unwrap();
    // go into sub/, then come back with ..
    let result = resolve_path("sub/../target.txt", &tmp.path);
    assert_eq!(result.unwrap(), dunce::canonicalize(&f).unwrap());
}

#[test]
fn resolve_relative_nonexistent() {
    let tmp = TempDir::new("rel_miss").unwrap();
    let result = resolve_path("ghost.txt", &tmp.path);
    assert!(result.is_err());
}

// ── resolve_path_partial ─────────────────────────────────────

#[test]
fn partial_existing_file() {
    let tmp = TempDir::new("part_ex").unwrap();
    let f = tmp.mkfile("a/b/c.txt").unwrap();
    let result = resolve_path_partial("a/b/c.txt", &tmp.path).unwrap();
    assert_eq!(result, dunce::canonicalize(&f).unwrap());
}

#[test]
fn partial_nonexistent_leaf() {
    let tmp = TempDir::new("part_leaf").unwrap();
    tmp.mkdir("a/b").unwrap();
    let result = resolve_path_partial("a/b/new_file.txt", &tmp.path).unwrap();
    let expected = dunce::canonicalize(tmp.join("a/b"))
        .unwrap()
        .join("new_file.txt");
    assert_eq!(result, expected);
    assert!(!result.exists()); // leaf itself must not exist
}

#[test]
fn partial_nonexistent_mid_dir() {
    let tmp = TempDir::new("part_mid").unwrap();
    tmp.mkdir("a").unwrap(); // only "a" exists
    let result = resolve_path_partial("a/b/c/new_file.txt", &tmp.path).unwrap();
    let expected = dunce::canonicalize(tmp.join("a"))
        .unwrap()
        .join("b")
        .join("c")
        .join("new_file.txt");
    assert_eq!(result, expected);
}

#[test]
fn partial_nothing_exists() {
    let tmp = TempDir::new("part_none").unwrap();
    // workspace exists but "x/y/z" and all ancestors are missing
    let result = resolve_path_partial("x/y/z/file.txt", &tmp.path).unwrap();
    // falls back to workspace-joined candidate
    assert_eq!(result, tmp.join("x/y/z/file.txt"));
}

#[test]
fn partial_dot_dot() {
    let tmp = TempDir::new("part_dd").unwrap();
    tmp.mkdir("sub").unwrap();
    let f = tmp.mkfile("target.txt").unwrap();
    let result = resolve_path_partial("sub/../target.txt", &tmp.path).unwrap();
    assert_eq!(result, dunce::canonicalize(&f).unwrap());
}

// ── candidate_path (edge cases via resolve_path*) ──────────

#[test]
fn empty_path_resolves_to_workspace() {
    let tmp = TempDir::new("empty").unwrap();
    // empty string → workspace.join("") which is the workspace dir itself
    let result = resolve_path("", &tmp.path).unwrap();
    assert_eq!(result, dunce::canonicalize(&tmp.path).unwrap());
}

#[test]
fn just_filename_resolves_in_workspace() {
    let tmp = TempDir::new("fn").unwrap();
    let f = tmp.mkfile("readme.md").unwrap();
    let result = resolve_path("readme.md", &tmp.path).unwrap();
    assert_eq!(result, dunce::canonicalize(&f).unwrap());
}

// ── empty workspace ──────────────────────────────────────────

#[test]
fn empty_workspace_relative_is_cwd_relative() {
    // When workspace is empty, a relative path resolves against CWD.
    // The project root always has "Cargo.toml", so use that as a stable target.
    let result = resolve_path("Cargo.toml", Path::new(""));
    assert!(result.is_ok());
    let resolved = result.unwrap();
    assert!(resolved.is_file());
    assert!(resolved.ends_with("Cargo.toml"));
}

#[test]
fn empty_workspace_absolute_still_works() {
    let tmp = TempDir::new("empty_abs").unwrap();
    let f = tmp.mkfile("some_file.txt").unwrap();
    let result = resolve_path(&f.to_string_lossy(), Path::new(""));
    assert_eq!(result.unwrap(), dunce::canonicalize(&f).unwrap());
}

#[test]
fn empty_workspace_relative_nonexistent_is_err() {
    // A relative path that doesn't exist → error.
    let result = resolve_path("__crabot_nonesuch_xyz__", Path::new(""));
    assert!(result.is_err());
}

#[test]
fn empty_workspace_partial_cwd_relative() {
    // "src" exists in the project root. Append a non-existent leaf.
    let result = resolve_path_partial("src/__crabot_nonesuch_xyz__", Path::new(""));
    let expected = dunce::canonicalize("src")
        .unwrap()
        .join("__crabot_nonesuch_xyz__");
    assert_eq!(result.unwrap(), expected);
}

#[test]
fn empty_workspace_empty_path() {
    // empty path + empty workspace: dunce::canonicalize("") errors
    let result = resolve_path("", Path::new(""));
    assert!(result.is_err());
}

// ── bash tool (bashkit in-process route + bash -c fallback) ────────

use std::sync::atomic::AtomicBool;

use crabot::tools::ToolRegistry;

/// The `cargo test` runner inherits Git Bash's POSIX-style PATH (`/usr/bin:...`),
/// which Windows `CreateProcess` cannot search. Normalize it to Windows form
/// (once per process) so children like `bash`, `cargo`, and `git` resolve.
fn fix_test_path() {
    use std::sync::OnceLock;
    static FIXED: OnceLock<()> = OnceLock::new();
    FIXED.get_or_init(|| {
        // `cargo` already converts the shell PATH to Windows form (`;`
        // separators) when spawning the test binary; only convert a
        // POSIX-style (`:`) PATH.
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
        // SAFETY: every test sets the same value and no other code reads
        // PATH concurrently in a way that matters here.
        unsafe {
            std::env::set_var("PATH", windows.join(";"));
        }
    });
}

/// The crabot repository root (used as the test workspace where git exists).
fn crabot_workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Run the `bash` tool exactly like `llm.rs::exec_tool` does: on a blocking
/// thread with a tokio runtime context.
fn run_bash(command: &str, workspace: &Path, timeout_ms: Option<u64>) -> Result<String, String> {
    fix_test_path();
    let registry = ToolRegistry::new();
    let bash = registry.find_tool("bash").expect("bash tool registered");
    let mut args = serde_json::json!({ "command": command });
    if let Some(ms) = timeout_ms {
        args["timeout"] = serde_json::json!(ms);
    }
    let workspace = workspace.to_path_buf();
    let cancel = AtomicBool::new(false);
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

/// `cargo --version 2>&1 | tail -1` — pipeline + stderr redirect.
#[test]
fn bashkit_pipe_and_stderr_redirect() {
    let result = run_bash(
        "git status --definitely-not-a-flag 2>&1 | tail -2",
        Path::new(""),
        None,
    )
    .unwrap();
    // git's usage output goes to stderr, is merged via 2>&1 and piped to tail.
    assert!(!result.is_empty());
    assert!(result.contains("git"), "unexpected: {result}");
}

/// `cd src && git status` — cwd persists via ctx.cwd and maps to the host.
#[test]
fn bashkit_cd_cwd_mapping() {
    let result = run_bash("cd src && git status --short", &crabot_workspace(), None).unwrap();
    // The crabot repo itself is the workspace here; status must succeed.
    assert!(!result.contains("Exit code"), "unexpected: {result}");
}

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

/// Non-zero exit codes are reported like the bash -c path.
#[test]
fn bashkit_exit_code_reporting() {
    let tmp = TempDir::new("bash_exit").unwrap();
    let result = run_bash("git status", &tmp.path, None).unwrap();
    assert!(result.contains("fatal:"), "unexpected: {result}");
    assert!(result.contains("Exit code: 128"), "unexpected: {result}");
}

/// `sleep 100` with a 3s timeout — the outer tokio select fires.
#[test]
fn bashkit_sleep_timeout() {
    let err = run_bash("sleep 100", &crabot_workspace(), Some(3000)).unwrap_err();
    assert!(err.contains("timed out"), "unexpected: {err}");
}

/// `while true; do :; done` — loop-iteration limit terminates, no hang.
#[test]
fn bashkit_dead_loop_limit() {
    let err = run_bash("while true; do :; done", &crabot_workspace(), Some(30_000)).unwrap_err();
    assert!(err.contains("loop"), "unexpected: {err}");
}

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

/// Plan case: `cd src && cargo check` — cwd persists through ctx.cwd and
/// a real build-tool command runs through the host bridge.
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

/// Plan case: `cargo build --release && git status` (dev-profile equivalent:
/// `cargo check && git status`) — two host commands joined by `&&`.
#[test]
fn bashkit_host_commands_and_list() {
    let result = run_bash(
        "cargo check --quiet && git status --short",
        &crabot_workspace(),
        Some(120_000),
    )
    .unwrap();
    assert!(!result.contains("Exit code"), "unexpected: {result}");
}
