use std::fmt::Write;
use std::io::{BufRead, BufReader};
use std::path::Path;

use tokio_util::sync::CancellationToken;

use serde_json::{Value, json};

use crate::tools::{
    CANCEL_REASON, ImageAttachment, Tool, arg_u64, human_bytes, make_workspace_relative,
    required_path, resolve_path, tool_limits,
};

/// Image extensions read as pictures rather than as text, with their MIME type.
const IMAGE_TYPES: [(&str, &str); 6] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
];

/// Longest image side the vision APIs accept (Anthropic rejects more).
const MAX_IMAGE_SIDE: u32 = 8000;

pub struct ReadTool;

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a file from the filesystem with line-numbered output. Supports offset and line-limit pagination. Image files are attached to the conversation so they can be looked at."
    }

    fn instruction(&self) -> &str {
        "When reading files, prefer larger, context-rich reads over multiple small consecutive reads. Large files may be truncated with a marker such as \"[213 more lines in file. Use offset=2000 to continue.]\". You can use the `read` tool to load additional content if needed. Never pass the truncation marker to an edit tool. You don't need to read a file if it's already provided in context. Reading an image file (.png, .jpg, .jpeg, .gif, .webp, .bmp) attaches the picture itself to the conversation instead of returning its bytes."
    }

    fn schema(&self) -> Value {
        let max_lines = tool_limits().read_max_lines;
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file (relative to workspace or absolute)"
                },
                "offset": {
                    "type": "integer",
                    "description": "1-based line number to start reading from (default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": format!("Maximum number of lines to read (default: {max_lines}, capped at {max_lines})")
                }
            },
            "required": ["path"]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        _cancel: &CancellationToken,
    ) -> Result<String, String> {
        run(args, workspace).0
    }

    fn execute_with_attachments(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> (Result<String, String>, Vec<ImageAttachment>) {
        if cancel.is_cancelled() {
            return (Err(CANCEL_REASON.into()), Vec::new());
        }
        run(args, workspace)
    }
}

/// Probe the file once and produce both the result text and any attachment.
fn run(args: &Value, workspace: &Path) -> (Result<String, String>, Vec<ImageAttachment>) {
    match read_image(args, workspace) {
        Some(ReadImage::Attached(image)) => {
            let text = format!("[Image] {}", image.summary());
            (Ok(text), vec![image])
        }
        Some(ReadImage::Skipped { image, reason }) => {
            let text = format!("[Image] {} — not attached: {reason}.", image.summary());
            (Ok(text), Vec::new())
        }
        // An image file that won't decode is never worth dumping as text.
        Some(ReadImage::Undecodable(reason)) => (Err(reason), Vec::new()),
        None => (execute(args, workspace), Vec::new()),
    }
}

/// Outcome of probing an image file: attached to the model, or reported by
/// description (in `run`) because the provider would reject it.
enum ReadImage {
    Attached(ImageAttachment),
    Skipped {
        image: ImageAttachment,
        reason: String,
    },
    /// The extension says image, but the bytes don't decode.
    Undecodable(String),
}

