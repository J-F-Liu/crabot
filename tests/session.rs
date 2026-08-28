//! Integration tests for session persistence in `crabot::session`.

use std::path::PathBuf;

use crabot::chat::TurnBody;
use crabot::model::ModelConfig;
use crabot::session::{Session, SessionRecord, list_session_paths};
use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent, ToolCall, ToolResponse};

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

/// An assistant message carrying a single tool call.
fn tool_call(call_id: &str, fn_name: &str, args: serde_json::Value) -> ChatMessage {
    ChatMessage::new(
        ChatRole::Assistant,
        MessageContent::from_tool_calls(vec![ToolCall {
            call_id: call_id.into(),
            fn_name: fn_name.into(),
            fn_arguments: args,
            thought_signatures: None,
        }]),
    )
}

/// A tool message answering `call_id`.
fn tool_result(call_id: &str) -> ChatMessage {
    ChatMessage::new(ChatRole::Tool, vec![ToolResponse::new(call_id, "ok")])
}

/// History roles in order.
fn roles(session: &Session) -> Vec<ChatRole> {
    session.history.iter().map(|m| m.role.clone()).collect()
}

/// Number of tool/temp turns across all dialogs.
fn tool_turns(session: &Session) -> usize {
    session
        .dialogs
        .iter()
        .flat_map(|d| &d.turns)
        .filter(|t| matches!(&t.body, TurnBody::Tool(_) | TurnBody::Temp(_)))
        .count()
}

#[test]
fn search_covers_dialog_and_turn_headers() {
    use crabot::chat::{ToolCall, ToolResult, Turn};
    use crabot::session::{SearchHit, SearchHitKind};
    use crabot::user::WorkMode;

    let mut session = Session::new();
    session.add_dialog("Fix the script".into(), Some(WorkMode::from("code")));
    session.push_turn(Turn::user("hello there"));
    session.push_turn(Turn::assistant("script edit done", None));
    session.push_turn(Turn::from_tool_calls(vec![ToolCall {
        name: "bash".into(),
        call_id: None,
        args: serde_json::json!({ "cmd": "ls" }),
    }]));
    session.push_turn(Turn::from_tool_results(vec![ToolResult {
        name: "edit".into(),
        call_id: Some("c1".into()),
        args: serde_json::json!({ "path": "src/main.rs" }),
        result: Ok("modified 3 lines".into()),
        timestamp: "15:00:00".into(),
        streaming: false,
    }]));
    session.dialogs[0].turns[0].timestamp = "12:34:56".into();
    session.dialogs[0].turns[1].timestamp = "13:45:00".into();
    session.dialogs[0].turns[2].timestamp = "14:00:00".into();
    session.add_dialog("Empty dialog".into(), None);

    let dialog_hits = |hits: &[SearchHit]| -> Vec<usize> {
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::DialogHeader)
            .map(|h| h.flat_idx)
            .collect()
    };
    let turn_hits = |hits: &[SearchHit]| -> Vec<usize> {
        hits.iter()
            .filter(|h| h.kind == SearchHitKind::Turn)
            .map(|h| h.flat_idx)
            .collect()
    };

    // Dialog title match jumps to the dialog's first turn.
    assert_eq!(dialog_hits(&session.search("script")), vec![0]);
    // Work-mode badge match is case-insensitive.
    assert_eq!(dialog_hits(&session.search("CODE")), vec![0]);
    // Header hits come before turn hits in visual order.
    let hits = session.search("script");
    assert_eq!(hits[0].kind, SearchHitKind::DialogHeader);
    assert_eq!(hits[1].kind, SearchHitKind::Turn);
    assert_eq!(hits[1].flat_idx, 1);

    // Role labels are deliberately not searchable — uniform across turn kinds.
    assert!(session.search("user").is_empty());
    assert!(session.search("assistant").is_empty());
    // Timestamp shown in the turn header matches.
    assert_eq!(turn_hits(&session.search("12:34")), vec![0]);
    // Pending tool-call names match via the turn header badge.
    assert_eq!(turn_hits(&session.search("bash")), vec![2]);
    // Pending tool-call args match too (they're rendered in the turn).
    assert_eq!(turn_hits(&session.search("ls")), vec![2]);
    // Completed tool names match via the body too (turn 1 hits only on content).
    assert_eq!(turn_hits(&session.search("edit")), vec![1, 3]);
    // Completed tool results match via the body: result text and args.
    assert_eq!(turn_hits(&session.search("modified")), vec![3]);
    assert_eq!(turn_hits(&session.search("main.rs")), vec![3]);

    // The "Tool - {name}" role badge is searchable: a query like "Tool - read"
    // matches the badge, case-insensitively, for both pending and completed calls.
    assert_eq!(turn_hits(&session.search("Tool - bash")), vec![2]);
    assert_eq!(turn_hits(&session.search("tool - edit")), vec![3]);
    assert_eq!(turn_hits(&session.search("TOOL - BASH")), vec![2]);
    // "Tool -" alone matches every tool turn badge.
    assert_eq!(turn_hits(&session.search("Tool -")), vec![2, 3]);

    // Blank queries match nothing.
    assert!(session.search("  ").is_empty());
    // A dialog-header match in an empty dialog has no jump target — skipped.
    assert!(session.search("empty dialog").is_empty());
}

