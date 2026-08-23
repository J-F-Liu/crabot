use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(windows)]
use crabot::tools::tmp_host_dir;
use crabot::tools::{
    COALESCE_MS, ChunkForwarder, OutStream, OutputSink, StreamingCap, ToolLimits,
    decode_stringified_args, resolve_path, resolve_path_partial, streaming_truncation_marker,
};
use serde_json::json;

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

/// Windows: the VFS `/tmp` resolves to the shared tmp host dir, so the file
/// tools agree with the `bash` tool's mount regardless of the process CWD.
#[cfg(windows)]
#[test]
fn resolve_tmp_maps_to_tmp_host_dir() {
    let tmp = TempDir::new("tmp_vfs").unwrap();
    let host_dir = tmp_host_dir(&tmp.path);
    // `/tmp` itself (created by tmp_host_dir, so canonicalize succeeds).
    assert_eq!(
        resolve_path("/tmp", &tmp.path).unwrap(),
        dunce::canonicalize(&host_dir).unwrap()
    );
    // `/tmp/<file>` appends to the same host dir.
    let probe = format!("crabot_tmp_vfs_{}", std::process::id());
    let host = host_dir.join(&probe);
    fs::write(&host, b"x").unwrap();
    assert_eq!(
        resolve_path(&format!("/tmp/{probe}"), &tmp.path).unwrap(),
        dunce::canonicalize(&host).unwrap()
    );
    let _ = fs::remove_file(&host);
}

/// Windows: `/tmpfoo` (no slash after `tmp`) is NOT the tmp mount — it stays
/// a CWD-drive root-relative path instead of capturing into `tmp_host_dir`.
#[cfg(windows)]
#[test]
fn resolve_tmp_prefix_does_not_capture_lookalikes() {
    let tmp = TempDir::new("tmp_look").unwrap();
    let resolved = resolve_path_partial("/tmpfoo", &tmp.path).unwrap();
    assert!(
        !resolved.starts_with(tmp_host_dir(&tmp.path)),
        "/tmpfoo wrongly captured by the tmp mount: {resolved:?}"
    );
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

// ── ToolLimits::sanitize ──────────────────────────────────────

/// Invalid settings (e.g. `max_command_timeout_ms < 1000`) are sanitized at
/// init, so the bash tool's `clamp(1000, max)` and its JSON schema
/// (`minimum <= maximum`) can never break.
#[test]
fn sanitize_keeps_timeouts_valid() {
    let mut limits = ToolLimits::new();
    limits.max_command_timeout_ms = 500;
    limits.command_timeout_ms = 20_000;
    limits.sanitize();
    assert_eq!(limits.max_command_timeout_ms, 1000);
    assert_eq!(limits.command_timeout_ms, 1000);

    let mut limits = ToolLimits::new();
    limits.max_command_timeout_ms = 10_000;
    limits.command_timeout_ms = 30_000;
    limits.sanitize();
    assert_eq!(limits.command_timeout_ms, 10_000);
}

// ── ChunkForwarder ─────────────────────────────────────────────

/// Forwarder whose chunks are collected into a Vec for inspection.
fn forwarder() -> (ChunkForwarder, Arc<Mutex<Vec<String>>>) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink: OutputSink = Arc::new({
        let captured = Arc::clone(&captured);
        move |chunk| captured.lock().unwrap().push(chunk.to_string())
    });
    (ChunkForwarder::new(Some(sink)), captured)
}

/// All captured chunks joined into one string.
fn joined(captured: &Mutex<Vec<String>>) -> String {
    captured.lock().unwrap().join("")
}

fn push_stdout(f: &mut ChunkForwarder, bytes: &[u8]) {
    f.push(OutStream::Stdout, bytes);
}

#[test]
fn normalizes_crlf_within_chunk() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"a\r\nb");
    f.finish();
    assert_eq!(joined(&out), "a\nb");
}

#[test]
fn normalizes_crlf_split_across_chunks() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"a\r");
    push_stdout(&mut f, b"\nb");
    f.finish();
    assert_eq!(joined(&out), "a\nb");
}

#[test]
fn keeps_trailing_bare_cr() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"a\r");
    f.finish();
    assert_eq!(joined(&out), "a\r");
}

#[test]
fn carries_incomplete_utf8_across_chunks() {
    let (mut f, out) = forwarder();
    // 中 = [0xE4, 0xB8, 0xAD], split 2 + 1.
    push_stdout(&mut f, &[0xE4, 0xB8]);
    push_stdout(&mut f, &[0xAD, b'x']);
    f.finish();
    assert_eq!(joined(&out), "中x");
}

#[test]
fn tick_flushes_time_due_pending() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"early");
    // Too small and too fresh to flush on push.
    assert!(joined(&out).is_empty());
    std::thread::sleep(COALESCE_MS + Duration::from_millis(20));
    f.tick();
    assert_eq!(joined(&out), "early");
}

#[test]
fn forwards_all_bytes() {
    // No cap here (that lives in `StreamingCap`): the sink gets the full stream.
    let (mut f, out) = forwarder();
    push_stdout(&mut f, &vec![b'x'; 300 * 1024]);
    f.finish();
    assert_eq!(joined(&out).len(), 300 * 1024);
}

