mod ask;
mod bash;
/// In-process bashkit interpreter; `pub` for `tests/bash.rs`.
pub mod bash_kit;
pub mod custom;
pub mod edit;
pub mod fetch;
pub mod find;
pub mod mcp;
pub mod process;
mod read;
mod renew;
mod search;
mod task;
pub mod todo;
mod write;

pub use renew::move_renews_to_end;
pub use task::{TASK_MODES, TaskRequest, task_request_from_call};

use crate::BoundedCapture;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use genai::chat::Tool as GenaiTool;
use interprocess::unnamed_pipe;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Constants ────────────────────────────────────────────────────────

/// Coalesce small chunks until this many bytes accumulate before flushing.
const COALESCE_BYTES: usize = 4 * 1024;
/// Coalesce small chunks for at most this long before flushing.
pub const COALESCE_MS: Duration = Duration::from_millis(100);

/// User-cancel reason shared by tools and the LLM loop.
pub const CANCEL_REASON: &str = "Cancelled by user";

/// How long timeout/cancel errors wait for a detached host command's final
/// drain (the forwarder lock) before reporting without partial output.
pub(crate) const CAPTURE_GRACE: Duration = Duration::from_secs(2);

// ── Tool trait ──────────────────────────────────────────────────────

pub type ToolRef = Arc<dyn Tool>;

/// Sink for incremental tool output (e.g. bash stdout/stderr chunks).
pub type OutputSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Trait implemented by every tool (built-in or custom).
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn instruction(&self) -> &str;
    fn schema(&self) -> Value;

    /// Cancel-aware wrapper: checks the token *before* delegating to
    /// [`execute_inner`](Self::execute_inner); tools may also poll it while running.
    fn execute(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        self.execute_inner(args, workspace, cancel)
    }

    /// Implement this instead of [`execute`](Self::execute) — the default
    /// `execute` wrapper already handles the pre-execution cancel check.
    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String>;

    /// Streaming variant of [`execute`](Self::execute): the same cancel-aware
    /// wrapper, but live-output tools forward incremental chunks to `sink`.
    /// The default ignores `sink` and behaves like `execute`. Chunks are raw
    /// text (UTF-8, newline normalized); the final result stays authoritative.
    fn execute_streaming(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
        sink: &OutputSink,
    ) -> Result<String, String> {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        self.execute_streaming_inner(args, workspace, cancel, sink)
    }

    /// Implement this instead of [`execute_streaming`](Self::execute_streaming)
    /// to stream output live; the default ignores `sink` and falls back to
    /// [`execute_inner`](Self::execute_inner).
    fn execute_streaming_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
        _sink: &OutputSink,
    ) -> Result<String, String> {
        self.execute_inner(args, workspace, cancel)
    }

    /// Full tool declaration suitable for genai ChatRequest.
    fn tool_declaration(&self, strict: bool) -> GenaiTool {
        let mut schema = self.schema();
        if strict {
            make_strict_schema(&mut schema);
        }
        let tool = GenaiTool::new(self.name())
            .with_description(self.description())
            .with_schema(schema);
        if strict { tool.with_strict(true) } else { tool }
    }
}

// ── Strict schema post-processing ──────────────────────────────────

