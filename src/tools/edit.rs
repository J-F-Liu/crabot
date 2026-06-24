use serde_json::{Value, json};

use super::{arg_str, resolve_path};

pub(super) fn description() -> &'static str {
    "Replace exact string matches in a file through an ordered list of edits. Each old_text must appear exactly once in the original file. Edits must not overlap or nested. If two changes touch the same block or nearby lines, merge them into one edit instead."
}

pub(super) fn schema() -> Value {
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

pub(super) fn execute(args: &Value, workspace: &std::path::Path) -> Result<String, String> {
    let path = arg_str(args, "path").ok_or("Missing 'path' argument")?;
    let file_path = resolve_path(path, workspace)
        .map_err(|e| format!("Failed to resolve path '{path}': {e}"))?;
    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;

    let edits = args
        .get("edits")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'edits' argument")?;
    if edits.is_empty() {
        return Err("'edits' array must not be empty".to_string());
    }

    // ── Phase 1: locate each old_text, record byte range ──────────
    struct LocatedEdit<'a> {
        idx: usize,
        start: usize,
        end: usize,
        new_text: &'a str,
    }

    let mut located: Vec<LocatedEdit> = Vec::with_capacity(edits.len());
    for (i, edit) in edits.iter().enumerate() {
        let old = edit
            .get("old_text")
            .and_then(|v| v.as_str())
            .ok_or(format!("Edit {i}: missing 'old_text'"))?;
        let new = edit.get("new_text").and_then(|v| v.as_str()).unwrap_or("");

        let start = content.find(old).ok_or_else(|| {
            format!(
                "Edit {i}: string not found in {}: '{}'",
                file_path.display(),
                old
            )
        })?;

        // Verify uniqueness: no second occurrence (including overlapping ones)
        if let Some(pos) = content[start + 1..].find(old) {
            return Err(format!(
                "Edit {i}: found multiple occurrences of '{}' in {} (positions {} and {}) — need unique match",
                old,
                file_path.display(),
                start,
                start + 1 + pos,
            ));
        }

        located.push(LocatedEdit {
            idx: i,
            start,
            end: start + old.len(),
            new_text: new,
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
        result.push_str(edit.new_text);
        cursor = edit.end;
    }
    result.push_str(&content[cursor..]);

    std::fs::write(&file_path, &result)
        .map_err(|e| format!("Failed to write {}: {e}", file_path.display()))?;
    Ok(format!(
        "Applied {} edits in {}",
        located.len(),
        file_path.display()
    ))
}
