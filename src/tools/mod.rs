mod bash;
pub(crate) mod edit;
mod find;
mod read;
mod search;
mod write;

use genai::chat::Tool;

use indexmap::IndexMap;
use serde_json::Value;
// ── DevTools ────────────────────────────────────────────────────────

/// The six coding-agent devtools exposed to the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DevTool {
    Read,
    Write,
    Edit,
    Find,
    Search,
    Bash,
}

impl DevTool {
    pub const ALL: &[DevTool] = &[
        Self::Read,
        Self::Write,
        Self::Edit,
        Self::Find,
        Self::Search,
        Self::Bash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Find => "find",
            Self::Search => "search",
            Self::Bash => "bash",
        }
    }

    /// Instruction text for composing the system prompt.
    pub fn instruction(self) -> &'static str {
        instruction(self)
    }

    pub fn description(self) -> &'static str {
        description(self)
    }

    /// Full tool declaration suitable for genai ChatRequest.
    pub fn tool_declaration(self) -> Tool {
        Tool::new(self.name())
            .with_description(self.description())
            .with_schema(schema(self))
    }

    /// Build the tools list for genai ChatRequest from selected tools.
    pub fn build_tools(selected: &IndexMap<DevTool, bool>) -> Vec<Tool> {
        selected
            .iter()
            .filter(|(_, enabled)| **enabled)
            .map(|(tool, _)| tool.tool_declaration())
            .collect()
    }

    /// Execute this tool with the given JSON arguments and workspace root.
    pub fn execute(self, args: &Value, workspace: &std::path::Path) -> Result<String, String> {
        match self {
            Self::Read => read::execute(args, workspace),
            Self::Write => write::execute(args, workspace),
            Self::Edit => edit::execute(args, workspace),
            Self::Find => find::execute(args, workspace),
            Self::Search => search::execute(args, workspace),
            Self::Bash => bash::execute(args, workspace),
        }
    }

    /// Parse tool call from name string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "edit" => Some(Self::Edit),
            "find" => Some(Self::Find),
            "search" => Some(Self::Search),
            "bash" => Some(Self::Bash),
            _ => None,
        }
    }
}

// ── schema dispatch ────────────────────────────────────────────────

fn schema(tool: DevTool) -> Value {
    match tool {
        DevTool::Read => read::schema(),
        DevTool::Write => write::schema(),
        DevTool::Edit => edit::schema(),
        DevTool::Find => find::schema(),
        DevTool::Search => search::schema(),
        DevTool::Bash => bash::schema(),
    }
}

fn instruction(tool: DevTool) -> &'static str {
    match tool {
        DevTool::Read => read::instruction(),
        DevTool::Write => write::instruction(),
        DevTool::Edit => edit::instruction(),
        DevTool::Find => find::instruction(),
        DevTool::Search => search::instruction(),
        DevTool::Bash => bash::instruction(),
    }
}

fn description(tool: DevTool) -> &'static str {
    match tool {
        DevTool::Read => read::description(),
        DevTool::Write => write::description(),
        DevTool::Edit => edit::description(),
        DevTool::Find => find::description(),
        DevTool::Search => search::description(),
        DevTool::Bash => bash::description(),
    }
}

// ── shared helpers ─────────────────────────────────────────────────

pub(crate) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

pub(crate) fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

/// Strip the workspace prefix and convert to Unix‑style display path.
pub(crate) fn make_workspace_relative(
    path: &std::path::Path,
    workspace: &std::path::Path,
) -> String {
    let rel = path.strip_prefix(workspace).unwrap_or(path);
    convert_path_to_unix_style(rel)
}

/// Convert a path to Unix‑style representation (reverse of `resolve_path`).
///
/// On Windows this turns `C:\Users\...` into `/c/Users/...`.
/// On Unix this is a no‑op (just ensures forward slashes).
pub fn convert_path_to_unix_style(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();

    #[cfg(windows)]
    {
        // If it already looks like a Unix‑style path, just normalise slashes.
        if s.starts_with('/') {
            return s.replace('\\', "/");
        }
        // Match a Windows absolute path like C:\...  or C:/...
        let mut comps = path.components();
        if let Some(std::path::Component::Prefix(p)) = comps.next()
            && let std::path::Prefix::Disk(d) | std::path::Prefix::VerbatimDisk(d) = p.kind()
        {
            let drive_letter = (d as char).to_ascii_lowercase();
            let rest: String = comps
                .filter(|c| {
                    !matches!(
                        c,
                        std::path::Component::RootDir | std::path::Component::CurDir
                    )
                })
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            return format!("/{drive_letter}/{rest}");
        }
    }

    // On non-Windows (or non‑absolute Windows), just normalise backslashes.
    s.replace('\\', "/")
}

