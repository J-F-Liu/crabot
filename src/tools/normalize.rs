//! Model-facing compaction of tool output.
//!
//! Grep-style results repeat the file path on every line — `path:12:text` for
//! matches, `path-12-text` for the context lines of `grep -A/-B/-C`. Grouping
//! runs that share a path into a `path:` heading drops the repetition while
//! keeping every line number, separator, and matched text byte for byte.
//!
//! Normalization happens once, where a tool result becomes conversation
//! content, so the model, the UI, the session file, and the HTML export all
//! agree; the live stream stays raw.

/// Group consecutive grep-style lines that share a file path.
///
/// Returns `None` when nothing was regrouped (single-line runs, non-grep
/// output); idempotent — grouped output passes through unchanged.
pub fn group_repeated_paths(text: &str) -> Option<String> {
    if !has_groupable_run(text) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut run: Vec<&str> = Vec::new();
    let mut run_path: Option<&str> = None;

    for line in text.split_inclusive('\n') {
        let path = split_match(line).map(|(path, _)| path);
        if path.is_some() && path == run_path {
            run.push(line);
            continue;
        }
        flush(&mut out, &mut run, run_path);
        run_path = path;
        match path {
            Some(_) => run.push(line),
            None => out.push_str(line),
        }
    }
    flush(&mut out, &mut run, run_path);

    Some(out)
}

/// Split `path:line:content` (a match line) or `path-line-content` (a context
/// line from `grep -A/-B/-C`) into `(path, line<sep>content)`.
///
/// Only the first `sep digits sep` candidate counts: if its prefix is not a
/// file path the line is not grep output, so `2026-01-01 10:00:00` never
/// splits as `2026` + `01`.
fn split_match(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    for (i, &sep) in bytes.iter().enumerate() {
        if sep != b':' && sep != b'-' {
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == start || bytes.get(end) != Some(&sep) {
            continue;
        }
        let path = &line[..i];
        return is_path_like(path).then_some((path, &line[start..]));
    }
    None
}

/// Whether two consecutive lines split to the same path — the condition for
/// `group_repeated_paths` to regroup.
fn has_groupable_run(text: &str) -> bool {
    let mut prev: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        match split_match(line) {
            Some((path, _)) if prev == Some(path) => return true,
            matched => prev = matched.map(|(path, _)| path),
        }
    }
    false
}

/// A usable path prefix: no surrounding whitespace and at least one separator
/// or dot — prose, tables, and bare words never qualify.
fn is_path_like(path: &str) -> bool {
    !path.is_empty() && path.trim() == path && path.contains(['.', '/', '\\'])
}

/// Emit a finished run: verbatim for a lone line, otherwise a `path:` heading
/// followed by each line's `line<sep>content` tail.
fn flush(out: &mut String, run: &mut Vec<&str>, path: Option<&str>) {
    if run.len() < 2 {
        for line in run.drain(..) {
            out.push_str(line);
        }
        return;
    }
    // Blank line before the heading, unless at the start or already blank;
    // `out` always ends with '\n' here — a group can never follow a
    // newline-less last line.
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    let path = path.expect("a multi-line run always has a path");
    out.push_str(path);
    out.push_str(":\n");
    // Every run line starts with `path` plus its one-byte separator.
    let tail = path.len() + 1;
    for line in run.drain(..) {
        out.push_str(line.get(tail..).unwrap_or(line));
    }
}
