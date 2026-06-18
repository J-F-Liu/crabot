use chrono::DateTime;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

// ── files tree builder ───────────────────────────────────────────────

/// Directories skipped during workspace scan.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "build",
    "dist",
    "out",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    "venv",
    ".env",
    "env",
    ".next",
    ".nuxt",
    ".output",
    "coverage",
    ".nyc_output",
    ".cache",
    ".parcel-cache",
    ".turbo",
    ".idea",
    ".vscode",
    ".vs",
    "android",
    "ios",
    ".expo",
];

/// Max directory depth to scan (root = depth 0, its children = depth 1, …).
const MAX_DEPTH: usize = 3;
/// Max entries shown per directory; excess middle entries become "… N more".
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

/// Walk `root` up to `MAX_DEPTH`, skipping hidden files and noisy dirs.
/// Returns entries sorted by recency (newest first, then name).
fn walk_entries(root: &Path) -> Vec<FlatEntry> {
    let mut entries: Vec<FlatEntry> = Vec::new();
    // stack: (fs_path, rel_path, depth)
    let mut stack: Vec<(std::path::PathBuf, String, usize)> = Vec::new();
    stack.push((root.to_path_buf(), String::new(), 0));

    while let Some((dir, rel, depth)) = stack.pop() {
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for dent in read.flatten() {
            let name = dent.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }
            let Ok(meta) = dent.metadata() else { continue };
            let is_dir = meta.is_dir();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let size = meta.len();
            let owned_name = name_str.into_owned();
            let child_rel = if rel.is_empty() {
                owned_name.clone()
            } else {
                format!("{}/{}", rel, owned_name)
            };

            entries.push(FlatEntry {
                rel_path: child_rel.clone(),
                is_dir,
                mtime,
                size,
            });

            if is_dir && !SKIP_DIRS.contains(&owned_name.as_str()) {
                stack.push((dent.path(), child_rel, depth + 1));
            }
        }
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
    let limit = PER_DIR_LIMIT.min(children.len());

    // Compute column widths for this directory
    let max_name = children
        .iter()
        .map(|e| {
            let name = e.rel_path.rsplit('/').next().unwrap_or(&e.rel_path);
            let suffix = if e.is_dir { "/" } else { "" };
            name.len() + suffix.len()
        })
        .max()
        .unwrap_or(0);

    for (i, e) in children.iter().enumerate() {
        if i >= limit {
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

    // "… N more" marker for elided middle entries
    let dropped = children.len().saturating_sub(limit);
    if dropped > 0 {
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
