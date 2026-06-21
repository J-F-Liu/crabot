use serde_json::{Value, json};

use super::arg_str;

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Execute commands in a Bash shell and return combined stdout/stderr. Use this tool for builds, tests, Git operations, package managers, and other CLI tasks.
Do not use shell commands to search, locate, read, or edit files when dedicated tools are available. Prefer `search`, `find`, `read`, and `edit` over utilities such as `grep`, `find`, `ls`, `cat`, and `sed` to ensure consistent cross-platform behavior.
The shell session starts in the workspace root; do not run `cd <workspace>` unless changing to a different directory is required.
"
            }
        },
        "required": ["command"]
    })
}

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let command = arg_str(args, "command").ok_or("Missing 'command' argument")?;
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(workspace)
        .output()
        .map_err(|e| format!("Failed to execute command: {e}"))?;

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
    Ok(result)
}
