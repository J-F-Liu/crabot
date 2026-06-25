use std::fmt::Write;
use std::io::{BufRead, BufReader};

use serde_json::{Value, json};

use super::{arg_str, arg_u64, make_workspace_relative, resolve_path};

pub(super) fn instruction() -> &'static str {
    "When reading files, prefer larger, context-rich reads over multiple small consecutive reads. Large files may be truncated with a marker such as \"[213 more lines in file. Use offset=2000 to continue.]\". You can use the `read` tool to load additional content if needed. Never pass the truncation marker to an edit tool. You don't need to read a file if it's already provided in context."
}

pub(super) fn description() -> &'static str {
    "Read a file from the filesystem with line-numbered output. Supports offset and line-limit pagination."
}

const DEFAULT_MAX_LINES: usize = 2000;
const DEFAULT_MAX_BYTES: usize = 64 * 1024; // 64 KB

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

pub(super) fn schema() -> Value {
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
                "description": "Maximum number of lines to read (default: 2000, capped at 2000)"
            }
        },
        "required": ["path"]
    })
}

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let path = arg_str(args, "path").ok_or("Missing 'path' argument")?;
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
        .unwrap_or(DEFAULT_MAX_LINES)
        .min(DEFAULT_MAX_LINES);

    let mut out = String::with_capacity(DEFAULT_MAX_BYTES);
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

                if byte_count + fmt_len > DEFAULT_MAX_BYTES {
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

    let total_lines = lines_skipped + lines_emitted + remaining;
    let end_line = start + lines_emitted; // last 1-based line emitted

    // ── edge case: first requested line exceeds byte limit ───────────

    if out.is_empty() && limit_kind == Some(LimitKind::Bytes) {
        let line = next_line.as_deref().unwrap_or("");
        let approx_kb = (line.len() + 7) / 1024;
        return Ok(format!(
            "[Line {offset} is ~{approx_kb}KB, exceeds {}KB limit. Use bash: sed -n '{offset}p' {display_path}]\n",
            DEFAULT_MAX_BYTES / 1024,
        ));
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
            "[Line {next_line_num} is ~{approx_kb}KB, exceeds {}KB limit. Use bash: sed -n '{next_line_num}p' {display_path}]",
            DEFAULT_MAX_BYTES / 1024,
        );
    }

    if limit_kind == Some(LimitKind::Lines) && remaining > 0 {
        let next_offset = end_line + 1;
        if user_limit == Some(lines_emitted) {
            let _ = writeln!(
                &mut out,
                "[{remaining} more lines in file. Use offset={next_offset} to continue.]"
            );
        } else {
            let _ = writeln!(
                &mut out,
                "[Showing lines {offset}-{end_line} of {total_lines}. Use offset={next_offset} to continue.]"
            );
        }
    }

    Ok(out)
}

// ── helpers ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LimitKind {
    Lines,
    Bytes,
}

/// Verify the path exists and is a regular file before attempting to read it.
fn check_readable(path: &std::path::Path, display_path: &str) -> Result<(), String> {
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
