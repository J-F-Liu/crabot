//! Integration tests for the `edit` tool in `crabot::tools::edit`.

use std::path::Path;

use crabot::tools::edit::{EditParam, build_line_starts, execute, line_number_at};
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
fn non_array_edits_reports_value_type() {
    let dir = std::env::temp_dir().join(format!("crabot_edit_ty_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let err = run_edit(&dir, "ty.txt", "x\n", json!({ "edits": "boom" })).unwrap_err();
    assert!(err.contains("must be an array, got string"), "got: {err}");
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
