use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use super::{Tool, arg_str, resolve_path};

pub struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in file contents. Returns file:line:content matches. Respects .gitignore."
    }

    fn instruction(&self) -> &str {
        "Search file contents using a regular expression. Returns matches in file:line:content format. Respects .gitignore rules. Use this tool to locate definitions, references, usages, or other patterns across the codebase before reading or editing specific files."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression (RE2 syntax) to match against each line of file contents"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search within, defaults to '.', i.e, search inside workspace. If path is directory, the search is recursive and depth-first, respects .gitignore rules."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        execute_search(args, workspace, cancel)
    }
}

pub(super) fn execute_search(
    args: &Value,
    workspace: &Path,
    cancel: &AtomicBool,
) -> Result<String, String> {
    const MAX_LINES: usize = 500;

    let pattern = arg_str(args, "pattern").ok_or("Missing 'pattern' argument")?;
    let search_path = arg_str(args, "path")
        .map(|p| resolve_path(p, workspace))
        .transpose()
        .map_err(|e| format!("Failed to resolve path: {e}"))?
        .unwrap_or_else(|| workspace.to_path_buf());

    let re = regex::Regex::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;

    let mut out = String::new();
    let mut out_lines: usize = 0;
    let mut found = false;
    let mut truncated = false;

    if search_path.is_file() {
        let path_string = super::make_workspace_relative(&search_path, workspace);
        let content = std::fs::read_to_string(&search_path)
            .map_err(|e| format!("Failed to read {path_string}: {e}"))?;
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                found = true;
                if out_lines < MAX_LINES {
                    let _ = std::fmt::Write::write_fmt(
                        &mut out,
                        format_args!("{}:{}:{}\n", path_string, i + 1, line),
                    );
                    out_lines += 1;
                } else {
                    truncated = true;
                    break;
                }
            }
        }
    } else if search_path.is_dir() {
        let walker = ignore::WalkBuilder::new(&search_path)
            .standard_filters(true)
            .build();
        for entry in walker {
            if out_lines >= MAX_LINES {
                truncated = true;
                break;
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
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
            let path_string = super::make_workspace_relative(file_path, workspace);
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    found = true;
                    if out_lines < MAX_LINES {
                        let _ = std::fmt::Write::write_fmt(
                            &mut out,
                            format_args!("{}:{}:{}\n", path_string, i + 1, line),
                        );
                        out_lines += 1;
                    } else {
                        truncated = true;
                        break;
                    }
                }
            }
        }
    } else {
        return Err(format!(
            "Path does not exist or is not searchable: {}",
            super::make_workspace_relative(&search_path, workspace)
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        let _ =
            std::fmt::Write::write_fmt(&mut out, format_args!("\n... [search was cancelled]\n"));
        Ok(super::truncate_output(out))
    } else if !found {
        Ok("No matches found.".into())
    } else if truncated {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "\n... [output truncated at {MAX_LINES} lines; more matches exist but were omitted] ...\n"
            ),
        );
        Ok(super::truncate_output(out))
    } else {
        Ok(super::truncate_output(out))
    }
}
