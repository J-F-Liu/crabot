mod common;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::TempDir;
#[cfg(windows)]
use crabot::tools::tmp_host_dir;
use crabot::tools::{
    COALESCE_MS, ChunkForwarder, ImageAttachment, ImageBudget, OutStream, OutputSink, RouteImages,
    StreamingCap, Tool, ToolLimits, decode_stringified_args, init_tool_limits, resolve_path,
    resolve_path_partial, streaming_truncation_marker,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

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
    let host_dir = tmp_host_dir();
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
        !resolved.starts_with(tmp_host_dir()),
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

/// A trailing `\r` (its `\n` never arrives) keeps the frame it drew and
/// never leaks a raw control character into the stream.
#[test]
fn trailing_bare_cr_leaves_its_frame() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"a\r");
    // Held back: the next chunk decides between a line end and a redraw.
    assert_eq!(joined(&out), "");
    f.finish();
    assert_eq!(joined(&out), "a");
}

/// A held `\r` followed by an escape sequence and a `\n` keeps the frame:
/// styling doesn't redraw, and the split across chunks changes nothing.
#[test]
fn escape_after_held_cr_keeps_the_frame() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"100%\r");
    push_stdout(&mut f, b"\x1b[0m\n");
    f.finish();
    assert_eq!(joined(&out), "100%\n");
}

/// A held `\r` followed by a tab doesn't redraw: the tab moves the cursor
/// off column 0, so the frame survives and the tabbed text follows it.
#[test]
fn tab_after_held_cr_keeps_the_frame() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"100%\r");
    push_stdout(&mut f, b"\tdone\n");
    f.finish();
    assert_eq!(joined(&out), "100%\tdone\n");
}

/// `ESC[K` erases the held frame even when the sequence arrives in the next
/// chunk; a private-marker CSI (`ESC[?25l`) does not.
#[test]
fn erase_in_line_split_across_chunks() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"abc\r");
    push_stdout(&mut f, b"\x1b[K\n");
    push_stdout(&mut f, b"def\r\x1b[?25l\n");
    f.finish();
    assert_eq!(joined(&out), "\ndef\n");
}

/// `ESC[K` with the cursor at end of line (no held `\r`) erases nothing;
/// the printed text survives.
#[test]
fn erase_in_line_at_end_of_line_keeps_text() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"abc\x1b[K\n");
    f.finish();
    assert_eq!(joined(&out), "abc\n");
}

/// An empty push contributes no chunk to the stream.
#[test]
fn empty_push_emits_nothing() {
    let (mut f, out) = forwarder();
    push_stdout(&mut f, b"");
    f.finish();
    assert!(out.lock().unwrap().is_empty(), "chunks: {:?}", joined(&out));
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

// ── read: image attachments ─────────────────────────────────────────

/// Tool limits are process-global, so tests that set them must not run
/// alongside tests that read them.
static LIMITS: Mutex<()> = Mutex::new(());

/// Write a small solid PNG into `dir`.
fn write_png(dir: &TempDir, name: &str, width: u32, height: u32) {
    let img = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
    img.save(dir.join(name)).unwrap();
}

/// A solid PNG of the given size, encoded in memory.
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    buf
}

/// Run `read` on `path` inside `dir`, returning the result text and attachments.
fn read(path: serde_json::Value, dir: &TempDir) -> (String, Vec<ImageAttachment>) {
    try_read(path, dir).unwrap()
}

/// Same as [`read`], but keeps the error instead of unwrapping it.
fn try_read(
    path: serde_json::Value,
    dir: &TempDir,
) -> Result<(String, Vec<ImageAttachment>), String> {
    let (result, attached) = crabot::tools::read::ReadTool.execute_with_attachments(
        &json!({ "path": path }),
        &dir.path,
        &CancellationToken::new(),
    );
    result.map(|text| (text, attached))
}

/// Publish a route so `read` sees this tab as (non-)vision-capable.
fn set_route(tab: usize, vision: bool, model_id: &str) {
    crabot::tools::set_route_images(
        tab,
        RouteImages {
            vision,
            model_id: model_id.to_string(),
            budget: ImageBudget::from_limits(&ToolLimits::new()),
        },
    );
}

/// Images are reported by path and dimensions instead of being dumped as bytes,
/// and offer themselves to the LLM layer as attachments.
#[test]
fn read_attaches_images_instead_of_bytes() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_image").unwrap();
    write_png(&tmp, "shot.png", 3, 2);

    let (result, attached) = read("shot.png".into(), &tmp);
    assert!(
        result.starts_with("[Image] ")
            && result.contains("image/png, ")
            && result.ends_with("3x2)"),
        "{result}"
    );

    assert_eq!(attached.len(), 1);
    assert!(attached[0].path.ends_with("shot.png"), "{:?}", attached[0]);
    assert_eq!((attached[0].width, attached[0].height), (3, 2));
    assert_eq!(attached[0].media_type, "image/png");
}

