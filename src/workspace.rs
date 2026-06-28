use chrono::DateTime;
use std::collections::BTreeMap;
use std::path::Path;

/// Get a Unix-style workspace path for system-prompt display.
///
/// Runs `pwd` in a bash shell with `cwd` set to `path`, giving a
/// representation natural for bash (e.g. `/c/Users/...` on Windows).
/// Falls back to `path.to_string_lossy()` if bash is unavailable.
pub fn get_unix_style_path(path: &Path) -> String {
    std::process::Command::new("bash")
        .arg("-c")
        .arg("pwd")
        .current_dir(path)
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

// ── files tree builder ───────────────────────────────────────────────

/// Max directory depth to scan (root = depth 0, its children = depth 1, …).
const MAX_DEPTH: usize = 3;
/// Max lines shown per directory, including the "… N more" marker.
const PER_DIR_LIMIT: usize = 12;
/// Max rendered lines; deeper entries are elided first.
const LINE_CAP: usize = 120;

/// A filesystem entry collected during the walk.
#[derive(Debug, Clone)]
struct FlatEntry {
    rel_path: String, // e.g. "src/main.rs" (relative to workspace root)
    is_dir: bool,
    mtime: i64, // unix seconds
    size: u64,  // bytes
}

/// Build a sorted, capped directory tree rendered as a string.
pub fn build_files_tree(workspace: &Path) -> String {
    if workspace.as_os_str().is_empty() {
        return String::new();
    }
    if !workspace.is_dir() {
        return String::new();
    }

    let entries = walk_entries(workspace);
    if entries.is_empty() {
        return ".".to_string();
    }

    // Group entries by parent directory key ("" for root children)
    let mut by_parent: BTreeMap<String, Vec<&FlatEntry>> = BTreeMap::new();
    for e in &entries {
        let parent = match e.rel_path.rfind('/') {
            Some(pos) => &e.rel_path[..pos],
            None => "",
        };
        by_parent.entry(parent.to_string()).or_default().push(e);
    }

    let mut lines: Vec<String> = Vec::with_capacity(LINE_CAP);
    lines.push(".".to_string());

    // Render depth-1 children of root
    let root_children = by_parent.get("").map(|v| v.as_slice()).unwrap_or(&[]);
    render_dir(&mut lines, "", root_children, &by_parent, 1);

    // Apply line cap: remove deepest lines first (depth ≥ 2)
    if lines.len() > LINE_CAP {
        let keep: Vec<String> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                let depth = line.chars().take_while(|c| *c == ' ').count() / 2;
                depth <= 1
            })
            .map(|(_, l)| l.clone())
            .collect();
        let removed = lines.len() - keep.len();
        lines = keep;
        if removed > 0 {
            lines.push(format!("… ({} lines elided beyond depth/cap)", removed));
        }
        lines.truncate(LINE_CAP);
    }

    lines.join("\n")
}

/// Walk `root` up to `MAX_DEPTH`, skipping entries filtered by
/// standard ignore rules (hidden files, `.gitignore`, `.ignore`, etc.).
/// Returns entries sorted by recency (newest first, then name).
fn walk_entries(root: &Path) -> Vec<FlatEntry> {
    let mut entries: Vec<FlatEntry> = Vec::new();

    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true) // hidden files, .gitignore, .ignore, etc.
        .max_depth(Some(MAX_DEPTH))
        .build();

    for result in walker {
        let Ok(dent) = result else { continue };
        // Skip the root itself (depth 0).
        if dent.depth() == 0 {
            continue;
        }

        let Some(ft) = dent.file_type() else { continue };
        let is_dir = ft.is_dir();

        let rel_path = dent
            .path()
            .strip_prefix(root)
            .unwrap_or_else(|_| dent.path())
            .to_string_lossy()
            .replace('\\', "/");

        let (mtime, size) = dent.metadata().map_or((0, 0), |m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            (mtime, m.len())
        });

        entries.push(FlatEntry {
            rel_path,
            is_dir,
            mtime,
            size,
        });
    }

    entries.sort_by(|a, b| {
        b.mtime
            .cmp(&a.mtime)
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    entries
}

/// Recursively render directory children into `lines`.
fn render_dir(
    lines: &mut Vec<String>,
    _parent_key: &str,
    children: &[&FlatEntry],
    by_parent: &BTreeMap<String, Vec<&FlatEntry>>,
    depth: usize,
) {
    let indent = "  ".repeat(depth);
    let total = children.len();
    // If we have more entries than PER_DIR_LIMIT, reserve one line for the marker.
    let show = if total > PER_DIR_LIMIT {
        PER_DIR_LIMIT.saturating_sub(1).min(total)
    } else {
        total
    };

    // Compute column widths for this directory
    let max_name = children
        .iter()
        .take(show)
        .map(|e| {
            let name = e.rel_path.rsplit('/').next().unwrap_or(&e.rel_path);
            let suffix = if e.is_dir { "/" } else { "" };
            name.len() + suffix.len()
        })
        .max()
        .unwrap_or(0);

    for (i, e) in children.iter().enumerate() {
        if i >= show {
            break;
        }
        let name = e.rel_path.rsplit('/').next().unwrap_or(&e.rel_path);
        let suffix = if e.is_dir { "/" } else { "" };
        let label = format!("{}- {}{}", indent, name, suffix);

        if e.is_dir {
            // Directories: no size column
            let mtime = format_mtime(e.mtime);
            lines.push(format!(
                "{: <width$}  {}",
                label,
                mtime,
                width = max_name + 2 + indent.len() + 2
            ));
            // Render grandchildren
            let grandchildren = by_parent
                .get(&e.rel_path)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            render_dir(lines, &e.rel_path, grandchildren, by_parent, depth + 1);
        } else {
            let size_str = format_size(e.size);
            let mtime = format_mtime(e.mtime);
            lines.push(format!(
                "{: <width$}{:>8}  {}",
                label,
                size_str,
                mtime,
                width = max_name + 2 + indent.len() + 2
            ));
        }
    }

    // "… N more" marker counts toward PER_DIR_LIMIT budget
    if total > show {
        let dropped = total - show;
        lines.push(format!("{}- … {} more", indent, dropped));
    }
}

/// Format `bytes` into a compact size string (e.g. "1.2KB").
fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{}B", bytes)
    } else {
        format!("{:.1}{}", size, UNITS[unit])
    }
}

/// Format unix seconds as `YYYY-MM-DD HH:MM` UTC.
fn format_mtime(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let dt = DateTime::from_timestamp(secs, 0).unwrap_or_default();
    dt.format("%Y-%m-%d %H:%M").to_string()
}
