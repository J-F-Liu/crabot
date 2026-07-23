mod ask;
mod bash;
pub mod custom;
pub mod edit;
mod fetch;
mod find;
pub mod mcp;
mod read;
mod search;
pub mod todo;
mod write;

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use genai::chat::Tool as GenaiTool;
use interprocess::unnamed_pipe;
use serde_json::Value;

// ── Tool trait ──────────────────────────────────────────────────────

pub type ToolRef = Arc<dyn Tool>;

/// Trait implemented by every tool (built-in or custom).
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn instruction(&self) -> &str;
    fn schema(&self) -> Value;

    /// Cancel-aware wrapper: checks the cancellation flag *before* delegating to
    /// [`execute_inner`](Self::execute_inner). Individual tools may also
    /// honour the flag during long-running operations.
    fn execute(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled by user".into());
        }
        self.execute_inner(args, workspace, cancel)
    }

    /// Implement this instead of [`execute`](Self::execute) — the default
    /// `execute` wrapper already handles the pre-execution cancel check.
    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &AtomicBool,
    ) -> Result<String, String>;

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
    /// Create a new registry pre-populated with the nine built-in tools.
    pub fn new() -> Self {
        let todo_items: todo::TodoList = Arc::new(Mutex::new(Vec::new()));
        let builtin: Vec<ToolRef> = vec![
            Arc::new(read::ReadTool),
            Arc::new(write::WriteTool),
            Arc::new(edit::EditTool),
            Arc::new(find::FindTool),
            Arc::new(search::SearchTool),
            Arc::new(bash::BashTool),
            Arc::new(ask::AskTool),
            Arc::new(todo::TodoTool::new(Arc::clone(&todo_items))),
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

/// Format a process's stdout, stderr, and exit code into a single truncated string.
///
/// Combines `stdout` and `stderr` (prefixed with `STDERR:\n`), and appends the
/// exit code when the process did not succeed. The result is then truncated.
pub(crate) fn format_command_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("STDERR:\n");
        result.push_str(&stderr);
    }
    if !output.status.success() {
        if !result.is_empty() {
            result.push('\n');
        }
        let _ = std::fmt::Write::write_fmt(
            &mut result,
            format_args!("Exit code: {}", output.status.code().unwrap_or(-1)),
        );
    }

    truncate_output(result)
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

/// Default timeout (in seconds) for external commands spawned by tools.
pub(crate) const COMMAND_TIMEOUT_SECONDS: u64 = 120;

/// Maximum allowed timeout (in milliseconds) for a single command.
pub(crate) const MAX_COMMAND_TIMEOUT_MS: u64 = 600_000; // 10 minutes

/// Create an unnamed pipe pair for capturing child process output.
///
/// `label` is used in the error message (e.g. `"stdout"`, `"stderr"`).
fn create_pipe_pair(label: &str) -> Result<(unnamed_pipe::Sender, unnamed_pipe::Recver), String> {
    unnamed_pipe::pipe().map_err(|e| format!("Failed to create {label} pipe: {e}"))
}

/// Forcibly kill a process and its entire descendant tree.
///
/// On Unix the child should have been started with `process_group(0)` so it is
/// the leader of a new process group; sending the signal to `-pid` kills the
/// whole group, including any grandchildren the shell spawned.
///
/// On Windows, `taskkill /F /T` forcibly terminates the process and its whole
/// descendant tree.
pub(crate) fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
}

/// Convert an unnamed pipe `Sender` to `std::process::Stdio` for child process
/// stdout/stderr.
pub(crate) fn sender_to_stdio(sender: unnamed_pipe::Sender) -> std::process::Stdio {
    #[cfg(unix)]
    {
        use std::os::unix::io::OwnedFd;
        std::process::Stdio::from(OwnedFd::from(sender))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::OwnedHandle;
        std::process::Stdio::from(OwnedHandle::from(sender))
    }
}

/// Set a pipe receiver to non-blocking mode.
///
/// On Unix, uses `interprocess`'s `UnnamedPipeExt::set_nonblocking`.
/// On Windows, uses `SetNamedPipeHandleState` with `PIPE_NOWAIT`.
///
/// Returns an error if the mode cannot be set — a blocking pipe would
/// deadlock the polling loop, so the caller must treat this as fatal.
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
        // PIPE_NOWAIT is deprecated by Microsoft but still functional for
        // anonymous pipes. There is no direct replacement without switching
        // to overlapped I/O, which would require a much larger refactor.
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

