use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Tool, arg_str, tool_limits, wait_with_timeout};

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command via Bash. For builds, tests, and Git only; use dedicated tools for file operations."
    }

    fn instruction(&self) -> &str {
        "Execute a shell command in the workspace directory using Bash. Commands time out after 120 seconds by default; pass a `timeout` value in milliseconds to adjust. Use this tool for builds, tests, Git operations, package management, and other CLI tasks. Do not use this tool to read, write, search, or locate files, dedicated tools are available for those operations."
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
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        execute(args, workspace, cancel)
    }
}

pub(super) fn execute(
    args: &Value,
    workspace: &Path,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let command = arg_str(args, "command").ok_or("Missing 'command' argument")?;
    let limits = tool_limits();
    let timeout_ms = super::arg_u64(args, "timeout")
        .map(|v| v.clamp(1000, limits.max_command_timeout_ms))
        .unwrap_or(limits.command_timeout_ms);
    let timeout = Duration::from_millis(timeout_ms);

    // Create unnamed pipe pairs for stdout and stderr.
    let (stdout_tx, stdout_rx) = super::create_pipe_pair("stdout")?;
    let (stderr_tx, stderr_rx) = super::create_pipe_pair("stderr")?;

    let mut cmd = std::process::Command::new("bash");
    // Don't inherit rustup's RUST_RECURSION_COUNT: once the value exceeds its internal max (20),
    // every rustup proxy (rustc/cargo/... are all shims) abort with "infinite recursion".
    cmd.env_remove("RUST_RECURSION_COUNT");
    cmd.arg("-c")
        .arg(command)
        .current_dir(workspace)
        .stdout(super::sender_to_stdio(stdout_tx))
        .stderr(super::sender_to_stdio(stderr_tx));

    // Make the child the leader of a new process group so that, on timeout,
    // we can kill the entire group (bash + any grandchildren it spawned)
    // instead of just the shell itself.
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

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to execute command: {e}"))?;

    let output = wait_with_timeout(
        child,
        Some(stdout_rx),
        Some(stderr_rx),
        timeout,
        true, // bash runs in its own process group → kill the whole group
        cancel,
    )?;

    Ok(super::format_command_output(&output))
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