/// Non-image files keep the line-numbered text path, with nothing attached.
#[test]
fn read_keeps_text_for_non_images() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_text").unwrap();
    tmp.write("notes.txt", b"hello\n").unwrap();

    let (result, attached) = read("notes.txt".into(), &tmp);
    assert!(result.contains("1|hello"), "{result}");
    assert!(attached.is_empty());
}

/// A file named like an image but not decodable errors instead of dumping
/// binary garbage as text.
#[test]
fn read_errors_for_broken_images() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_broken").unwrap();
    tmp.write("broken.png", b"not a png\n").unwrap();

    let result = try_read("broken.png".into(), &tmp).unwrap_err();
    assert!(result.contains("Failed to decode image"), "{result}");
}

/// A file over the byte budget but within the pixel budgets is still
/// attached: the encoder re-compresses it at the same dimensions instead of
/// the read tool refusing it.
#[test]
fn read_attaches_images_over_the_byte_budget() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_big_bytes").unwrap();
    // Noise compresses badly, so a small raster still exceeds a modest byte cap.
    let noise = image::RgbImage::from_fn(128, 128, |x, y| {
        image::Rgb([(x * 7 + y * 13) as u8, (x * 31) as u8, (y * 17) as u8])
    });
    noise.save(tmp.join("noise.png")).unwrap();

    let mut limits = ToolLimits::new();
    limits.read_max_image_bytes = 32 * 1024; // the PNG on disk is bigger
    init_tool_limits(limits);
    let budget = ImageBudget::from_limits(&limits);

    let (result, attached) = read("noise.png".into(), &tmp);
    assert_eq!(attached.len(), 1, "{result}");
    // Same dimensions on the wire, so nothing is reported as downscaled.
    assert_eq!((attached[0].width, attached[0].height), (128, 128));
    assert!(attached[0].source_width.is_none(), "{result}");

    let encoded = crabot::tools::encode_for_request(&tmp.join("noise.png"), &budget).unwrap();
    assert_eq!((encoded.width, encoded.height), (128, 128));
    assert!(
        encoded.data.len() <= budget.max_bytes,
        "{} bytes exceed the {} cap",
        encoded.data.len(),
        budget.max_bytes
    );

    init_tool_limits(ToolLimits::new());
}

/// An image outside the workspace round-trips: the marker records a displayable
/// path, and resolving it again finds the same file on the next request.
#[test]
fn read_attaches_images_outside_the_workspace() {
    let _limits = crabot::lock(&LIMITS);
    let ws = TempDir::new("read_ws").unwrap();
    let outside = TempDir::new("read_outside").unwrap();
    write_png(&outside, "shot.png", 2, 2);
    let path = outside.join("shot.png");

    let (result, attached) = read(path.to_string_lossy().into(), &ws);
    assert!(result.starts_with("[Image] "), "{result}");

    assert_eq!(attached.len(), 1);

    let marker = crabot::chat::image_marker_message(&[attached[0].path.clone()]);
    let paths = crabot::chat::image_marker_paths(&marker).unwrap();
    assert_eq!(
        crabot::tools::resolve_path(paths[0], &ws.path).unwrap(),
        dunce::canonicalize(&path).unwrap()
    );
}

/// An image past the pixel budget is reported at the size the request will
/// carry, with the multiplier that maps coordinates back onto the file.
#[test]
fn read_reports_the_downscaled_size_and_coordinate_advice() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_downscale").unwrap();
    write_png(&tmp, "big.png", 400, 200);

    let mut limits = ToolLimits::new();
    limits.read_max_image_pixels = 20_000; // 4000 px², so 200x100
    init_tool_limits(limits);

    let (result, attached) = read("big.png".into(), &tmp);
    assert_eq!(attached.len(), 1);
    assert_eq!((attached[0].width, attached[0].height), (200, 100));
    assert_eq!(attached[0].source_width, Some(400));
    assert!(result.contains("downscaled from 400x200 px"), "{result}");
    assert!(result.contains("multiply coordinates by 2.00"), "{result}");

    init_tool_limits(ToolLimits::new());
}

/// A file whose bytes disagree with its extension is refused with the fix,
/// rather than shipped under the wrong MIME type.
#[test]
fn read_rejects_an_extension_that_contradicts_the_bytes() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_mismatch").unwrap();
    tmp.write("shot.jpg", &png_bytes(4, 4)).unwrap();

    let result = try_read("shot.jpg".into(), &tmp).unwrap_err();
    assert!(result.contains("declares image/jpeg"), "{result}");
    assert!(result.contains("bytes are image/png"), "{result}");
}

/// An image format with no extension is recognized from its own bytes.
#[test]
fn read_recognizes_an_extensionless_image() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_signature").unwrap();
    tmp.write("screenshot", &png_bytes(4, 4)).unwrap();

    let (result, attached) = read("screenshot".into(), &tmp);
    assert_eq!(attached.len(), 1, "{result}");
    assert_eq!(attached[0].media_type, "image/png");
}