/// Build the (non‑canonicalized) target path for `path` relative to `workspace`.
///
/// Handles native absolute paths, Windows Unix‑style paths such as
/// `/c/Users/...`, and workspace‑relative paths.
fn candidate_path(path: &str, workspace: &std::path::Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }

    // On Windows a path like "/c/Users/..." is Unix‑style absolute, but
    // `Path::is_absolute()` returns false without a drive prefix.
    #[cfg(windows)]
    if let Some(native) = normalise_windows_unix_style(path) {
        return native;
    }

    workspace.join(p)
}

/// On Windows, convert a Unix‑style path like `/c/Users/...` into a native
/// `C:\Users\...` `PathBuf`. Returns `None` when `path` is not Unix‑style
/// absolute (i.e. does not start with `/`).
#[cfg(windows)]
fn normalise_windows_unix_style(path: &str) -> Option<std::path::PathBuf> {
    let stripped = path.strip_prefix('/')?;
    let native = if let Some((drive, rest)) = stripped.split_once('/')
        && drive.len() == 1
        && drive.as_bytes()[0].is_ascii_alphabetic()
    {
        format!(
            "{}:\\{}",
            drive.to_ascii_uppercase(),
            rest.replace('/', "\\")
        )
    } else {
        path.replace('/', "\\")
    };
    Some(std::path::PathBuf::from(native))
}

pub(crate) fn resolve_path(
    path: &str,
    workspace: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    dunce::canonicalize(candidate_path(path, workspace))
}

/// Like [`resolve_path`] but does not require the final path to exist.
///
/// Canonicalizes the nearest existing ancestor, then appends the remaining
/// (possibly non‑existent) tail components.
pub(crate) fn resolve_path_partial(
    path: &str,
    workspace: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    let candidate = candidate_path(path, workspace);

    // Walk up from the candidate until we find an existing ancestor, then
    // re‑attach the missing tail components. The first iteration covers the
    // common case where the full path already exists.
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = candidate.as_path();
    loop {
        if let Ok(canon) = dunce::canonicalize(current) {
            let mut result = canon;
            for seg in missing.iter().rev() {
                result.push(seg);
            }
            return Ok(result);
        }
        match current.parent() {
            Some(parent) => {
                if let Some(name) = current.file_name() {
                    missing.push(name);
                }
                current = parent;
            }
            // Reached the root without finding an existing ancestor — fall
            // back to the un‑canonicalized candidate.
            None => return Ok(candidate),
        }
    }
}

// ── output truncation ──────────────────────────────────────────────

/// Maximum bytes for tool output before truncation.
const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KB

/// Number of bytes to keep from the head and tail when truncating.
const HEAD_TAIL_BYTES: usize = 3 * 1024; // 3 KB each

/// Truncate output that exceeds [`MAX_OUTPUT_BYTES`], keeping head and tail.
pub(crate) fn truncate_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }

    let total = s.len();
    let skipped = total - HEAD_TAIL_BYTES * 2;

    // Find valid UTF-8 boundaries near the split points
    let head_end = find_char_boundary(&s, HEAD_TAIL_BYTES);
    let tail_start = find_char_boundary(&s, total - HEAD_TAIL_BYTES);

    let head = &s[..head_end];
    let tail = &s[tail_start..];

    let mut truncated = String::with_capacity(HEAD_TAIL_BYTES * 2 + 128);
    truncated.push_str(head);
    let _ = std::fmt::Write::write_fmt(
        &mut truncated,
        format_args!(
            "\n\n... [{skipped} bytes truncated ({total} total, max {MAX_OUTPUT_BYTES})] ...\n\n"
        ),
    );
    truncated.push_str(tail);
    truncated
}

/// Find the closest valid UTF-8 character boundary at or before `pos`.
fn find_char_boundary(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    if s.is_char_boundary(pos) {
        pos
    } else {
        // Step back until we hit a valid boundary (at most 3 bytes for UTF-8)
        (pos.saturating_sub(3)..pos)
            .rev()
            .find(|&i| s.is_char_boundary(i))
            .unwrap_or(0)
    }
}