#[test]
fn header_hit_deduped_when_first_turn_matches() {
    use crabot::chat::Turn;
    use crabot::session::{SearchHit, SearchHitKind};

    let mut session = Session::new();
    session.add_dialog("user request".into(), None);
    session.push_turn(Turn::user("user data"));

    // Header and first turn share the same jump target — only one hit remains,
    // so Next/Prev never lands on the same target twice.
    let hits: Vec<SearchHit> = session.search("user");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, SearchHitKind::Turn);
    assert_eq!(hits[0].flat_idx, 0);
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
    session.save_with_tally().expect("second save");

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
    loaded.save_with_tally().expect("first jsonl save");
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

    let paths = list_session_paths(&ws, None).expect("list");
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
fn pop_last_turn_removes_trailing_turn_and_empty_dialog() {
    use crabot::chat::{Turn, TurnBody};
    let mut session = Session::new();
    session.add_dialog("test".into(), None);
    session.push_turn(Turn::user("hello"));
    session.push_turn(Turn::assistant("hi", None));

    let popped = session.pop_last_turn().expect("assistant turn");
    assert_eq!(popped.role, genai::chat::ChatRole::Assistant);
    let TurnBody::Text(tc) = &popped.body else {
        panic!("expected text turn");
    };
    assert_eq!(tc.content, "hi");
    assert_eq!(session.total_turns(), 1);

    // Popping the user turn empties the dialog and drops it.
    assert!(session.pop_last_turn().is_some());
    assert!(session.pop_last_turn().is_none());
    assert!(session.is_empty());
}

#[test]
fn tally_written_only_by_save_with_tally() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.save().expect("first save");
    let path = session.save_path().unwrap();

    // No-op save: nothing to append (meta unchanged, no new messages).
    session.save().expect("no-op save");
    let lines = std::fs::read_to_string(&path).unwrap();
    assert_eq!(lines.lines().count(), 1);

    // Counter change alone appends nothing — plain saves never write a Tally.
    session.requests = 5;
    session.save().expect("counter-only save");
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);

    // A new message persists without a Tally line.
    session.history.push(ChatMessage::user("second prompt"));
    session.save().expect("message save");
    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.requests, 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);

    // save_with_tally appends the cumulative Tally even without new history.
    session.save_with_tally().expect("tally save");
    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.requests, 5);
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);
}

#[test]
fn record_system_prompt_dedupes_and_roundtrips() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();

    // First record writes a new System message and persists it.
    assert!(session.record_system_prompt("preamble\nrules"));
    assert_eq!(session.history.len(), 1);
    assert_eq!(session.history[0].role, ChatRole::System);
    assert_eq!(
        session.history[0].content.joined_texts().as_deref(),
        Some("preamble\nrules")
    );

    // Identical prompt → deduped, no new record.
    assert!(!session.record_system_prompt("preamble\nrules"));
    assert_eq!(session.history.len(), 1);

    // A changed prompt appends a second record.
    assert!(session.record_system_prompt("preamble\nnew rules"));
    assert_eq!(session.history.len(), 2);

    // Empty / whitespace prompts are ignored.
    assert!(!session.record_system_prompt(""));
    assert!(!session.record_system_prompt("   \n"));
    assert_eq!(session.history.len(), 2);

    // Round trip: System records survive reload but never become dialogs.
    session.save().expect("save");
    let path = session.save_path().unwrap();
    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.history.len(), 2);
    assert_eq!(loaded.history[0].role, ChatRole::System);
    assert_eq!(loaded.history[1].role, ChatRole::System);
    assert!(loaded.dialogs.is_empty());
}