/// A truncated file fails the full decode that admission performs, instead of
/// reaching a provider as a valid-looking header.
#[test]
fn read_rejects_a_truncated_image() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_truncated").unwrap();
    let mut png = png_bytes(64, 64);
    png.truncate(png.len() / 2);
    tmp.write("half.png", &png).unwrap();

    let result = try_read("half.png".into(), &tmp).unwrap_err();
    assert!(result.contains("Failed to decode image"), "{result}");
}

/// A route that cannot see pictures refuses the read outright — silently
/// returning the path would let the model describe a picture it never saw.
#[test]
fn read_refuses_images_for_a_text_only_model() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_blind").unwrap();
    write_png(&tmp, "shot.png", 4, 4);

    set_route(77, false, "text-only-1");
    let result =
        crabot::tools::with_tab_scope(77, || try_read("shot.png".into(), &tmp).unwrap_err());

    assert!(result.contains("text-only-1"), "{result}");
    assert!(result.contains("does not accept image input"), "{result}");
}

/// The same read succeeds once the route declares image input.
#[test]
fn read_attaches_images_for_a_vision_model() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_sighted").unwrap();
    write_png(&tmp, "shot.png", 4, 4);

    set_route(78, true, "vision-1");
    let (result, attached) = crabot::tools::with_tab_scope(78, || read("shot.png".into(), &tmp));

    assert_eq!(attached.len(), 1, "{result}");
    assert_eq!((attached[0].width, attached[0].height), (4, 4));
}

/// A picture past the byte budget is still attached when downscaling brings
/// it back under — refusing outright would be the wrong answer.
#[test]
fn oversized_images_are_encoded_down_instead_of_refused() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("read_encode").unwrap();
    // Noise compresses badly, so the file is far larger than its pixels need.
    let noise = image::RgbImage::from_fn(1200, 900, |x, y| {
        image::Rgb([(x * 7 + y * 13) as u8, (x * 31) as u8, (y * 17) as u8])
    });
    noise.save(tmp.join("noise.png")).unwrap();

    let mut limits = ToolLimits::new();
    limits.read_max_image_pixels = 250_000;
    init_tool_limits(limits);
    let budget = ImageBudget::from_limits(&limits);

    let source_bytes = fs::metadata(tmp.join("noise.png")).unwrap().len();
    let (result, attached) = read("noise.png".into(), &tmp);
    assert_eq!(attached.len(), 1, "{result}");
    assert!(attached[0].source_width.is_some(), "{result}");

    let encoded = crabot::tools::encode_for_request(&tmp.join("noise.png"), &budget).unwrap();
    assert_eq!(
        (encoded.width, encoded.height),
        (attached[0].width, attached[0].height),
        "the request must carry the size read advertised"
    );
    assert!(
        (encoded.data.len() as u64) < source_bytes,
        "{} vs {source_bytes}",
        encoded.data.len()
    );

    init_tool_limits(ToolLimits::new());
}

/// A big raster is downscaled to the budget and encoded, and the encoder is
/// checked to have produced the dimensions the read tool advertised.
#[test]
fn request_encoding_downscales_and_keeps_the_promised_size() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("encode_downscale").unwrap();
    let path = tmp.join("big.png");
    image::RgbaImage::from_pixel(3200, 2400, image::Rgba([3, 200, 40, 255]))
        .save(&path)
        .unwrap();

    let budget = ImageBudget::from_limits(&ToolLimits::new());
    let encoded = crabot::tools::encode_for_request(&path, &budget).unwrap();
    assert_eq!(
        (encoded.width, encoded.height),
        crabot::tools::target_dimensions(3200, 2400, &budget)
    );
    assert!(encoded.width as u64 * encoded.height as u64 <= budget.max_pixels);
    assert!(encoded.width <= budget.max_side);
    assert!(encoded.data.len() < std::fs::metadata(&path).unwrap().len() as usize);
}

/// A fully opaque alpha plane is not worth the lossless codec: the picture
/// still goes out as the far smaller JPEG.
#[test]
fn opaque_sources_encode_as_jpeg() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("encode_opaque").unwrap();
    let path = tmp.join("flat.png");
    image::RgbaImage::from_pixel(256, 256, image::Rgba([1, 2, 3, 255]))
        .save(&path)
        .unwrap();

    let budget = ImageBudget {
        max_pixels: 1024, // forces a re-encode
        max_side: 64,
        max_bytes: 1024 * 1024,
    };
    let encoded = crabot::tools::encode_for_request(&path, &budget).unwrap();
    assert_eq!(encoded.media_type, "image/jpeg");
}

/// Transparency keeps the alpha channel by routing through WebP.
#[test]
fn transparent_sources_encode_as_webp() {
    let _limits = crabot::lock(&LIMITS);
    let tmp = TempDir::new("encode_alpha").unwrap();
    let path = tmp.join("cutout.png");
    image::RgbaImage::from_pixel(256, 256, image::Rgba([1, 2, 3, 0]))
        .save(&path)
        .unwrap();

    let budget = ImageBudget {
        max_pixels: 1024,
        max_side: 64,
        max_bytes: 1024 * 1024,
    };
    let encoded = crabot::tools::encode_for_request(&path, &budget).unwrap();
    assert_eq!(encoded.media_type, "image/webp");
}
