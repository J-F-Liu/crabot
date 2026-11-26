use std::path::Path;
use tokio_util::sync::CancellationToken;

use serde_json::{Value, json};

use crate::tools::{
    Tool, make_workspace_relative, normalize_newlines, required_path, required_str,
    resolve_path_partial,
};

pub struct WriteTool;

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write complete file content, overwriting existing files and creating parent directories as needed. Briefly describe each file's intended changes, and make multiple calls in one response when useful."
    }

    fn schema(&self) -> Value {
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

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        _cancel: &CancellationToken,
    ) -> Result<String, String> {
        execute(args, workspace)
    }
}

pub(super) fn execute(args: &Value, workspace: &Path) -> Result<String, String> {
    let path = required_path(args)?;
    let content_raw = required_str(args, "content")?;
    let content = normalize_newlines(content_raw);
    let file_path = resolve_path_partial(path, workspace)
        .map_err(|e| format!("Failed to resolve path '{path}': {e}"))?;
    let display_path = make_workspace_relative(&file_path, workspace);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent dir: {e}"))?;
    }
    std::fs::write(&file_path, content.as_ref())
        .map_err(|e| format!("Failed to write {display_path}: {e}"))?;
    Ok(format!("Wrote {} bytes to {display_path}", content.len(),))
}
