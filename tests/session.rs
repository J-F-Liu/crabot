//! Integration tests for session persistence in `crabot::session`.

use std::path::PathBuf;

use crabot::model::ModelConfig;
use crabot::session::{Session, SessionRecord, list_session_paths};
use genai::chat::ChatMessage;

fn temp_workspace() -> PathBuf {
    let base = std::env::temp_dir().join(format!("crabot-test-{}", std::process::id()));
    // Unique suffix so parallel tests don't interfere.
    static CNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = CNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = base.join(format!("t{n}"));
    let ws = dir.join("workspace");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&ws).unwrap();
    ws
}

#[test]
fn round_trip_jsonl() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.title = "Hello world".into();
    session.model = Some(ModelConfig {
        model_id: "test-model".into(),
        ..Default::default()
    });

    session.save().expect("first save");
    let path = session.save_path().unwrap();
    assert!(path.exists());
    assert!(path.extension().is_some_and(|e| e == "jsonl"));

    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.title, "Hello world");
    assert_eq!(loaded.model.as_ref().unwrap().model_id, "test-model");
    assert_eq!(loaded.persisted, loaded.history.len());
}

#[test]
fn incremental_append() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.title = "Incremental".into();

    // First save: meta only (no messages).
    session.save().expect("first save");
    let path = session.save_path().unwrap();
    let loaded1 = Session::load(&path).expect("load 1");
    assert!(loaded1.history.is_empty());

    // Add messages via history.
    let msg = ChatMessage::user("test prompt");
    session.history.push(msg.clone());
    session.requests = 1;
    session.tokens.prompt = 10;
    session.tokens.output = 20;
    session.updated_at = "2026-08-05 12:00:00".into();
    session.save().expect("second save");

    let loaded2 = Session::load(&path).expect("load 2");
    assert_eq!(loaded2.history.len(), 1);
    assert_eq!(loaded2.requests, 1);
    assert_eq!(loaded2.tokens.prompt, 10);
    assert_eq!(loaded2.updated_at, "2026-08-05 12:00:00");
    assert_eq!(loaded2.persisted, 1);
}

#[test]
fn migrate_legacy_json() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.title = "Legacy".into();
    session.history.push(ChatMessage::user("old prompt"));
    session.requests = 3;

    // Write a legacy .json file.
    let legacy_path = session.save_path().unwrap().with_extension("json");
    let json = serde_json::to_string_pretty(&session).unwrap();
    std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
    std::fs::write(&legacy_path, &json).unwrap();

    // Load from legacy path.
    let mut loaded = Session::load(&legacy_path).expect("load legacy");
    assert_eq!(loaded.persisted, 0); // not jsonl → 0
    assert_eq!(loaded.title, "Legacy");

    // Save → creates jsonl with full content; the legacy .json is removed.
    loaded.save().expect("first jsonl save");
    let jsonl_path = loaded.save_path().unwrap();
    assert!(jsonl_path.extension().is_some_and(|e| e == "jsonl"));
    assert!(jsonl_path.exists());
    assert!(!legacy_path.exists());

    // Load again via json path → prefers jsonl.
    let reloaded = Session::load(&legacy_path).expect("reload via json");
    assert_eq!(reloaded.requests, 3);
}

#[test]
fn list_dedupes_by_stem() {
    let ws = temp_workspace();
    // Use a current id so the file lands inside the scanned 3-month window.
    let mut session = Session::new();
    session.workspace = ws.clone();
    let jsonl_path = session.save_path().unwrap();
    session.save().expect("save jsonl");
    // Also write a fake legacy .json next to it.
    let json_path = jsonl_path.with_extension("json");
    std::fs::write(&json_path, format!("{{\"id\":\"{}\"}}\n", session.id)).unwrap();

    let paths = list_session_paths(&ws).expect("list");
    let file_names: Vec<_> = paths
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    assert_eq!(file_names, vec![format!("{}.jsonl", session.id)]);
}

#[test]
fn meta_update_appends_new_line() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.title = "Initial".into();
    session.save().expect("first save");
    let path = session.save_path().unwrap();

    // Rename the session — the next save must commit a new Meta line.
    session.title = "Renamed".into();
    session.save().expect("second save");

    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.title, "Renamed");
    // Meta from the first save, then the updated Meta line.
    let lines = std::fs::read_to_string(&path).unwrap();
    assert_eq!(lines.lines().count(), 2);
}

#[test]
fn tally_skipped_when_unchanged() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.save().expect("first save");
    let path = session.save_path().unwrap();

    // No-op save: nothing to append (meta unchanged, no new messages).
    session.save().expect("no-op save");
    let lines = std::fs::read_to_string(&path).unwrap();
    assert_eq!(lines.lines().count(), 1);

    // Counter change alone appends nothing — the tally follows new history.
    session.requests = 5;
    session.save().expect("counter-only save");
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);

    // A new message persists the updated counters in a fresh Tally line.
    session.history.push(ChatMessage::user("second prompt"));
    session.save().expect("message save");
    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.requests, 5);
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);
}

#[test]
fn fork_resets_persisted() {
    let mut session = Session::new();
    session.persisted = 42;
    let forked = session.fork();
    assert_eq!(forked.persisted, 0);
    assert_ne!(forked.id, session.id);
}

#[test]
fn workspace_switch_writes_full_history_to_new_file() {
    let ws_a = temp_workspace();
    let ws_b = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws_a.clone();
    session.title = "Workspace switch".into();
    session.save().expect("first save");

    // Two messages persisted under workspace A.
    session.history.push(ChatMessage::user("first prompt"));
    session.save().expect("save 2");
    session.history.push(ChatMessage::assistant("first reply"));
    session.save().expect("save 3");

    // Simulate a workspace switch: same session id, new workspace dir.
    session.workspace = ws_b.clone();
    session.history.push(ChatMessage::user("second prompt"));
    session.save().expect("save in workspace B");

    let path_b = session.save_path().unwrap();
    assert!(path_b.starts_with(&ws_b));
    let loaded = Session::load(&path_b).expect("load from B");
    // Full history must be present in the new file, not just the tail.
    assert_eq!(loaded.history.len(), 3);
    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.persisted, 3);
}

#[test]
fn jsonl_without_meta_falls_back_to_file_stem() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    let path = session.save_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Header line corrupt, but a message line still parses.
    std::fs::write(
        &path,
        format!(
            "{{this is not json}}\n{}",
            serde_json::to_string(&SessionRecord::Message {
                message: ChatMessage::user("orphan message")
            })
            .unwrap()
        ),
    )
    .unwrap();

    let loaded = Session::load(&path).expect("load degraded file");
    assert_eq!(loaded.id, session.id); // fell back to the file stem
    assert_eq!(loaded.history.len(), 1);
}
