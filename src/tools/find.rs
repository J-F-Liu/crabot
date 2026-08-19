use std::path::Path;
use std::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use globset::{GlobBuilder, GlobMatcher};
use serde_json::{Value, json};

use super::{Tool, arg_str, lock, resolve_path, tool_limits};

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
                    "description": "Glob pattern. Bare patterns match file names (e.g. \"*.rs\"); patterns with '/' match workspace-relative paths (e.g. \"src/**/*.ts\"). '*' stays within one segment; '**' crosses directories. Matching is case-insensitive unless the pattern contains an uppercase character."
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
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        execute(args, workspace, cancel)
    }
}

/// fd-style glob: bare patterns match file names; patterns with '/' match workspace-relative paths.
pub struct PatternMatcher {
    glob: GlobMatcher,
    matches_path: bool,
}

impl PatternMatcher {
    pub fn new(pattern: &str) -> Result<Self, String> {
        // Candidate paths are always '/'-separated (see `make_workspace_relative`),
        // so normalize backslashes and make Windows-style patterns work everywhere.
        let pattern = pattern.replace('\\', "/");
        // literal_separator keeps '*' inside one path segment, like fd's --glob.
        // Smart case: case-insensitive by default; a pattern with an uppercase
        // character switches to exact-case matching (globset folds ASCII only,
        // so detect ASCII uppercase to stay consistent).
        let case_insensitive = !pattern.chars().any(|c| c.is_ascii_uppercase());
        let glob = GlobBuilder::new(&pattern)
            .literal_separator(true)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|e| format!("Glob pattern error: {e}"))?;
        Ok(Self {
            glob: glob.compile_matcher(),
            matches_path: pattern.contains('/'),
        })
    }

    pub fn matches(&self, rel: &str) -> bool {
        if self.matches_path {
            self.glob.is_match(rel)
        } else {
            Path::new(rel)
                .file_name()
                .is_some_and(|name| self.glob.is_match(name))
        }
    }
}

pub(super) fn execute(
    args: &Value,
    workspace: &Path,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let max_lines = tool_limits().find_max_lines;

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

    let matcher = PatternMatcher::new(pattern_str)?;
    let (tx, rx) = mpsc::channel::<String>();
    let root_error = Mutex::new(None);
    // Capture references only: the visitor closures are moved onto worker threads.
    let (matcher, tx, root_error) = (&matcher, &tx, &root_error);

    // Parallel walk; standard ignore filters (hidden files, .gitignore, .ignore).
    ignore::WalkBuilder::new(&search_path)
        .standard_filters(true)
        .build_parallel()
        .run(move || {
            Box::new(move |result| {
                if cancel.is_cancelled() {
                    return ignore::WalkState::Quit;
                }
                match result {
                    // Skip unreadable entries, like fd. An IO error on the search
                    // root itself (e.g. permission denied) makes the whole result
                    // unreliable, so surface it instead of "No files matched.".
                    Err(err) => {
                        if err.depth() == Some(0) && !err.is_partial() {
                            *lock(root_error) = Some(err.to_string());
                            return ignore::WalkState::Quit;
                        }
                        tracing::debug!(%err, "skipping unreadable entry while walking");
                    }
                    Ok(entry) => {
                        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                            return ignore::WalkState::Continue;
                        }
                        let rel = super::make_workspace_relative(entry.path(), workspace);
                        if matcher.matches(&rel) {
                            let _ = tx.send(rel);
                        }
                    }
                }
                ignore::WalkState::Continue
            })
        });

    if cancel.is_cancelled() {
        return Err(super::CANCEL_REASON.into());
    }
    let root_err = lock(root_error).take();
    if let Some(err) = root_err {
        return Err(format!("Walk error: {err}"));
    }

    let mut results: Vec<String> = rx.try_iter().collect();
    if results.is_empty() {
        return Ok("No files matched.".into());
    }
    results.sort();
    let total = results.len();
    if total > max_lines {
        let skipped = total - max_lines;
        results.truncate(max_lines);
        return Ok(format!(
            "{}\n\n... [{skipped} lines truncated ({total} total, shows first {max_lines})] ...",
            results.join("\n")
        ));
    }

    Ok(super::truncate_output(results.join("\n")))
}
