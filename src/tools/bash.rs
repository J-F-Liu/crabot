use serde_json::{Value, json};

use super::arg_str;

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to execute"
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
