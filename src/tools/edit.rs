use serde_json::{Value, json};

use super::{arg_str, resolve_path};

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the file"
            },
            "old_string": {
                "type": "string",
                "description": "Exact text to find"
            },
            "new_string": {
                "type": "string",
                "description": "Replacement text"
            }
        },
        "required": ["path", "old_string", "new_string"]
    })
}

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let path = arg_str(args, "path").ok_or("Missing 'path' argument")?;
    let old_string = arg_str(args, "old_string").ok_or("Missing 'old_string' argument")?;
    let new_string = arg_str(args, "new_string").ok_or("Missing 'new_string' argument")?;
    let file_path = resolve_path(path, workspace);
    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;
    let occurrences = content.matches(old_string).count();
    if occurrences == 0 {
        return Err(format!(
            "String not found in {}: '{}'",
            file_path.display(),
            old_string
        ));
    }
    if occurrences > 1 {
        return Err(format!(
            "Found {} occurrences of '{}' in {} — need unique match",
            occurrences,
            old_string,
            file_path.display()
        ));
    }
    let new_content = content.replacen(old_string, new_string, 1);
    std::fs::write(&file_path, &new_content)
        .map_err(|e| format!("Failed to write {}: {e}", file_path.display()))?;
    Ok(format!("Replaced 1 occurrence in {}", file_path.display()))
}