/// The image a `read` call refers to: extension must be an image type and the
/// file must decode with a non-zero size. `None` ⇒ read it as text.
fn read_image(args: &Value, workspace: &Path) -> Option<ReadImage> {
    let path = required_path(args).ok()?;
    let media_type = IMAGE_TYPES
        .iter()
        .find(|(ext, _)| {
            Path::new(path)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .map(|(_, mime)| *mime)?;
    let file_path = resolve_path(path, workspace).ok()?;
    let bytes = std::fs::metadata(&file_path).ok()?.len();
    let (width, height) = match image::image_dimensions(&file_path) {
        Ok(dimensions) => dimensions,
        Err(e) => {
            let display = make_workspace_relative(&file_path, workspace);
            return Some(ReadImage::Undecodable(format!(
                "Failed to decode image {display}: {e}"
            )));
        }
    };
    if width == 0 || height == 0 {
        return None;
    }
    let image = ImageAttachment {
        path: make_workspace_relative(&file_path, workspace),
        media_type: media_type.to_string(),
        width,
        height,
        bytes,
    };
    let limit = tool_limits().read_max_image_bytes as u64;
    if bytes > limit {
        let reason = format!("larger than {}", human_bytes(limit));
        return Some(ReadImage::Skipped { image, reason });
    }
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        let reason = format!("over {MAX_IMAGE_SIDE}px on a side");
        return Some(ReadImage::Skipped { image, reason });
    }
    Some(ReadImage::Attached(image))
}

/// Number of decimal digits of `n` (0 → 1, 5 → 1, 99 → 2, …).
const fn digit_count(mut n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut d = 0;
    while n > 0 {
        n /= 10;
        d += 1;
    }
    d
}

/// Exact formatted length of `"{:>4}:{line}\n"` without allocating.
fn formatted_len(line_num: usize, line: &str) -> usize {
    digit_count(line_num).max(4) + 1 + line.len() + 1 // padding + colon + content + newline
}

/// Strip trailing `\n` or `\r\n` from a [`BufRead::read_line`] result.
fn strip_newline(s: &str) -> &str {
    s.strip_suffix('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .unwrap_or(s)
}

pub(super) fn execute(args: &Value, workspace: &Path) -> Result<String, String> {
    let limits = tool_limits();
    let max_lines_cap = limits.read_max_lines;
    let max_bytes = limits.read_max_bytes;

    let path = required_path(args)?;
    let file_path = resolve_path(path, workspace)
        .map_err(|e| format!("Failed to resolve path '{path}': {e}"))?;
    let display_path = make_workspace_relative(&file_path, workspace);

    // offset is 1-based; default to 1 (first line)
    let offset = arg_u64(args, "offset").map(|v| v as usize).unwrap_or(1);
    let user_limit = arg_u64(args, "limit").map(|v| v as usize);

    if offset == 0 {
        return Err("Offset must be >= 1 (1-based numbering)".into());
    }

    // Pre-check: existence and file-vs-directory give clearer errors.
    check_readable(&file_path, &display_path)?;

    let file = std::fs::File::open(&file_path)
        .map_err(|e| format!("Failed to open {display_path}: {e}"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);

    let start = offset - 1; // 0-based lines to skip

    // ── single-pass: skip → emit → count-remaining ──────────────────

    let mut buf = String::new();
    let mut lines_skipped = 0usize;

    // Phase 1 – skip to the requested offset
    for _ in 0..start {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => {
                // EOF during skip — offset is beyond end of file
                if lines_skipped == 0 {
                    return Ok("(file is empty)".into());
                }
                return Err(format!(
                    "Offset {offset} is beyond end of file ({lines_skipped} lines total)"
                ));
            }
            Ok(_) => lines_skipped += 1,
            Err(e) => return Err(format!("Failed to read {display_path}: {e}")),
        }
    }

    let max_lines = user_limit
        .map(|n| n.max(1))
        .unwrap_or(max_lines_cap)
        .min(max_lines_cap);

    let mut out = String::with_capacity(max_bytes);
    let mut byte_count = 0usize;
    let mut lines_emitted = 0usize;
    let mut limit_kind: Option<LimitKind> = None;
    // Captured during iteration; owned because the line buffer is reused
    let mut next_line: Option<String> = None;

    // Phase 2 – emit the window
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => break, // natural EOF
            Ok(_) => {
                let line_num = offset + lines_emitted;
                let content = strip_newline(&buf);
                let fmt_len = formatted_len(line_num, content);

                if byte_count + fmt_len > max_bytes {
                    limit_kind = Some(LimitKind::Bytes);
                    next_line = Some(content.to_owned());
                    break;
                }

                let _ = writeln!(&mut out, "{:>4}|{}", line_num, content);
                byte_count += fmt_len;
                lines_emitted += 1;

                if lines_emitted >= max_lines {
                    limit_kind = Some(LimitKind::Lines);
                    break;
                }
            }
            Err(e) => return Err(format!("Failed to read {display_path}: {e}")),
        }
    }

    // Phase 3 – if truncated by line limit, count remaining lines for the hint
    let mut remaining = 0usize;
    if limit_kind == Some(LimitKind::Lines) {
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => remaining += 1,
                Err(_) => break,
            }
        }
    }

    let end_line = start + lines_emitted; // last 1-based line emitted

    // ── edge case: first requested line exceeds byte limit ───────────

    if out.is_empty() && limit_kind == Some(LimitKind::Bytes) {
        let line = next_line.as_deref().unwrap_or("");
        let approx_kb = (line.len() + 7) / 1024;
        let limit_kb = max_bytes / 1024;
        // Emit as much of the oversized line as we can, with a truncation notice.
        let notice = format!(
            "[Line {offset} truncated: ~{approx_kb}KB, exceeds {limit_kb}KB limit — showing first {limit_kb}KB]\n",
        );
        let available = max_bytes.saturating_sub(notice.len());
        let truncated = truncate_at_boundary(line, available);
        let _ = writeln!(&mut out, "{:>4}|{}", offset, truncated);
        out.push_str(&notice);
        return Ok(out);
    }

    // ── empty file / offset exactly at EOF ──────────────────────────

    if out.is_empty() {
        if lines_skipped == 0 {
            return Ok("(file is empty)".into());
        }
        // Shouldn't reach here normally, but keep as safety net
        return Ok("(no lines to show)".into());
    }

    // ── continuation hints ──────────────────────────────────────────

    if limit_kind == Some(LimitKind::Bytes) {
        let next_line_num = end_line + 1;
        let overflowing = next_line.as_deref().unwrap_or("");
        let approx_kb = (overflowing.len() + 7) / 1024;
        let _ = writeln!(
            &mut out,
            "[Line {next_line_num} is ~{approx_kb}KB, exceeds {}KB limit — skipped]",
            max_bytes / 1024,
        );
    }

    if limit_kind == Some(LimitKind::Lines) && remaining > 0 && user_limit.is_none() {
        let next_offset = end_line + 1;
        let _ = writeln!(
            &mut out,
            "[{remaining} more lines in file. Use offset={next_offset} to continue.]"
        );
    }

    Ok(out)
}

/// Truncate `s` to at most `max_bytes` bytes, landing on a valid UTF-8
/// character boundary.
fn truncate_at_boundary(s: &str, max_bytes: usize) -> &str {
    let end = max_bytes.min(s.len());
    &s[..s.floor_char_boundary(end)]
}

// ── helpers ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LimitKind {
    Lines,
    Bytes,
}

/// Verify the path exists and is a regular file before attempting to read it.
fn check_readable(path: &Path, display_path: &str) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(meta) => {
            if meta.is_dir() {
                return Err(format!("Path is a directory, not a file: {display_path}"));
            }
            Ok(())
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(format!("File not found: {display_path}"))
            } else {
                Err(format!("Cannot access {display_path}: {e}"))
            }
        }
    }
}
