use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crabot::tools::{
    COALESCE_MS, ChunkForwarder, OutputSink, resolve_path, resolve_path_partial, tool_limits,
};

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

    fn mkfile(&self, name: &str) -> io::Result<PathBuf> {
        let p = self.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&p, b"")?;
        Ok(p)
    }

    fn mkdir(&self, name: &str) -> io::Result<PathBuf> {
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
    tmp.mkdir("sub").unwrap();
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

// ── ChunkForwarder ─────────────────────────────────────────────

/// Forwarder whose chunks are collected into a Vec for inspection.
fn forwarder() -> (ChunkForwarder, Arc<Mutex<Vec<String>>>) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink: OutputSink = Arc::new({
        let captured = Arc::clone(&captured);
        move |chunk| captured.lock().unwrap().push(chunk.to_string())
    });
    (ChunkForwarder::new(sink), captured)
}

/// All captured chunks joined into one string.
fn joined(captured: &Mutex<Vec<String>>) -> String {
    captured.lock().unwrap().join("")
}

#[test]
fn normalizes_crlf_within_chunk() {
    let (mut f, out) = forwarder();
    f.push(b"a\r\nb");
    f.finish();
    assert_eq!(joined(&out), "a\nb");
}

#[test]
fn normalizes_crlf_split_across_chunks() {
    let (mut f, out) = forwarder();
    f.push(b"a\r");
    f.push(b"\nb");
    f.finish();
    assert_eq!(joined(&out), "a\nb");
}

#[test]
fn keeps_trailing_bare_cr() {
    let (mut f, out) = forwarder();
    f.push(b"a\r");
    f.finish();
    assert_eq!(joined(&out), "a\r");
}

#[test]
fn carries_incomplete_utf8_across_chunks() {
    let (mut f, out) = forwarder();
    // 中 = [0xE4, 0xB8, 0xAD], split 2 + 1.
    f.push(&[0xE4, 0xB8]);
    f.push(&[0xAD, b'x']);
    f.finish();
    assert_eq!(joined(&out), "中x");
}

#[test]
fn tick_flushes_time_due_pending() {
    let (mut f, out) = forwarder();
    f.push(b"early");
    // Too small and too fresh to flush on push.
    assert!(joined(&out).is_empty());
    std::thread::sleep(COALESCE_MS + Duration::from_millis(20));
    f.tick();
    assert_eq!(joined(&out), "early");
}

#[test]
fn caps_forwarded_bytes() {
    let (mut f, out) = forwarder();
    f.push(&vec![b'x'; 300 * 1024]);
    f.finish();
    let total = joined(&out).len();
    let cap = tool_limits().max_output_bytes;
    assert!(total <= cap, "forwarded {total} > cap {cap}");
    assert!(total > 0, "nothing forwarded");
}