/// Wait for a child process to finish, with a hard timeout.
///
/// The pipe receivers (`stdout`, `stderr`) are created with the `interprocess`
/// crate and switched to non-blocking mode so that they can be drained directly
/// in the polling loop — no reader threads are spawned. This avoids the
/// thread-leak problem where a surviving grandchild keeps a pipe write-end open
/// and blocks a detached reader thread forever.
///
/// On timeout the process — and, if `kill_tree` is set, its whole group/tree —
/// is killed and reaped *without* blocking on pipe EOF.
///
/// `kill_tree` should be `true` only when the child was started as a
/// process-group leader (e.g. bash with `process_group(0)` on Unix). Otherwise
/// `kill -9 -pid` would target an unrelated process group.
pub(crate) fn wait_with_timeout(
    mut child: std::process::Child,
    mut stdout: Option<unnamed_pipe::Recver>,
    mut stderr: Option<unnamed_pipe::Recver>,
    timeout: Duration,
    kill_tree: bool,
    cancel: &AtomicBool,
) -> Result<std::process::Output, String> {
    let pid = child.id();

    // Switch the pipe receivers to non-blocking mode so we can drain them in
    // the polling loop without spawning reader threads. A blocking pipe would
    // deadlock the loop, so propagate any failure.
    if let Some(ref r) = stdout {
        set_recver_nonblocking(r)?;
    }
    if let Some(ref r) = stderr {
        set_recver_nonblocking(r)?;
    }

    let deadline = Instant::now() + timeout;
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let mut tmp = [0u8; 8192];

    // Poll until the process exits, the deadline passes, or cancellation is
    // requested. Drain pipe output along the way to prevent the child from
    // blocking on a full pipe buffer.
    let status = loop {
        drain_pipe(stdout.as_mut(), &mut stdout_buf, &mut tmp);
        drain_pipe(stderr.as_mut(), &mut stderr_buf, &mut tmp);

        // Check for user cancellation before trying the child.
        if cancel.load(Ordering::Relaxed) {
            return Err(kill_and_error(
                &mut child,
                pid,
                kill_tree,
                &stdout_buf,
                &stderr_buf,
                "Cancelled by user",
            ));
        }

        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Err(kill_and_error(
                        &mut child,
                        pid,
                        kill_tree,
                        &stdout_buf,
                        &stderr_buf,
                        &format!("Command timed out after {}ms", timeout.as_millis()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Failed to wait on command: {e}"));
            }
        }
    };

    // Final drain: the process has exited, so the pipe write-ends should be
    // closed (unless a grandchild inherited them). Give the pipes up to 2
    // seconds to reach EOF; if a grandchild still holds them open, return
    // whatever output was collected so far.
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let stdout_done = drain_pipe(stdout.as_mut(), &mut stdout_buf, &mut tmp);
        let stderr_done = drain_pipe(stderr.as_mut(), &mut stderr_buf, &mut tmp);
        if stdout_done && stderr_done {
            break;
        }
        if Instant::now() >= drain_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(std::process::Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

/// Read all currently-available bytes from `reader` into `buf`.
///
/// Returns `true` if the pipe has reached EOF (or there is no reader), `false`
/// if it is still open but has no data available right now (non-blocking
/// `WouldBlock`).
fn drain_pipe(
    reader: Option<&mut unnamed_pipe::Recver>,
    buf: &mut Vec<u8>,
    tmp: &mut [u8],
) -> bool {
    let Some(reader) = reader else {
        return true;
    };
    loop {
        match reader.read(tmp) {
            Ok(0) => return true, // EOF — write end closed
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(ref e) if is_would_block(e) => return false,
            Err(_) => return true, // treat unexpected errors as EOF
        }
    }
}

/// Check whether an I/O error means "no data available right now" in
/// non-blocking mode.
fn is_would_block(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    // On Windows, `PIPE_NOWAIT` mode causes `ReadFile` to fail with
    // `ERROR_NO_DATA` when no data is available yet.
    #[cfg(windows)]
    if e.raw_os_error() == Some(win32::ERROR_NO_DATA) {
        return true;
    }
    false
}

/// Append partial stdout/stderr content to an error message.
fn append_partial_output(msg: &mut String, stdout: &[u8], stderr: &[u8]) {
    if !stdout.is_empty() {
        msg.push_str("\n--- partial stdout ---\n");
        msg.push_str(&String::from_utf8_lossy(stdout));
    }
    if !stderr.is_empty() {
        msg.push_str("\n--- partial stderr ---\n");
        msg.push_str(&String::from_utf8_lossy(stderr));
    }
}

/// Kill a child process (optionally its whole tree), reap it, and build an
/// error message with the given reason and any partial output collected.
fn kill_and_error(
    child: &mut std::process::Child,
    pid: u32,
    kill_tree: bool,
    stdout: &[u8],
    stderr: &[u8],
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

/// Minimal Win32 constants and FFI for named-pipe non-blocking mode.
#[cfg(windows)]
mod win32 {
    unsafe extern "system" {
        pub(crate) fn SetNamedPipeHandleState(
            hNamedPipe: isize,
            lpMode: *mut u32,
            lpMaxCollectionCount: *mut u32,
            lpCollectDataTimeout: *mut u32,
        ) -> i32;
    }

    pub(crate) const PIPE_NOWAIT: u32 = 0x0000_0001;

    /// `ERROR_NO_DATA` (232) — returned by `ReadFile` on a `PIPE_NOWAIT` pipe
    /// when no data is currently available.
    pub(crate) const ERROR_NO_DATA: i32 = 232;
}