// ── StreamingCap ─────────────────────────────────────────────

#[test]
fn streaming_cap_cuts_and_marks_once() {
    let mut c = StreamingCap::new(100);
    let first = c.push(&"x".repeat(60)).unwrap();
    assert_eq!(first.len(), 60);
    // Straddling chunk: keep the room, append the marker, then mute.
    let cut = c.push(&"y".repeat(80)).unwrap();
    assert_eq!(cut.len(), 40 + streaming_truncation_marker(100).len());
    assert!(cut.ends_with(&streaming_truncation_marker(100)));
    assert!(c.push("z").is_none(), "cut stream must stay muted");
}

#[test]
fn streaming_cap_exact_fill_marks_on_next_chunk() {
    let mut c = StreamingCap::new(100);
    assert_eq!(c.push(&"x".repeat(60)).unwrap().len(), 60);
    // Exact fill (across chunks): nothing dropped yet → no marker.
    let full = c.push(&"x".repeat(40)).unwrap();
    assert_eq!(full.len(), 40);
    assert!(!full.contains("truncated"));
    // First chunk past the fill is dropped → marker alone, then muted.
    let cut = c.push("y").unwrap();
    assert_eq!(cut, streaming_truncation_marker(100));
    assert!(c.push("z").is_none());
}

#[test]
fn streaming_cap_cuts_on_utf8_boundary() {
    let mut c = StreamingCap::new(5);
    // "é" = 2 bytes; 6 bytes total → cut at 4 bytes ("éé"), never mid-char.
    let cut = c.push("ééé").unwrap();
    let marker = streaming_truncation_marker(5);
    assert!(cut.ends_with(&marker));
    assert_eq!(cut.len() - marker.len(), 4);
    assert!(cut.starts_with("éé") && !cut.starts_with("ééé"));
}

#[test]
fn streaming_cap_zero_still_forwards_marker() {
    let mut c = StreamingCap::new(0);
    let cut = c.push("ab").unwrap();
    assert!(cut.starts_with('a')); // room = max(1)
    assert!(cut.contains("truncated"));
    assert!(c.push("c").is_none());
}

// ── decode_stringified_args ─────────────────────────────────

#[test]
fn decode_stringified_args_decodes_objects_and_arrays() {
    let schema = json!({
        "type": "object",
        "properties": {
            "obj": { "type": "object" },
            "arr": { "type": "array", "items": { "type": "string" } }
        }
    });
    let mut args = json!({
        "obj": r#"{"a":1}"#,
        "arr": r#"["x","y"]"#
    });
    decode_stringified_args(&schema, &mut args);
    assert_eq!(args["obj"], json!({ "a": 1 }));
    assert_eq!(args["arr"], json!(["x", "y"]));
}

#[test]
fn decode_stringified_args_recurses_into_properties_and_items() {
    let schema = json!({
        "type": "object",
        "properties": {
            "outer": {
                "type": "object",
                "properties": { "inner": { "type": "object" } }
            },
            "list": {
                "type": "array",
                "items": { "type": "object" }
            }
        }
    });
    let mut args = json!({
        "outer": { "inner": r#"{"deep":true}"# },
        "list": [r#"{"n":1}"#]
    });
    decode_stringified_args(&schema, &mut args);
    assert_eq!(args["outer"]["inner"], json!({ "deep": true }));
    assert_eq!(args["list"][0], json!({ "n": 1 }));
}

#[test]
fn decode_stringified_args_leaves_undecodable_or_wrong_kind_strings() {
    let schema = json!({
        "type": "object",
        "properties": {
            "obj": { "type": "object" },
            "arr": { "type": "array" },
            "text": { "type": "string" }
        }
    });
    let mut args = json!({
        "obj": "not json",
        "arr": r#"{"not":"an array"}"#,
        "text": r#"{"a":1}"#
    });
    decode_stringified_args(&schema, &mut args);
    assert_eq!(args["obj"], "not json");
    assert_eq!(args["arr"], r#"{"not":"an array"}"#);
    // A string field is never decoded, even when it happens to be JSON.
    assert_eq!(args["text"], r#"{"a":1}"#);
}

// ── tool-execution tab scope ─────────────────────────────────────

#[test]
fn tab_scope_nests_and_restores() {
    use crabot::tools::{current_tab_number, with_tab_scope};

    assert!(current_tab_number().is_none());
    with_tab_scope(2, || {
        assert_eq!(current_tab_number(), Some(2));
        with_tab_scope(7, || {
            assert_eq!(current_tab_number(), Some(7));
        });
        assert_eq!(current_tab_number(), Some(2));
    });
    assert!(current_tab_number().is_none());
}

#[test]
fn tab_scope_restores_after_panic() {
    use crabot::tools::{current_tab_number, with_tab_scope};

    assert!(current_tab_number().is_none());
    let panicked = std::panic::catch_unwind(|| {
        with_tab_scope(3, || {
            assert_eq!(current_tab_number(), Some(3));
            panic!("tool panicked");
        });
    });
    assert!(panicked.is_err());
    assert!(current_tab_number().is_none());
}
