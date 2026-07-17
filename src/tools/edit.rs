use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, arg_str, make_workspace_relative, normalize_newlines, resolve_path};

/// A single edit operation with flexible field-name aliases for cross‑model
/// compatibility (e.g. `old_text` / `old` / `search`).
#[derive(Deserialize)]
pub struct EditParam {
    #[serde(alias = "old")]
    #[serde(alias = "old_str")]
    #[serde(alias = "old_string")]
    #[serde(alias = "search")]
    pub old_text: String,

    #[serde(alias = "new")]
    #[serde(alias = "new_str")]
    #[serde(alias = "new_string")]
    #[serde(alias = "replace")]
    pub new_text: String,
}

pub struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Replace exact string matches in a file through an ordered list of edits. Each old_text must appear exactly once in the original file. Edits must not overlap or nested. If two changes touch the same block or nearby lines, merge them into one edit instead."
    }

    fn instruction(&self) -> &str {
        "Perform exact string replacements in an existing file. Use this tool for precise, localized edits instead of rewriting the entire file. Each old_text must uniquely identify a single location in the file. Include sufficient surrounding context to ensure an exact match. Edits are validated before application and will fail if matches are ambiguous, duplicated, overlapping, or missing."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file (relative to workspace or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "Ordered list of edits. Each old_text must appear exactly once in the original file. Edits must not overlap and are applied simultaneously.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_text": {
                                "type": "string",
                                "description": "Exact text to search for and replace. Must appear exactly once in the original file."
                            },
                            "new_text": {
                                "type": "string",
                                "description": "Replacement text to substitute in place of old_text"
                            }
                        },
                        "required": ["old_text", "new_text"]
                    }
                }
            },
            "required": ["path", "edits"]
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
    let path = arg_str(args, "path").ok_or("Missing 'path' argument")?;
    let file_path = resolve_path(path, workspace)
        .map_err(|e| format!("Failed to resolve path '{path}': {e}"))?;
    let display_path = make_workspace_relative(&file_path, workspace);

    let edits = args
        .get("edits")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'edits' argument")?;
    if edits.is_empty() {
        return Err("'edits' array must not be empty".to_string());
    }

    let raw = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {display_path}: {e}"))?;

    // Normalize `\r\n` → `\n` for the file content, `old_text` and `new_text` in each edit.
    // This ensures consistent behavior across platforms and avoids accidental mismatches due to line ending differences.
    // The entire file is written back with `\n` endings.
    let content = normalize_newlines(&raw);

    // ── Phase 1: locate each old_text, record byte range ──────────
    struct LocatedEdit {
        idx: usize,
        start: usize,
        end: usize,
        new_text: String,
    }

    let mut located: Vec<LocatedEdit> = Vec::with_capacity(edits.len());
    for (i, edit_value) in edits.iter().enumerate() {
        let idx = i + 1; // 1‑based for human‑readable messages
        let edit: EditParam =
            serde_json::from_value(edit_value.clone()).map_err(|e| format!("Edit {idx}: {e}"))?;
        let old_text = normalize_newlines(&edit.old_text);
        let new_text = normalize_newlines(&edit.new_text);

        // An empty old_text matches everywhere; reject it before the search.
        if old_text.is_empty() {
            return Err(format!("Edit {idx}: 'old_text' must not be empty"));
        }

        let start = content.find(old_text.as_ref()).ok_or_else(|| {
            format!("Edit {idx}: string not found in {display_path}: '{old_text}'",)
        })?;

        // Verify uniqueness: no second occurrence (including overlapping ones).
        // Search from the next character boundary after `start` to avoid
        // panicking when old_text begins with a multi-byte (non-ASCII) char.
        let search_from = content[start..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| start + i)
            .unwrap_or(content.len());
        if let Some(pos) = content[search_from..].find(old_text.as_ref()) {
            return Err(format!(
                "Edit {idx}: found multiple occurrences of '{old_text}' in {display_path} (positions {start} and {}) — need unique match",
                search_from + pos,
            ));
        }

        located.push(LocatedEdit {
            idx,
            start,
            end: start + old_text.len(),
            new_text: new_text.into_owned(),
        });
    }

    // ── Phase 2: check for overlapping ranges ─────────────────────
    located.sort_by_key(|e| e.start);
    for pair in located.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        if a.end > b.start {
            return Err(format!(
                "Edits {} and {} overlap: edit {} range [{}..{}) conflicts with edit {} range [{}..{})",
                a.idx, b.idx, a.idx, a.start, a.end, b.idx, b.start, b.end,
            ));
        }
    }

    // ── Phase 3: apply edits ───────────────────────────────────────
    let total_old: usize = located.iter().map(|e| e.end - e.start).sum();
    let total_new: usize = located.iter().map(|e| e.new_text.len()).sum();
    let mut result = String::with_capacity(content.len() - total_old + total_new);
    let mut cursor = 0usize;
    for edit in &located {
        result.push_str(&content[cursor..edit.start]);
        result.push_str(&edit.new_text);
        cursor = edit.end;
    }
    result.push_str(&content[cursor..]);

    std::fs::write(&file_path, &result)
        .map_err(|e| format!("Failed to write {display_path}: {e}"))?;
    Ok(format!("Applied {} edits in {display_path}", located.len(),))
}
