mod bash;
pub(crate) mod edit;
mod find;
mod read;
mod search;
mod write;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use genai::chat::Tool as GenaiTool;
use indexmap::IndexMap;
use serde_json::Value;

// ── Tool trait ──────────────────────────────────────────────────────

pub type ToolRef = Arc<dyn Tool>;

/// Trait implemented by every tool (built-in or custom).
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn instruction(&self) -> &str;
    fn schema(&self) -> Value;
    fn execute(&self, args: &Value, workspace: &Path) -> Result<String, String>;

    /// Full tool declaration suitable for genai ChatRequest.
    fn tool_declaration(&self, strict: bool) -> GenaiTool {
        let mut schema = self.schema();
        if strict {
            make_strict_schema(&mut schema);
        }
        GenaiTool::new(self.name())
            .with_description(self.description())
            .with_schema(schema)
            .with_strict(strict)
    }
}

// ── Strict schema post-processing ──────────────────────────────────

/// Adjust the schema in-place for strict tool-calling mode:
/// every property becomes required, and optional properties get
/// `"type": ["T", "null"]` union types.
fn make_strict_schema(schema: &mut Value) {
    process_strict(schema);
}

fn process_strict(value: &mut Value) {
    let Value::Object(obj) = value else {
        // Recurse into array items
        if let Value::Array(arr) = value {
            for item in arr.iter_mut() {
                process_strict(item);
            }
        }
        return;
    };

    // If this is an object-typed schema node with properties, enforce that
    // *every* declared property appears in `required`.
    if obj.get("type").and_then(Value::as_str) == Some("object") {
        // Collect property keys and identify optional ones without holding
        // a borrow on `obj` across the mutable `insert` below.
        let (all_keys, optional_keys) =
            if let Some(properties) = obj.get("properties").and_then(|v| v.as_object()) {
                let required_set: HashSet<&str> = obj
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();

                let all: Vec<String> = properties.keys().cloned().collect();
                let optional: Vec<String> = all
                    .iter()
                    .filter(|k| !required_set.contains(k.as_str()))
                    .cloned()
                    .collect();
                (all, optional)
            } else {
                (Vec::new(), Vec::new())
            };

        if !all_keys.is_empty() {
            obj.insert(
                "required".to_string(),
                Value::Array(all_keys.iter().map(|k| Value::String(k.clone())).collect()),
            );
        }

        // Make optional properties nullable.
        if let Some(props) = obj.get_mut("properties").and_then(|v| v.as_object_mut()) {
            for key in &optional_keys {
                if let Some(prop) = props.get_mut(key) {
                    make_nullable(prop);
                }
            }
        }
    }

    // Recurse into every child value
    for (_k, v) in obj.iter_mut() {
        process_strict(v);
    }
}

/// Modify a property schema in-place so that it accepts `null`.
///
/// Handles both `"type": "T"` → `"type": ["T", "null"]` and
/// `anyOf` → appends `{"type": "null"}`.
fn make_nullable(value: &mut Value) {
    let Value::Object(obj) = value else { return };

    if let Some(type_val) = obj.get_mut("type") {
        match type_val {
            Value::String(s) if s != "null" => {
                *type_val =
                    Value::Array(vec![Value::String(s.clone()), Value::String("null".into())]);
            }
            Value::Array(arr) if !arr.iter().any(|v| v.as_str() == Some("null")) => {
                arr.push(Value::String("null".into()));
            }
            _ => {}
        }
    }

    // If the property uses `anyOf` (union type from custom tools), add a null variant.
    if let Some(any_of) = obj.get_mut("anyOf").and_then(|v| v.as_array_mut())
        && !any_of.iter().any(|v| {
            v.as_object()
                .and_then(|o| o.get("type"))
                .and_then(Value::as_str)
                == Some("null")
        })
    {
        any_of.push(serde_json::json!({"type": "null"}));
    }
}

// ── Tool registry ───────────────────────────────────────────────────

/// Global registry of all built-in tools, keyed by name in insertion order.
///
/// Stored as a `LazyLock` so tool lookups return a borrowed `&'static dyn Tool`
/// without any heap allocation.
static BUILTIN_TOOLS: LazyLock<IndexMap<&'static str, ToolRef>> = LazyLock::new(|| {
    let mut map: IndexMap<&'static str, ToolRef> = IndexMap::new();
    map.insert("read", Arc::new(read::ReadTool));
    map.insert("write", Arc::new(write::WriteTool));
    map.insert("edit", Arc::new(edit::EditTool));
    map.insert("find", Arc::new(find::FindTool));
    map.insert("search", Arc::new(search::SearchTool));
    map.insert("bash", Arc::new(bash::BashTool));
    map
});

/// Borrow the built-in tool registry.
pub fn builtin_tools() -> &'static IndexMap<&'static str, ToolRef> {
    &BUILTIN_TOOLS
}

/// Look up a tool by name.
pub fn find_tool(name: &str) -> Option<ToolRef> {
    BUILTIN_TOOLS.get(name).cloned()
}

pub fn enabled_tools(enabled: &HashSet<String>) -> Vec<ToolRef> {
    BUILTIN_TOOLS
        .iter()
        .filter_map(|(name, tool)| {
            if enabled.contains(*name) {
                Some(Arc::clone(tool))
            } else {
                None
            }
        })
        .collect()
}

/// Build the genai tools list from the enabled set.
pub fn build_tools(tools: &[ToolRef], strict: bool) -> Vec<GenaiTool> {
    tools.iter().map(|t| t.tool_declaration(strict)).collect()
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
    if let Some(native) = convert_path_to_windows_style(path) {
        return native;
    }

    workspace.join(p)
}

/// On Windows, convert a Unix‑style path like `/c/Users/...` into a native
/// `C:\Users\...` `PathBuf`. Returns `None` when `path` is not Unix‑style
/// absolute (i.e. does not start with `/`).
#[cfg(windows)]
fn convert_path_to_windows_style(path: &str) -> Option<std::path::PathBuf> {
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