/// Adjust the schema in-place for strict tool-calling mode:
/// every property becomes required, and optional properties get `"type": ["T", "null"]` union types.
/// "additionalProperties: false" is automatically added by `genai`.
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

        // Change to string type to accept arbitrary key-value data in strict mode.
        if !obj.contains_key("properties")
            && let Some(type_val) = obj.get_mut("type")
        {
            *type_val = Value::String("string".into());
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
            Value::String(s)
                if ["string", "number", "integer", "boolean"].contains(&s.as_str()) =>
            {
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

/// Owned registry of all tools (built-in, custom, and MCP-discovered).
pub struct ToolRegistry {
    pub builtin: Vec<ToolRef>,
    pub custom: Vec<custom::CustomTool>,
    /// MCP tools grouped by server name: `(server_name, tools)`.
    pub mcp: Vec<(String, Vec<mcp::McpTool>)>,
    pub builtin_names: Vec<String>,
    pub custom_names: Vec<String>,
    pub mcp_servers: Vec<mcp::McpServer>,
    /// MCP tool names grouped by server name: `(server_name, tool_names)`.
    pub mcp_groups: Vec<(String, Vec<String>)>,
    /// Shared todo list — written by the `todo` tool, read by the right pane.
    pub todo_items: todo::TodoList,
}

impl ToolRegistry {
    /// Create a new registry pre-populated with the twelve built-in tools.
    pub fn new() -> Self {
        let todo_items: todo::TodoList = todo::create_todo_list(Vec::new());
        let builtin: Vec<ToolRef> = vec![
            Arc::new(read::ReadTool),
            Arc::new(write::WriteTool),
            Arc::new(edit::EditTool),
            Arc::new(find::FindTool),
            Arc::new(search::SearchTool),
            Arc::new(bash::BashTool),
            Arc::new(process::ProcessTool),
            Arc::new(ask::AskTool),
            Arc::new(todo::TodoTool::new(Arc::clone(&todo_items))),
            Arc::new(task::TaskTool),
            Arc::new(renew::RenewTool),
            Arc::new(fetch::FetchTool),
        ];
        Self {
            builtin_names: builtin.iter().map(|t| t.name().to_string()).collect(),
            builtin,
            custom: Vec::new(),
            mcp: Vec::new(),
            custom_names: Vec::new(),
            mcp_servers: Vec::new(),
            mcp_groups: Vec::new(),
            todo_items,
        }
    }

    /// Replace the custom tools in the registry.
    pub fn register_custom(&mut self, tool_list: custom::ToolList) {
        self.custom_names = tool_list
            .custom_tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        self.custom = tool_list.custom_tools;
    }

    /// Add one MCP server's tools to the registry (incremental).
    /// If a group with the same server name already exists, it is replaced.
    pub fn register_mcp_group(&mut self, server_name: String, tools: Vec<mcp::McpTool>) {
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

        // Replace existing group with the same server name, or append.
        if let Some(pos) = self.mcp_groups.iter().position(|(n, _)| n == &server_name) {
            self.mcp_groups[pos] = (server_name.clone(), names);
            self.mcp[pos] = (server_name, tools);
        } else {
            self.mcp_groups.push((server_name.clone(), names));
            self.mcp.push((server_name, tools));
        }
    }

    /// Remove a server's tools from the registry, returning the tool names
    /// that were removed. Used when a server is deleted or reconfigured.
    pub fn unregister_mcp_group(&mut self, server_name: &str) -> Vec<String> {
        if let Some(pos) = self.mcp_groups.iter().position(|(n, _)| n == server_name) {
            let (_, names) = self.mcp_groups.remove(pos);
            self.mcp.remove(pos);
            names
        } else {
            Vec::new()
        }
    }

    /// Look up an MCP server config by name.
    pub fn find_mcp_server(&self, server: &str) -> Option<mcp::McpServer> {
        self.mcp_servers.iter().find(|s| s.name == server).cloned()
    }

    /// Return a clone of all custom tool names.
    pub fn custom_names(&self) -> Vec<String> {
        self.custom_names.to_vec()
    }

    /// Return names of all registered tools (built-in + custom + MCP).
    pub fn all_names(&self) -> impl Iterator<Item = &String> {
        self.builtin_names
            .iter()
            .chain(self.custom_names.iter())
            .chain(self.mcp_groups.iter().flat_map(|(_s, names)| names.iter()))
    }

    /// Return a snapshot of the current todo list.
    pub fn snapshot_todo(&self) -> Vec<todo::TodoItem> {
        self.todo_items
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default()
    }

    /// Clear all todo items.
    pub fn clear_todo(&self) {
        if let Ok(mut items) = self.todo_items.lock() {
            items.clear();
        }
    }

    /// Get the list of MCP tool names for a specific server.
    pub fn get_mcp_tool_names(&self, server: &str) -> &[String] {
        self.mcp_groups
            .iter()
            .find(|(s, _)| s == server)
            .map(|(_, tools)| tools.as_slice())
            .unwrap_or_default()
    }

    /// Collect every tool whose name appears in `enabled`.
    /// MCP tools are further filtered by `enabled_servers` (server name must be present).
    pub fn enabled_tools(
        &self,
        enabled: &HashSet<String>,
        enabled_servers: &HashSet<String>,
    ) -> Vec<ToolRef> {
        let mut tools: Vec<ToolRef> = Vec::new();
        for tool in self.builtin.iter() {
            if enabled.contains(tool.name()) {
                tools.push(Arc::clone(tool));
            }
        }
        for t in &self.custom {
            if enabled.contains(&t.name) {
                tools.push(Arc::new(t.clone()));
            }
        }
        for (server, group) in &self.mcp {
            if enabled_servers.contains(server) {
                for t in group {
                    if enabled.contains(&t.name) {
                        tools.push(Arc::new(t.clone()));
                    }
                }
            }
        }
        tools
    }

    /// Look up a tool by name across builtin, custom, and MCP groups.
    /// Returns a reference-counted tool for execution.
    pub fn find_tool(&self, name: &str) -> Option<ToolRef> {
        // Search builtin tools.
        for tool in self.builtin.iter() {
            if tool.name() == name {
                return Some(Arc::clone(tool));
            }
        }
        // Search custom tools.
        for tool in &self.custom {
            if tool.name() == name {
                return Some(Arc::new(tool.clone()));
            }
        }
        // Search MCP tools.
        for (_server, tools) in &self.mcp {
            for tool in tools {
                if tool.name() == name {
                    return Some(Arc::new(tool.clone()));
                }
            }
        }
        None
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the genai tools list from a set of tool refs.
pub fn build_tools(tools: &[ToolRef], strict: bool) -> Vec<GenaiTool> {
    tools.iter().map(|t| t.tool_declaration(strict)).collect()
}

/// Build a helpful error message when an unknown tool is requested.
pub fn unknown_tool_message(name: &str) -> String {
    let hint = match name {
        "grep" => Some("use the search tool instead"),
        "cat" => Some("use the read tool instead"),
        "ls" | "dir" => Some("use the find or bash tool instead"),
        "mv" | "cp" | "rm" | "mkdir" => Some("use the bash tool instead"),
        "curl" | "wget" => Some("use the fetch tool instead"),
        "git" => Some("use the bash tool instead"),
        _ => None,
    };

    match hint {
        Some(suggestion) => format!("Unknown tool: {name} — {suggestion}"),
        None => format!("Unknown tool: {name}"),
    }
}

// ── shared helpers ─────────────────────────────────────────────────

/// Convert Windows-style `\r\n` line endings to Unix `\n`.
pub fn normalize_newlines(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\r') {
        return std::borrow::Cow::Borrowed(s);
    }
    std::borrow::Cow::Owned(s.replace("\r\n", "\n"))
}

pub(crate) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// Like `arg_str` but accepts common aliases for a path parameter.
pub fn arg_path(args: &Value) -> Option<&str> {
    const KEYS: &[&str] = &[
        "path",
        "file",
        "filename",
        "file_path",
        "filepath",
        "filePath",
    ];
    KEYS.iter().find_map(|k| arg_str(args, k))
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
pub(crate) fn convert_path_to_windows_style(path: &str) -> Option<std::path::PathBuf> {
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

pub fn resolve_path(
    path: &str,
    workspace: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    dunce::canonicalize(candidate_path(path, workspace))
}

/// Like [`resolve_path`] but does not require the final path to exist.
///
/// Canonicalizes the nearest existing ancestor, then appends the remaining
/// (possibly non‑existent) tail components.
pub fn resolve_path_partial(
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

/// Configurable limits for the built-in tools.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolLimits {
    /// `bash`: default timeout (ms) when no explicit `timeout` argument is given.
    pub command_timeout_ms: u64,
    /// `bash`: hard maximum timeout (ms) for a single command.
    pub max_command_timeout_ms: u64,
    /// Bytes kept from head and tail when truncating oversized tool output.
    pub head_tail_bytes: usize,
    /// Maximum output bytes for tool results before truncation.
    pub max_output_bytes: usize,
    /// `read`: default and maximum lines per call.
    pub read_max_lines: usize,
    /// `read`: byte budget per call.
    pub read_max_bytes: usize,
    /// `find`: maximum result lines.
    pub find_max_lines: usize,
    /// `search`: maximum result lines.
    pub search_max_lines: usize,
    /// `fetch`: maximum downloaded body bytes.
    pub fetch_max_body_bytes: usize,
    /// `fetch`: HTTP request timeout (ms).
    pub fetch_timeout_ms: u64,
    /// `mcp`: connection (handshake) timeout (ms).
    pub mcp_connect_timeout_ms: u64,
    /// `mcp`: single tool-call timeout (ms).
    pub mcp_call_timeout_ms: u64,
}

impl ToolLimits {
    /// Built-in default limits.
    pub const fn new() -> Self {
        Self {
            command_timeout_ms: 120_000,     // 2 minutes
            max_command_timeout_ms: 600_000, // 10 minutes
            head_tail_bytes: 3 * 1024,       // 3 KB each
            max_output_bytes: 100 * 1024,    // 100 KB
            read_max_lines: 2000,
            read_max_bytes: 64 * 1024, // 64 KB
            find_max_lines: 100,
            search_max_lines: 500,
            fetch_max_body_bytes: 8 * 1024 * 1024, // 8 MB
            fetch_timeout_ms: 60_000,              // 1 minute
            mcp_connect_timeout_ms: 60_000,        // 1 minute
            mcp_call_timeout_ms: 300_000,          // 5 minutes
        }
    }

    /// Keep both timeout fields in the valid `1000..=max` range; the bash
    /// tool's `clamp` panics and its schema breaks when `max < 1000`.
    pub fn sanitize(&mut self) {
        self.max_command_timeout_ms = self.max_command_timeout_ms.max(1000);
        self.command_timeout_ms = self
            .command_timeout_ms
            .clamp(1000, self.max_command_timeout_ms);
    }
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide tool limits, applied from settings at startup and replaceable
/// at runtime (e.g. from the settings dialog).  `RwLock::read()` is
/// near-zero-cost on the uncontended fast-path, so the hot read path in tool
/// executions stays cheap while writes remain rare.
static TOOL_LIMITS: RwLock<ToolLimits> = RwLock::new(ToolLimits::new());

/// Apply tool limits from settings; invalid values are sanitized first.
pub fn init_tool_limits(mut limits: ToolLimits) {
    limits.sanitize();
    if let Ok(mut guard) = TOOL_LIMITS.write() {
        *guard = limits;
    }
}

/// The current effective tool limits (defaults until first `init` call).
pub fn tool_limits() -> ToolLimits {
    TOOL_LIMITS.read().map(|g| *g).unwrap_or_default()
}

/// Truncate output that exceeds the configured maximum, keeping head and tail.
pub(crate) fn truncate_output(s: String) -> String {
    let limits = tool_limits();
    let max = limits.max_output_bytes.max(1);
    if s.len() <= max {
        return s;
    }

    let total = s.len();
    // Shrink head/tail proportionally so tiny caps never underflow or overlap.
    let head_tail = limits.head_tail_bytes.min(max / 2);
    let skipped = total - head_tail * 2;

    // Find valid UTF-8 boundaries near the split points
    let head_end = find_char_boundary(&s, head_tail);
    let tail_start = find_char_boundary(&s, total - head_tail);

    let head = &s[..head_end];
    let tail = &s[tail_start..];

    let mut truncated = String::with_capacity(head_tail * 2 + 128);
    truncated.push_str(head);
    let _ = std::fmt::Write::write_fmt(
        &mut truncated,
        format_args!("\n\n... [{skipped} bytes truncated ({total} total, max {max})] ...\n\n"),
    );
    truncated.push_str(tail);
    truncated
}

/// Map an `ExitStatus` to a bash-style exit code (`128 + signal` for signal
/// death). Unix reports signal death via `signal()` (no `code()`); MSYS/Cygwin
/// encodes it as `signal << 8` (SIGTERM → 3840, which native exit codes never
/// collide with). Both are normalized here so the bashkit and real-`bash`
/// routes report identically. Falls back to -1 when neither is available.
pub(crate) fn exit_code_of(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        // MSYS/Cygwin signal death on Windows (`code()` ≤ 255 on Unix).
        let sig = code >> 8;
        if (1..=64).contains(&sig) && sig << 8 == code {
            return 128 + sig;
        }
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    -1
}

/// Combine stdout, stderr, and exit code into one string, then truncate.
pub(crate) fn format_command_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    truncate_output(combine_output(
        &stdout,
        &stderr,
        exit_code_of(&output.status),
    ))
}

/// Combine stdout, `STDERR:`-prefixed stderr, and a non-zero exit code into
/// one untruncated string. Shared by the real-bash and bashkit paths.
pub(crate) fn combine_output(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("STDERR:\n");
        result.push_str(stderr);
    }
    if exit_code != 0 {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("Exit code: {exit_code}"));
    }
    result
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

// ── Process execution helpers ──────────────────────────────────────

/// Create an unnamed pipe pair for capturing child process output.
///
/// `label` is used in the error message (e.g. `"stdout"`, `"stderr"`).
fn create_pipe_pair(label: &str) -> Result<(unnamed_pipe::Sender, unnamed_pipe::Recver), String> {
    unnamed_pipe::pipe().map_err(|e| format!("Failed to create {label} pipe: {e}"))
}

/// A signal to deliver to a process tree (see [`signal_process_tree`]).
#[derive(Clone, Copy)]
pub(crate) enum ProcessSignal {
    Terminate,
    Kill,
    Interrupt,
}

impl ProcessSignal {
    /// Schema name used by the `process` tool's `stop` action.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Terminate => "terminate",
            Self::Kill => "kill",
            Self::Interrupt => "interrupt",
        }
    }
}

/// Send `signal` to a process and its whole descendant tree.
///
/// Unix: the child must be started with `process_group(0)`; the `kill`
/// syscall is used directly because some `kill` binaries (e.g. util-linux ≥
/// 2.42) misparse `-<pid>` and never deliver. Windows: `interrupt` has no
/// portable Ctrl+C equivalent and degrades to a graceful terminate; `kill`
/// also passes `/F`.
pub(crate) fn signal_process_tree(pid: u32, signal: ProcessSignal) {
    #[cfg(unix)]
    {
        let sig = match signal {
            ProcessSignal::Terminate => libc::SIGTERM,
            ProcessSignal::Kill => libc::SIGKILL,
            ProcessSignal::Interrupt => libc::SIGINT,
        };
        // SAFETY: sending a signal to the child's own process group is
        // exactly this helper's purpose.
        unsafe {
            libc::kill(-(pid as i32), sig);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut args = vec!["/T".to_string()];
        if matches!(signal, ProcessSignal::Kill) {
            args.push("/F".to_string());
        }
        args.extend(["/PID".to_string(), pid.to_string()]);
        let _ = std::process::Command::new("taskkill")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
}

/// Forcibly kill a process and its entire descendant tree.
pub(crate) fn kill_process_tree(pid: u32) {
    signal_process_tree(pid, ProcessSignal::Kill);
}

/// Start the child as a process-group leader (Unix) so its whole tree can be
/// killed on timeout, and suppress the console window (Windows).
pub(crate) fn detach_child(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
}

/// Whether an env var name is a secret that must not reach bash execution
/// (names ending in `API_KEY`, e.g. `OPENAI_API_KEY`).
pub(crate) fn is_secret_env_key(key: &str) -> bool {
    key.ends_with("API_KEY")
}

/// Strip secrets from a child command's inherited env: every variable whose
/// name ends in `API_KEY`, plus rustup's recursion counter which aborts
/// proxies past their max.
pub(crate) fn sanitize_child_env(cmd: &mut std::process::Command) {
    cmd.env_remove("RUST_RECURSION_COUNT");
    for key in std::env::vars().map(|(k, _)| k) {
        if is_secret_env_key(&key) {
            cmd.env_remove(&key);
        }
    }
}

/// Convert an unnamed pipe end (`Sender` or `Recver`) to `std::process::Stdio`.
#[cfg(unix)]
pub(crate) fn pipe_to_stdio<E: Into<std::os::unix::io::OwnedFd>>(end: E) -> std::process::Stdio {
    std::process::Stdio::from(end.into())
}

/// Convert an unnamed pipe end (`Sender` or `Recver`) to `std::process::Stdio`.
#[cfg(windows)]
pub(crate) fn pipe_to_stdio<E: Into<std::os::windows::io::OwnedHandle>>(
    end: E,
) -> std::process::Stdio {
    std::process::Stdio::from(end.into())
}

/// Set a pipe receiver to non-blocking mode.
///
/// A blocking pipe would deadlock the polling loop, so failure is fatal.
fn set_recver_nonblocking(recver: &unnamed_pipe::Recver) -> Result<(), String> {
    #[cfg(unix)]
    {
        use interprocess::os::unix::unnamed_pipe::UnnamedPipeExt;
        recver
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set non-blocking mode: {e}"))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // PIPE_NOWAIT is deprecated but still works for anonymous pipes; the
        // alternative (overlapped I/O) would be a much larger refactor.
        let handle = recver.as_raw_handle() as isize;
        let mut mode = win32::PIPE_NOWAIT;
        let ok = unsafe {
            win32::SetNamedPipeHandleState(
                handle,
                &mut mode,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "Failed to set pipe non-blocking mode: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

/// Set a pipe sender to non-blocking mode (mirror of `set_recver_nonblocking`).
///
/// No-op on Windows: `PIPE_NOWAIT` on a write end is undefined behavior. A blocking
/// `WriteFile` fails with `ERROR_NO_DATA` once the read end closes, so the feeder
/// thread cannot hang.
pub(crate) fn set_sender_nonblocking(sender: &unnamed_pipe::Sender) -> Result<(), String> {
    #[cfg(unix)]
    {
        use interprocess::os::unix::unnamed_pipe::UnnamedPipeExt;
        sender
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set non-blocking mode: {e}"))
    }
    #[cfg(windows)]
    {
        let _ = sender;
        Ok(())
    }
}

/// Prevent a pipe sender's handle from being inherited by spawned children.
///
/// On both platforms a child holding the write end of its own stdin pipe would
/// never see EOF, so we explicitly mark the sender end non-inheritable.
pub(crate) fn set_sender_noninheritable(sender: &unnamed_pipe::Sender) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = sender.as_raw_fd();
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(format!(
                "Failed to set FD_CLOEXEC: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let ok = unsafe {
            win32::SetHandleInformation(
                sender.as_raw_handle() as isize,
                win32::HANDLE_FLAG_INHERIT,
                0,
            )
        };
        if ok == 0 {
            return Err(format!(
                "Failed to clear handle inheritance: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

// ── Incremental output forwarding ──────────────────────────────────

/// Lock a mutex, recovering the payload if the holder panicked (poisoned).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Timeout error message used by both bash routes.
pub(crate) fn timeout_message(timeout: Duration) -> String {
    format!("Command timed out after {}ms", timeout.as_millis())
}

/// Lock `m`, polling for up to `budget`; `None` if it stays held.
pub(crate) fn try_lock_for<T>(m: &Mutex<T>, budget: Duration) -> Option<MutexGuard<'_, T>> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        match m.try_lock() {
            Ok(guard) => return Some(guard),
            // Poisoned still holds the payload — recover it like `lock`.
            Err(std::sync::TryLockError::Poisoned(p)) => return Some(p.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Incremental byte→text decoder shared by the bash and process tools: carries
/// incomplete UTF-8 across feeds and normalizes `\r\n` → `\n` (a trailing `\r`
/// is held back for the next chunk).
pub(crate) struct StreamDecoder {
    carry: Vec<u8>,
    pending_cr: bool,
}

impl StreamDecoder {
    pub(crate) fn new() -> Self {
        Self {
            carry: Vec::new(),
            pending_cr: false,
        }
    }

    /// Decode `bytes`, appending normalized text chunks to `out`.
    pub(crate) fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        if bytes.is_empty() {
            return;
        }
        let mut full = std::mem::take(&mut self.carry);
        full.extend_from_slice(bytes);
        match std::str::from_utf8(&full) {
            Ok(text) => self.push_normalized(text, out),
            Err(e) => {
                let valid = e.valid_up_to();
                if valid > 0 {
                    // SAFETY: valid_up_to() is a valid UTF-8 boundary.
                    self.push_normalized(
                        unsafe { std::str::from_utf8_unchecked(&full[..valid]) },
                        out,
                    );
                }
                if e.error_len().is_none() {
                    // Incomplete trailing sequence — carry to the next feed.
                    self.carry = full[valid..].to_vec();
                } else {
                    // Truly invalid bytes — lossy-replace like from_utf8_lossy.
                    let text = String::from_utf8_lossy(&full[valid..]).into_owned();
                    self.push_normalized(&text, out);
                }
            }
        }
    }

    /// Flush a carried incomplete sequence and a pending trailing `\r`.
    pub(crate) fn flush(&mut self, out: &mut Vec<String>) {
        if !self.carry.is_empty() {
            let text = String::from_utf8_lossy(&self.carry).into_owned();
            self.carry.clear();
            self.push_normalized(&text, out);
        }
        if self.pending_cr {
            self.pending_cr = false;
            out.push("\r".into());
        }
    }

    /// Normalize `\r\n` → `\n` (carrying a trailing `\r`), then emit the chunk.
    fn push_normalized(&mut self, text: &str, out: &mut Vec<String>) {
        let mut normalized = String::with_capacity(text.len() + 1);
        let mut chars = text.chars();
        if self.pending_cr {
            self.pending_cr = false;
            match chars.next() {
                Some('\n') => normalized.push('\n'), // `\r\n` split across chunks
                Some(c) => {
                    normalized.push('\r');
                    normalized.push(c);
                }
                None => {
                    self.pending_cr = true;
                    return;
                }
            }
        }
        let mut iter = chars.peekable();
        while let Some(c) = iter.next() {
            if c == '\r' {
                match iter.peek() {
                    Some('\n') => {
                        normalized.push('\n');
                        iter.next();
                    }
                    Some(_) => normalized.push('\r'),
                    None => self.pending_cr = true,
                }
            } else {
                normalized.push(c);
            }
        }
        if !normalized.is_empty() {
            out.push(normalized);
        }
    }
}

/// Which output stream a chunk came from, for per-stream capture.
#[derive(Clone, Copy)]
pub enum OutStream {
    Stdout,
    Stderr,
}

/// Forwards pipe bytes to an [`OutputSink`] as text chunks: stdout/stderr
/// merged in arrival order, `\r\n` → `\n`, small chunks coalesced, past-cap
/// bytes dropped. Per-stream windows are also captured for partial-output
/// errors, even without a sink.
pub struct ChunkForwarder {
    /// Live streaming sink; `None` when only the partial capture is needed.
    sink: Option<OutputSink>,
    /// Cap on forwarded bytes; the sink is silently muted past this.
    cap: usize,
    /// Total bytes forwarded so far.
    forwarded: usize,
    decoder: StreamDecoder,
    /// Coalesced text awaiting flush.
    pending: String,
    last_flush: std::time::Instant,
    /// Captured stdout/stderr for partial-output messages.
    stdout_cap: BoundedCapture,
    stderr_cap: BoundedCapture,
}

impl ChunkForwarder {
    pub fn new(sink: Option<OutputSink>) -> Self {
        let cap = tool_limits().max_output_bytes.max(1);
        let keep = per_stream_keep();
        Self {
            sink,
            cap,
            forwarded: 0,
            decoder: StreamDecoder::new(),
            pending: String::new(),
            last_flush: std::time::Instant::now(),
            stdout_cap: BoundedCapture::new(keep),
            stderr_cap: BoundedCapture::new(keep),
        }
    }

    /// Capture `bytes` on `stream`, then coalesce them for the sink.
    pub fn push(&mut self, stream: OutStream, bytes: &[u8]) {
        match stream {
            OutStream::Stdout => self.stdout_cap.push(bytes),
            OutStream::Stderr => self.stderr_cap.push(bytes),
        }
        let mut texts = Vec::new();
        self.decoder.feed(bytes, &mut texts);
        for text in texts {
            self.pending.push_str(&text);
        }
        self.tick();
    }

    /// Flush carried/coalesced bytes at the end of the stream.
    pub fn finish(&mut self) {
        let mut texts = Vec::new();
        self.decoder.flush(&mut texts);
        for text in texts {
            self.pending.push_str(&text);
        }
        self.flush();
    }

    /// Flush the coalescing buffer once it is large or old enough. Called
    /// after each push and periodically from idle loops, so a quiet stretch
    /// after some output still streams promptly.
    pub fn tick(&mut self) {
        if self.pending.len() >= COALESCE_BYTES
            || (!self.pending.is_empty() && self.last_flush.elapsed() >= COALESCE_MS)
        {
            self.flush();
        }
    }

    /// Append the captured stdout/stderr to `msg`, like [`kill_and_error`].
    pub fn append_partial_output(&self, msg: &mut String) {
        append_partial_output(msg, &self.stdout_cap, &self.stderr_cap);
    }

    fn flush(&mut self) {
        let Some(sink) = self.sink.as_ref() else {
            self.pending.clear(); // no sink — nothing to forward
            return;
        };
        if self.pending.is_empty() || self.forwarded >= self.cap {
            self.pending.clear();
            return;
        }
        let s = std::mem::take(&mut self.pending);
        let room = self.cap - self.forwarded;
        let take = if s.len() > room {
            &s[..find_char_boundary(&s, room)] // drop the overflow (final result has it)
        } else {
            &s
        };
        self.forwarded += take.len();
        (sink)(take);
        self.last_flush = std::time::Instant::now();
    }
}

// ── Bounded output capture ─────────────────────────────────────────

/// Failure from [`wait_with_timeout`], classified so callers can map to bash
/// exit-code conventions (124 = timeout, 130 = cancel) without mislabeling
/// pipe or `wait()` I/O failures as timeouts.
pub(crate) enum WaitError {
    /// The child outlived the deadline; it was killed and reaped.
    Timeout(String),
    /// The caller's cancel token was cancelled; the child was killed and reaped.
    Cancelled(String),
    /// Pipe setup or `wait()` failure — neither a timeout nor a cancel.
    Other(String),
}

impl WaitError {
    /// The message to surface to the caller (includes any partial output).
    pub(crate) fn into_message(self) -> String {
        match self {
            WaitError::Timeout(m) | WaitError::Cancelled(m) | WaitError::Other(m) => m,
        }
    }

    /// bash exit-code convention: 124 = timeout, 130 = cancel (SIGINT), 1 = other.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            WaitError::Timeout(_) => 124,
            WaitError::Cancelled(_) => 130,
            WaitError::Other(_) => 1,
        }
    }
}

/// Wait for a child process to finish, with a hard timeout.
///
/// Pipes are drained in non-blocking polling mode (no reader threads), so a
/// surviving grandchild holding a pipe open cannot leak a thread. Output is
/// captured with a bounded head/tail window (`tool_limits().max_output_bytes`
/// total, split evenly), so runaway output (e.g. `yes`) cannot exhaust memory.
///
/// On timeout the process — and its whole tree if `kill_tree` — is killed and
/// reaped without blocking on pipe EOF. `kill_tree` must be `true` only when
/// the child was started as a process-group leader.
///
/// `remaining` is the budget actually waited; `timeout_total` is reported in
/// the timeout message (they differ when the caller already spent budget).
/// `forwarder` also gets each drained chunk for live streaming and partial
/// capture.
#[allow(clippy::too_many_arguments)] // 8 params; a context struct would churn 3 call sites
pub(crate) fn wait_with_timeout(
    mut child: std::process::Child,
    mut stdout: Option<unnamed_pipe::Recver>,
    mut stderr: Option<unnamed_pipe::Recver>,
    remaining: Duration,
    timeout_total: Duration,
    kill_tree: bool,
    cancel: &CancellationToken,
    mut forwarder: Option<&mut ChunkForwarder>,
) -> Result<std::process::Output, WaitError> {
    let pid = child.id();

    let keep = per_stream_keep();
    let mut stdout_cap = BoundedCapture::new(keep);
    let mut stderr_cap = BoundedCapture::new(keep);

    // Non-blocking pipes let the polling loop drain them without reader threads.
    // A setup failure here must not leak the already-spawned child.
    for pipe in [&stdout, &stderr].into_iter().flatten() {
        if let Err(e) = set_recver_nonblocking(pipe) {
            return Err(WaitError::Other(kill_and_error(
                &mut child,
                pid,
                kill_tree,
                &stdout_cap,
                &stderr_cap,
                &format!("Failed to set pipe non-blocking mode: {e}"),
            )));
        }
    }

    let deadline = Instant::now() + remaining;
    let mut tmp = [0u8; 8192];

    // Drain pipes while polling, or the child blocks on a full pipe buffer.
    let status = loop {
        drain_pipe(
            stdout.as_mut(),
            &mut stdout_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stdout,
        );
        drain_pipe(
            stderr.as_mut(),
            &mut stderr_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stderr,
        );

        // Time-based flush so coalesced bytes stream while the child is quiet.
        if let Some(f) = forwarder.as_deref_mut() {
            f.tick();
        }

        // Check for user cancellation before trying the child.
        if cancel.is_cancelled() {
            return Err(WaitError::Cancelled(kill_and_error(
                &mut child,
                pid,
                kill_tree,
                &stdout_cap,
                &stderr_cap,
                CANCEL_REASON,
            )));
        }

        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Err(WaitError::Timeout(kill_and_error(
                        &mut child,
                        pid,
                        kill_tree,
                        &stdout_cap,
                        &stderr_cap,
                        &timeout_message(timeout_total),
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(WaitError::Other(kill_and_error(
                    &mut child,
                    pid,
                    kill_tree,
                    &stdout_cap,
                    &stderr_cap,
                    &format!("Failed to wait on command: {e}"),
                )));
            }
        }
    };

    // The process exited, but a grandchild may still hold the write-ends
    // open; drain for up to 2s and return whatever was collected.
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let stdout_done = drain_pipe(
            stdout.as_mut(),
            &mut stdout_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stdout,
        );
        let stderr_done = drain_pipe(
            stderr.as_mut(),
            &mut stderr_cap,
            &mut tmp,
            forwarder.as_deref_mut(),
            OutStream::Stderr,
        );
        if (stdout_done && stderr_done) || Instant::now() >= drain_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(std::process::Output {
        status,
        stdout: stdout_cap.into_bytes(),
        stderr: stderr_cap.into_bytes(),
    })
}

/// Drain all currently-available bytes from `reader` into `cap`; `true` on
/// EOF (or no reader), `false` on `WouldBlock`. Chunks are also pushed to
/// `forwarder` for live streaming and partial capture.
fn drain_pipe(
    reader: Option<&mut unnamed_pipe::Recver>,
    cap: &mut BoundedCapture,
    tmp: &mut [u8],
    mut forwarder: Option<&mut ChunkForwarder>,
    stream: OutStream,
) -> bool {
    let Some(reader) = reader else {
        return true;
    };
    loop {
        match reader.read(tmp) {
            Ok(0) => return true, // EOF — write end closed
            Ok(n) => {
                cap.push(&tmp[..n]);
                if let Some(f) = forwarder.as_mut() {
                    f.push(stream, &tmp[..n]);
                }
            }
            Err(ref e) if is_would_block(e) => return false,
            Err(_) => return true, // treat unexpected errors as EOF
        }
    }
}

/// True if the error means "no data available right now" in non-blocking mode.
pub(crate) fn is_would_block(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    // Windows `PIPE_NOWAIT` pipes report `ERROR_NO_DATA` when empty.
    #[cfg(windows)]
    if e.raw_os_error() == Some(win32::ERROR_NO_DATA) {
        return true;
    }
    false
}

/// Per-stream capture window: half the output budget, rounded up.
fn per_stream_keep() -> usize {
    tool_limits().max_output_bytes.div_ceil(2)
}

/// Append partial stdout/stderr content to an error message.
fn append_partial_output(msg: &mut String, stdout: &BoundedCapture, stderr: &BoundedCapture) {
    append_stream(msg, "stdout", stdout);
    append_stream(msg, "stderr", stderr);
}

fn append_stream(msg: &mut String, name: &str, cap: &BoundedCapture) {
    if cap.total() == 0 {
        return;
    }
    msg.push_str(&format!("\n--- partial {name} ---\n"));
    msg.push_str(&String::from_utf8_lossy(&cap.materialize()));
    let skipped = cap.skipped();
    if skipped > 0 {
        msg.push_str(&format!(
            "\n\n... [{skipped} bytes truncated ({} total)] ...\n",
            cap.total()
        ));
    }
}

/// Kill and reap the child (its whole tree if `kill_tree`) and build an error
/// message with the reason and any partial output.
fn kill_and_error(
    child: &mut std::process::Child,
    pid: u32,
    kill_tree: bool,
    stdout: &BoundedCapture,
    stderr: &BoundedCapture,
    reason: &str,
) -> String {
    if kill_tree {
        kill_process_tree(pid);
    } else {
        let _ = child.kill();
    }
    let _ = child.wait();
    let mut msg = reason.to_string();
    append_partial_output(&mut msg, stdout, stderr);
    msg
}

/// Minimal Win32 constants and FFI for pipe handle management.
#[cfg(windows)]
mod win32 {
    unsafe extern "system" {
        pub(crate) fn SetNamedPipeHandleState(
            hNamedPipe: isize,
            lpMode: *mut u32,
            lpMaxCollectionCount: *mut u32,
            lpCollectDataTimeout: *mut u32,
        ) -> i32;

        pub(crate) fn SetHandleInformation(hObject: isize, dwMask: u32, dwFlags: u32) -> i32;
    }

    pub(crate) const PIPE_NOWAIT: u32 = 0x0000_0001;
    pub(crate) const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;

    /// `ReadFile` on a `PIPE_NOWAIT` pipe returns this when no data is available.
    pub(crate) const ERROR_NO_DATA: i32 = 232;
}
