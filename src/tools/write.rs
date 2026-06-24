use serde_json::{Value, json};

use super::{arg_str, resolve_path};

pub(super) fn description() -> &'static str {
    "Write content to a file, overwriting existing files. Creates parent directories as needed."
}

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the file (relative to workspace or absolute)"
            },
            "content": {
                "type": "string",
                "description": "Complete file content to write. Overwrites the file entirely if it already exists."
            }
        },
        "required": ["path", "content"]
    })
}

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let path = arg_str(args, "path").ok_or("Missing 'path' argument")?;
    let content = arg_str(args, "content").ok_or("Missing 'content' argument")?;
    let file_path = resolve_path(path, workspace)
        .map_err(|e| format!("Failed to resolve path '{path}': {e}"))?;
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent dir: {e}"))?;
    }
    std::fs::write(&file_path, content)
        .map_err(|e| format!("Failed to write {}: {e}", file_path.display()))?;
    Ok(format!(
        "Wrote {} bytes to {}",
        content.len(),
        file_path.display()
    ))
}
