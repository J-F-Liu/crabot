use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use super::{Tool, arg_str, resolve_path};

pub struct FindTool;

impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Respects .gitignore and returns workspace-relative paths."
    }

    fn instruction(&self) -> &str {
        "Find files matching a glob pattern (for example, *.rs or src/**/*.ts). Respects .gitignore rules and returns workspace-relative paths, one per line. Use this tool to discover file locations before attempting to read or modify files."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match file paths (e.g. \"*.rs\", \"src/**/*.ts\")"
                },
                "path": {
                    "type": "string",
                    "description": "Root directory to search within (default: workspace root)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        _cancel: &AtomicBool,
    ) -> Result<String, String> {
        execute(args, workspace)
    }
}

pub(super) fn execute(args: &Value, workspace: &Path) -> Result<String, String> {
    const MAX_LINES: usize = 100;

    let pattern_str = arg_str(args, "pattern").ok_or("Missing 'pattern' argument")?;
    let search_path = arg_str(args, "path")
        .map(|p| resolve_path(p, workspace))
        .transpose()
        .map_err(|e| format!("Failed to resolve path: {e}"))?
        .unwrap_or_else(|| workspace.to_path_buf());

    if !search_path.exists() {
        return Err(format!(
            "Path does not exist: {}",
            super::make_workspace_relative(&search_path, workspace)
        ));
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
        let relative_str = super::make_workspace_relative(path, workspace);
        if pattern.matches(&relative_str) {
            results.push(relative_str);
        }
    }

    if results.is_empty() {
        Ok("No files matched.".into())
    } else {
        results.sort();
        let total = results.len();
        if total > MAX_LINES {
            let skipped = total - MAX_LINES;
            results.truncate(MAX_LINES);
            let mut output = results.join("\n");
            let _ = std::fmt::Write::write_fmt(
                &mut output,
                format_args!(
                    "\n\n... [{skipped} lines truncated ({total} total, shows first {MAX_LINES})] ..."
                ),
            );
            return Ok(output);
        }

        Ok(super::truncate_output(results.join("\n")))
    }
}
