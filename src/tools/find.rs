use serde_json::{Value, json};

use super::{arg_str, resolve_path};

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "Glob pattern (e.g. \"*.rs\", \"**/*.ts\")"
            },
            "path": {
                "type": "string",
                "description": "Directory to search (default \".\")"
            }
        },
        "required": ["pattern"]
    })
}

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let pattern_str = arg_str(args, "pattern").ok_or("Missing 'pattern' argument")?;
    let search_path = arg_str(args, "path")
        .map(|p| resolve_path(p, workspace))
        .unwrap_or_else(|| workspace.to_path_buf());

    if !search_path.exists() {
        return Err(format!("Path does not exist: {}", search_path.display()));
    }

    let pattern =
        glob::Pattern::new(pattern_str).map_err(|e| format!("Glob pattern error: {e}"))?;

    let mut results: Vec<String> = Vec::new();
    let walker = ignore::WalkBuilder::new(&search_path)
        .standard_filters(true)
        .build();

    for entry in walker {
        let entry = entry.map_err(|e| format!("Walk error: {e}"))?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(&search_path).unwrap_or(path);
        let relative_str = relative.to_string_lossy().replace('\\', "/");
        if pattern.matches(&relative_str) {
            results.push(super::convert_path_to_unix_style(path));
        }
    }

    if results.is_empty() {
        Ok("No files matched.".into())
    } else {
        results.sort();
        Ok(results.join("\n"))
    }
}
