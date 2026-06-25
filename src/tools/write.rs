use serde_json::{Value, json};

use super::{arg_str, make_workspace_relative, resolve_path_partial};

pub(super) fn instruction() -> &'static str {
    "You may use tools multiple times in a single response and continue writing after tool calls. When editing files, group your changes by file. For each file, provide a brief description of the intended changes."
}

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
    let file_path = resolve_path_partial(path, workspace)
        .map_err(|e| format!("Failed to resolve path '{path}': {e}"))?;
    let display_path = make_workspace_relative(&file_path, workspace);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent dir: {e}"))?;
    }
    std::fs::write(&file_path, content)
        .map_err(|e| format!("Failed to write {display_path}: {e}"))?;
    Ok(format!("Wrote {} bytes to {display_path}", content.len(),))
}
