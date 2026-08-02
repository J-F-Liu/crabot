use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, arg_path, make_workspace_relative, normalize_newlines, resolve_path};

/// A single edit operation with flexible field-name aliases for cross‑model
/// compatibility (e.g. `old_text` / `old` / `search`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Line-start offsets (line *k* starts at `line_starts[k-1]`), built with one
/// SIMD pass over `\n`-normalized `content` so [`line_number_at`] is O(log n).
fn build_line_starts(content: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            memchr::memchr_iter(b'\n', content.as_bytes()).map(|i| i + 1), // line starts after each `\n`
        )
        .collect()
}

/// 1-based line containing `byte_pos`.
fn line_number_at(line_starts: &[usize], byte_pos: usize) -> usize {
    // returns the number of elements in the prefix (the index of the first element of the second partition).
    line_starts.partition_point(|&s| s <= byte_pos)
}

/// All accepted JSON keys for an edit object (canonical names + aliases).
fn is_known_edit_key(key: &str) -> bool {
    matches!(
        key,
        "old_text"
            | "old"
            | "old_str"
            | "old_string"
            | "search"
            | "new_text"
            | "new"
            | "new_str"
            | "new_string"
            | "replace"
    )
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
        "Perform exact string replacements in an existing file. Use this tool for precise, localized edits instead of rewriting the entire file. Edits are validated before application and will fail if matches are ambiguous, duplicated, overlapping, or missing."
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
                    "description": "Ordered list of edits. Each old_text must appear exactly once, add surrounding context to disambiguate. Edits must not overlap.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_text": {
                                "type": "string",
                                "description": "Exact text to replace (must be unique in the file)"
                            },
                            "new_text": {
                                "type": "string",
                                "description": "Replacement text (may be empty to delete)"
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
    let mut errors: Vec<String> = Vec::new();

    // ── Validate path argument ────────────────────────────────────
    let path = arg_path(args);
    let file_path = match &path {
        Some(p) => match resolve_path(p, workspace) {
            Ok(fp) => Some(fp),
            Err(e) => {
                errors.push(format!("Failed to resolve path '{p}': {e}"));
                None
            }
        },
        None => {
            errors.push("Missing 'path' argument".to_string());
            None
        }
    };

    // ── Validate edits argument ───────────────────────────────────
    let edits = match args.get("edits") {
        Some(v) => match v.as_array() {
            Some(arr) if !arr.is_empty() => Some(arr),
            Some(_) => {
                errors.push("'edits' array must not be empty".to_string());
                None
            }
            None => {
                errors.push(format!("'edits' must be an array, got {}", v));
                None
            }
        },
        None => {
            errors.push("Missing 'edits' argument".to_string());
            None
        }
    };

    // Report arg-level errors before proceeding.
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    // Safe: we returned above if either was invalid.
    let file_path = file_path.unwrap();
    let display_path = make_workspace_relative(&file_path, workspace);
    let edits = edits.unwrap();

    let raw = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {display_path}: {e}"))?;

    // Normalize CRLF → LF in file and edits; file is written back with `\n` endings.
    let content = normalize_newlines(&raw);

    // Line index built lazily — only error messages (duplicates/overlaps) need line numbers.
    let mut line_starts: Option<Vec<usize>> = None;

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
        // Validate JSON keys before deserialising to report unexpected fields with a clear message.
        let mut edit_errors: Vec<String> = Vec::new();
        if let Some(obj) = edit_value.as_object() {
            for key in obj.keys() {
                if !is_known_edit_key(key) {
                    edit_errors.push(format!(
                        "Edit {idx}: unexpected field '{key}', accepted fields are: old_text, new_text"
                    ));
                }
            }
        }
        if !edit_errors.is_empty() {
            errors.extend(edit_errors);
            continue;
        }
        let edit: EditParam = match serde_json::from_value(edit_value.clone()) {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("Edit {idx}: {e}"));
                continue;
            }
        };
        let old_text = normalize_newlines(&edit.old_text);
        let new_text = normalize_newlines(&edit.new_text);

        // An empty old_text matches everywhere; reject it before the search.
        if old_text.is_empty() {
            errors.push(format!("Edit {idx}: 'old_text' must not be empty"));
            continue;
        }

        let start = match content.find(old_text.as_ref()) {
            Some(s) => s,
            None => {
                errors.push(format!(
                    "Edit {idx}: string of old_text not found in {display_path}",
                ));
                continue;
            }
        };

        // Search for a second occurrence after the next char boundary (avoids slicing mid-char).
        let search_from = content[start..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| start + i)
            .unwrap_or(content.len());
        if let Some(pos) = content[search_from..].find(old_text.as_ref()) {
            let second = search_from + pos;
            let starts = line_starts.get_or_insert_with(|| build_line_starts(&content));
            errors.push(format!(
                "Edit {idx}: found multiple occurrences of '{old_text}' in {display_path} (lines {} and {}) — need unique match, add surrounding context to disambiguate",
                line_number_at(starts, start),
                line_number_at(starts, second),
            ));
            continue;
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
            let starts = line_starts.get_or_insert_with(|| build_line_starts(&content));
            errors.push(format!(
                "Edits {} and {} overlap: edit {} range [lines {}..{}] conflicts with edit {} range [lines {}..{}]",
                a.idx, b.idx, a.idx,
                line_number_at(starts, a.start),
                line_number_at(starts, a.end - 1),
                b.idx,
                line_number_at(starts, b.start),
                line_number_at(starts, b.end - 1),
            ));
        }
    }

    // Report all collected errors at once.
    if !errors.is_empty() {
        return Err(errors.join("\n"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Write a throwaway file in a unique temp dir and run [`execute`].
    fn run_edit(
        dir: &Path,
        file_name: &str,
        body: &str,
        args: serde_json::Value,
    ) -> Result<String, String> {
        let file = dir.join(file_name);
        std::fs::write(&file, body).unwrap();
        let mut args = args;
        args["path"] = json!(file.to_str().unwrap());
        execute(&args, Path::new("."))
    }

    #[test]
    fn duplicate_occurrence_reports_line_numbers() {
        let dir = std::env::temp_dir().join(format!("crabot_edit_dup_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = run_edit(
            &dir,
            "dup.txt",
            "first line\nsecond foo\nthird foo\nlast\n",
            json!({ "edits": [{ "old_text": "foo", "new_text": "bar" }] }),
        )
        .unwrap_err();
        assert!(err.contains("lines 2 and 3"), "got: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overlap_reports_line_numbers_with_multibyte_ends() {
        // Second old_text ends in multi-byte 'é'; end line must not slice mid-char.
        let dir = std::env::temp_dir().join(format!("crabot_edit_ovl_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = run_edit(
            &dir,
            "overlap.txt",
            "abécd\n",
            json!({
                "edits": [
                    { "old_text": "abé", "new_text": "x" },
                    { "old_text": "bécd", "new_text": "y" }
                ]
            }),
        )
        .unwrap_err();
        assert!(err.contains("overlap"), "got: {err}");
        assert!(err.contains("lines 1..1"), "got: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn line_number_at_matches_byte_and_char_boundaries() {
        // "é" is 2 bytes, so its start is a non-char-aligned byte offset.
        let content = "abé\ncd\n";
        let starts = build_line_starts(content);
        assert_eq!(starts, vec![0, 5, 8]); // 8 = empty line after trailing \n
        // bytes: 0/4/5/7/8 → lines 1/1/2/2/3.
        for (pos, line) in [(0, 1), (1, 1), (4, 1), (5, 2), (6, 2), (7, 2), (8, 3)] {
            assert_eq!(line_number_at(&starts, pos), line, "byte {pos}");
        }
        // Empty tail: a match that ends exactly at the last byte of the file.
        let starts = build_line_starts("x");
        assert_eq!(line_number_at(&starts, 0), 1);
    }

    #[test]
    fn successful_edit_needs_no_line_index() {
        let dir = std::env::temp_dir().join(format!("crabot_edit_ok_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_edit(
            &dir,
            "ok.txt",
            "one\ntwo foo\nthree\n",
            json!({ "edits": [{ "old_text": "foo", "new_text": "bar" }] }),
        )
        .unwrap();
        assert!(result.contains("Applied 1 edits"), "got: {result}");
        assert_eq!(
            std::fs::read_to_string(dir.join("ok.txt")).unwrap(),
            "one\ntwo bar\nthree\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deserialize_edit_params_with_unknown_fields() {
        // Valid: known fields only.
        let v = json!({"old_text": "foo", "new_text": "bar"});
        assert!(serde_json::from_value::<EditParam>(v).is_ok());

        // Valid: aliases work.
        let v = json!({"old": "foo", "new": "bar"});
        assert!(serde_json::from_value::<EditParam>(v).is_ok());

        // deny_unknown_fields: unknown fields rejected with a message listing accepted keys.
        let v = json!({"old_text": "foo", "new_text": "bar", "bogus": 1});
        let err = serde_json::from_value::<EditParam>(v).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown field"),
            "expected 'unknown field' error, got: {msg}"
        );
    }
}
