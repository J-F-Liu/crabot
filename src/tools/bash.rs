use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Tool, arg_str};

/// Maximum seconds a bash command is allowed to run before being killed.
const BASH_TIMEOUT_SECONDS: u64 = 120;

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command via Bash. For builds, tests, and Git only; use dedicated tools for file operations."
    }

    fn instruction(&self) -> &str {
        "Execute a shell command in the workspace directory using Bash. Commands time out after 120 seconds. Use this tool for builds, tests, Git operations, package management, and other CLI tasks. Do not use this tool to read, write, search, or locate files. Dedicated tools are available for those operations."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash shell command to execute. Use only for builds, tests, Git, package managers, and CLI tooling. Never use for file reading, writing, searching, or path-finding — use the dedicated `read`, `write`, `edit`, `search`, and `find` tools instead. Returns combined stdout and stderr."
                }
            },
            "required": ["command"]
        })
    }

    fn execute(&self, args: &Value, workspace: &Path) -> Result<String, String> {
        execute(args, workspace)
    }
}

pub(super) fn execute(args: &Value, workspace: &Path) -> Result<String, String> {
    let command = arg_str(args, "command").ok_or("Missing 'command' argument")?;
    let mut cmd = std::process::Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

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

    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    let output = match rx.recv_timeout(Duration::from_secs(BASH_TIMEOUT_SECONDS)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("Failed to wait on command: {e}")),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Kill the whole process group (bash + children), then wait for
            // the worker thread to finish so we don't leak a live thread that
            // is still blocked on `wait_with_output`.
            kill_process_group(pid);
            let _ = handle.join();
            return Err(format!("Command timed out after {BASH_TIMEOUT_SECONDS}s"));
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // The worker thread ended without sending a result (e.g. it
            // panicked). Make sure no process is left running.
            kill_process_group(pid);
            let _ = handle.join();
            return Err("Command process terminated unexpectedly".to_string());
        }
    };

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

    Ok(super::truncate_output(result))
}

/// Kill the process group led by `pgid`.
///
/// On Unix the child is started with `process_group(0)`, making it the leader
/// of a new process group whose ID equals the child's PID. Sending the signal
/// to `-pgid` therefore kills the entire group, including any grandchildren
/// the shell spawned — not just the bash process itself.
///
/// On Windows, `taskkill /F /T` forcibly terminates the process and its whole
/// descendant tree.
fn kill_process_group(pgid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{pgid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pgid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
}
