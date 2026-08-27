use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use serde_json::{Value, json};

use crate::tools::{
    ChunkForwarder, OutputSink, Tool, WaitError, arg_str, bash_kit, tool_limits, wait_with_timeout,
};

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command via Bash. For builds, tests, and Git only; use dedicated tools for file operations."
    }

    fn instruction(&self) -> &str {
        "Execute a shell command in the workspace directory using Bash. Commands time out after 120 seconds by default; pass a `timeout` value in milliseconds to adjust. Use this tool for builds, tests, Git operations, package management, and other CLI tasks. Do not use this tool to read, write, search, or locate files, dedicated tools are available for those operations. Run `compgen -b` to list all available built-in commands (such as `json`, `csv`, `tomlq`, `http`); run `help <cmd>` for a description of a specific builtin."
    }

    fn schema(&self) -> Value {
        let limits = tool_limits();
        let default_desc = fmt_timeout(limits.command_timeout_ms);
        let max_desc = fmt_timeout(limits.max_command_timeout_ms);
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash shell command to execute. Use only for builds, tests, Git, package managers, and CLI tooling. Never use for file reading, writing, searching, or path-finding — use the dedicated `read`, `write`, `edit`, `search`, and `find` tools instead. Returns combined stdout and stderr."
                },
                "timeout": {
                    "type": "integer",
                    "description": format!("Timeout in milliseconds for the command. Defaults to {} ({default_desc}) if not provided. Values below 1000 are clamped up; maximum is {} ({max_desc}).", limits.command_timeout_ms, limits.max_command_timeout_ms),
                    "minimum": 1000,
                    "maximum": limits.max_command_timeout_ms
                }
            },
            "required": ["command"]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        execute(args, workspace, cancel)
    }

    fn execute_streaming_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
        sink: &OutputSink,
    ) -> Result<String, String> {
        execute_streaming(args, workspace, cancel, sink)
    }
}

pub(super) fn execute(
    args: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
) -> Result<String, String> {
    run(args, workspace, cancel, None)
}

/// Like [`execute`] but forwards incremental output chunks to `sink`.
pub(super) fn execute_streaming(
    args: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
    sink: &OutputSink,
) -> Result<String, String> {
    run(args, workspace, cancel, Some(Arc::clone(sink)))
}

fn run(
    args: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
    sink: Option<OutputSink>,
) -> Result<String, String> {
    let command = arg_str(args, "command").ok_or("Missing 'command' argument")?;
    let limits = tool_limits();
    let timeout = Duration::from_millis(
        crate::tools::arg_u64(args, "timeout")
            .map(|v| v.clamp(1000, limits.max_command_timeout_ms))
            .unwrap_or(limits.command_timeout_ms),
    );
    // Prefer bashkit's in-process interpreter; fall back to `bash -c` for
    // scripts it cannot faithfully execute.
    match bash_kit::analyze_script(command) {
        Ok(plan) => bash_kit::execute(command, workspace, timeout, cancel, plan, sink),
        Err(()) => {
            tracing::info!("bashkit falling back to host bash: {}", command);
            execute_real_bash(command, workspace, timeout, cancel, sink)
        }
    }
}

/// Fallback: run the command through a real `bash -c` process.
fn execute_real_bash(
    command: &str,
    workspace: &Path,
    timeout: Duration,
    cancel: &CancellationToken,
    sink: Option<OutputSink>,
) -> Result<String, String> {
    let (stdout_tx, stdout_rx) = crate::tools::create_pipe_pair("stdout")?;
    let (stderr_tx, stderr_rx) = crate::tools::create_pipe_pair("stderr")?;

    let mut cmd = std::process::Command::new("bash");
    // Drop secrets (names ending in `API_KEY`) and rustup's recursion counter
    // (rustup proxies abort past their counter max).
    crate::tools::sanitize_child_env(&mut cmd);
    cmd.arg("-c")
        .arg(command)
        .current_dir(workspace)
        .stdout(crate::tools::pipe_to_stdio(stdout_tx))
        .stderr(crate::tools::pipe_to_stdio(stderr_tx));

    crate::tools::detach_child(&mut cmd);

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to execute command: {e}"))?;

    let mut forwarder = ChunkForwarder::new(sink);
    let result = wait_with_timeout(
        child,
        Some(stdout_rx),
        Some(stderr_rx),
        timeout,
        timeout,
        true, // bash runs in its own process group → kill the whole group
        cancel,
        Some(&mut forwarder),
    );
    // Flush carried/coalesced bytes on every path (success, timeout, cancel).
    forwarder.finish();
    let output = result.map_err(WaitError::into_message)?;
    // The fallback runs a real (MSYS/Cygwin on Windows) bash, whose signal
    // deaths use the `signal << 8` exit-code encoding.
    Ok(crate::tools::format_command_output(&output, true))
}

/// Render a timeout in milliseconds as a human-readable duration, e.g.
/// `120000` → `"2 minutes"`, `90000` → `"90s"`.
fn fmt_timeout(ms: u64) -> String {
    if ms.is_multiple_of(60_000) {
        format!("{} minutes", ms / 60_000)
    } else {
        format!("{}s", ms / 1000)
    }
}