#[test]
fn system_prompt_record_roundtrips_with_conversation() {
    let ws = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.title = "With system".into();

    // Simulate a stream: system record first, then the user message.
    assert!(session.record_system_prompt("system prompt"));
    session.history.push(ChatMessage::user("hello"));
    session.save().expect("save");
    let path = session.save_path().unwrap();

    let mut loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.history.len(), 2);
    assert_eq!(loaded.history[0].role, ChatRole::System);
    assert_eq!(loaded.history[1].role, ChatRole::User);
    // rebuild_dialogs must not surface the system record as a turn.
    assert_eq!(loaded.total_turns(), 1);
    assert_eq!(loaded.dialogs[0].turns[0].role, ChatRole::User);

    // A later identical prompt is deduped against the recorded one.
    assert!(!loaded.record_system_prompt("system prompt"));
    assert_eq!(loaded.history.len(), 2);
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
fn compact_keeps_user_and_last_assistant_per_dialog() {
    let mut session = Session::new();
    session.history.push(ChatMessage::system("audit"));
    // Dialog 1: user → tool round → intermediate → final answer.
    session.history.push(ChatMessage::user("first prompt"));
    session
        .history
        .push(tool_call("c1", "bash", serde_json::json!({ "cmd": "ls" })));
    session.history.push(tool_result("c1"));
    session.history.push(ChatMessage::assistant("intermediate"));
    session.history.push(ChatMessage::assistant("final answer"));
    // Dialog 2: interrupted mid-tool — tool calls, no final text.
    session.history.push(ChatMessage::user("second prompt"));
    session.history.push(tool_call(
        "c2",
        "read",
        serde_json::json!({ "path": "a.txt" }),
    ));

    let compacted = session.compact();

    // Fresh id; the source is untouched; only the prompts and the final text
    // answer remain — the system record and all tool activity are gone.
    assert_ne!(compacted.id, session.id);
    assert_eq!(session.history.len(), 8);
    assert_eq!(
        roles(&compacted),
        vec![ChatRole::User, ChatRole::Assistant, ChatRole::User]
    );
    assert_eq!(
        compacted.history[1].content.joined_texts().as_deref(),
        Some("final answer")
    );
    assert_eq!(tool_turns(&compacted), 0);
    assert_eq!(compacted.dialogs.len(), 2);
    assert_eq!(compacted.dialogs[0].turns.len(), 2); // prompt + final answer
    assert_eq!(compacted.dialogs[1].turns.len(), 1); // interrupted — prompt only
    assert_eq!(compacted.dialogs[1].turns[0].role, ChatRole::User);
}

#[test]
fn fork_drops_system_prompt_records() {
    let mut session = Session::new();
    session.history.push(ChatMessage::system("audit 1"));
    session.history.push(ChatMessage::user("first prompt"));
    session.history.push(ChatMessage::assistant("first reply"));
    session.history.push(ChatMessage::system("audit 2"));
    session.history.push(ChatMessage::user("second prompt"));
    session.history.push(ChatMessage::assistant("second reply"));
    session.rebuild_dialogs();

    let forked = session.fork();

    // The fork keeps the conversation but starts its own audit trail.
    assert_eq!(session.history.len(), 6);
    assert_eq!(
        roles(&forked),
        vec![
            ChatRole::User,
            ChatRole::Assistant,
            ChatRole::User,
            ChatRole::Assistant
        ]
    );
    assert_eq!(
        forked.history[0].content.joined_texts().as_deref(),
        Some("first prompt")
    );
    assert_eq!(
        forked.history[3].content.joined_texts().as_deref(),
        Some("second reply")
    );
    assert_eq!(forked.dialogs.len(), 2);
}

#[test]
fn compact_skips_assistant_replies_with_tool_calls() {
    let mut session = Session::new();
    session.history.push(ChatMessage::user("prompt"));
    // A final answer that also carries tool calls is dropped entirely.
    session.history.push(ChatMessage::new(
        ChatRole::Assistant,
        MessageContent::from_parts(vec![
            "done".into(),
            ContentPart::ToolCall(ToolCall {
                call_id: "c1".into(),
                fn_name: "bash".into(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }),
        ]),
    ));

    let compacted = session.compact();

    // The mixed answer is skipped, not partially kept — only the prompt stays.
    assert_eq!(session.history.len(), 2);
    assert_eq!(compacted.history.len(), 1);
    assert_eq!(compacted.history[0].role, ChatRole::User);
}

#[test]
fn compact_without_workspace_is_in_memory_only() {
    let mut session = Session::new();
    session.history.push(ChatMessage::user("prompt"));
    session.history.push(ChatMessage::assistant("skipped"));
    session.history.push(ChatMessage::assistant("kept"));

    let compacted = session.compact();

    // Fresh id; the source is untouched.
    assert_ne!(compacted.id, session.id);
    assert_eq!(session.history.len(), 3);
    assert_eq!(compacted.history.len(), 2);
    assert_eq!(
        compacted.history[1].content.joined_texts().as_deref(),
        Some("kept")
    );
}

#[test]
fn has_reply_detects_assistant_messages() {
    let mut session = Session::new();
    assert!(!session.has_reply());
    session.history.push(ChatMessage::system("audit"));
    session.history.push(ChatMessage::user("first"));
    assert!(!session.has_reply());
    session.history.push(ChatMessage::assistant("reply"));
    assert!(session.has_reply());
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
