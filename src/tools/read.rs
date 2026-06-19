use serde_json::{Value, json};

use super::{arg_str, arg_u64, resolve_path};

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the file"
            },
            "offset": {
                "type": "integer",
                "description": "0-based line offset to start reading from"
            },
            "limit": {
                "type": "integer",
                "description": "Maximum lines to read"
            }
        },
        "required": ["path"]
    })
}

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let path = arg_str(args, "path").ok_or("Missing 'path' argument")?;
    let file_path = resolve_path(path, workspace);
    let offset = arg_u64(args, "offset").unwrap_or(0) as usize;
    let limit = arg_u64(args, "limit");

    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;

    let lines: Vec<&str> = content.lines().collect();
    let start = offset.min(lines.len());
    let end = match limit {
        Some(lim) => (start + lim as usize).min(lines.len()),
        None => lines.len(),
    };
    let selected = &lines[start..end];
    if selected.is_empty() {
        return Ok("(file is empty or offset beyond end)".into());
    }
    let mut out = String::new();
    for (i, line) in selected.iter().enumerate() {
        let _ =
            std::fmt::Write::write_fmt(&mut out, format_args!("{:>4}:{}\n", start + i + 1, line));
    }
    Ok(out)
}
