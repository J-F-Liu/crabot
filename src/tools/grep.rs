use serde_json::{Value, json};

use super::{arg_str, resolve_path};

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "Regular expression (RE2 syntax)"
            },
            "path": {
                "type": "string",
                "description": "File or directory to search (default \".\")"
            }
        },
        "required": ["pattern"]
    })
}

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let pattern = arg_str(args, "pattern").ok_or("Missing 'pattern' argument")?;
    let search_path = arg_str(args, "path")
        .map(|p| resolve_path(p, workspace))
        .unwrap_or_else(|| workspace.to_path_buf());

    let re = regex::Regex::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;

    let mut out = String::new();
    let mut found = false;

    if search_path.is_file() {
        let path_string = super::convert_path_to_unix_style(&search_path);
        let content = std::fs::read_to_string(&search_path)
            .map_err(|e| format!("Failed to read {}: {e}", &path_string))?;
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                let _ = std::fmt::Write::write_fmt(
                    &mut out,
                    format_args!("{}:{}:{}\n", &path_string, i + 1, line),
                );
                found = true;
            }
        }
    } else if search_path.is_dir() {
        let walker = ignore::WalkBuilder::new(&search_path)
            .standard_filters(true)
            .build();
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let file_path = entry.path();
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let path_string = super::convert_path_to_unix_style(&file_path);
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    let _ = std::fmt::Write::write_fmt(
                        &mut out,
                        format_args!("{}:{}:{}\n", &path_string, i + 1, line),
                    );
                    found = true;
                }
            }
        }
    }
    if !found {
        Ok("No matches found.".into())
    } else {
        Ok(out)
    }
}
