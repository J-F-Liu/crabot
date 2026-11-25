//! Integration tests for session persistence in `crabot::session`.

mod common;

use std::path::PathBuf;

use common::TempDir;
use crabot::chat::TurnBody;
use crabot::model::ModelConfig;
use crabot::session::{RENEW_HINT_PROMPT, Session, SessionRecord, list_session_paths};
use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent, ToolCall, ToolResponse};

/// A temp dir guard plus the `workspace/` subdir a session persists into.
fn temp_workspace() -> (TempDir, PathBuf) {
    let dir = TempDir::new("session").unwrap();
    let ws = dir.mkdir("workspace").unwrap();
    (dir, ws)
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
    tool_result_with(call_id, "ok")
}

/// A tool message answering `call_id` with `content`.
fn tool_result_with(call_id: &str, content: &str) -> ChatMessage {
    ChatMessage::new(ChatRole::Tool, vec![ToolResponse::new(call_id, content)])
}

/// A user prompt carrying a work-mode prefix.
fn user_prompt(mode: &str, text: &str) -> ChatMessage {
    ChatMessage::new(
        ChatRole::User,
        MessageContent::from_parts(vec![format!("work-mode: {mode}").into(), text.into()]),
    )
}

/// An assistant message carrying one `ask` call.
fn ask_call(call_id: &str, question: &str) -> ChatMessage {
    ask_call_with(call_id, question, "", None)
}

/// An assistant message carrying one `ask` call plus its text preamble and reasoning.
fn ask_call_with(
    call_id: &str,
    question: &str,
    text: &str,
    reasoning: Option<&str>,
) -> ChatMessage {
    let mut msg = tool_call(
        call_id,
        "ask",
        serde_json::json!({ "question": question, "options": [] }),
    );
    if let Some(reasoning) = reasoning {
        msg.content
            .prepend(ContentPart::ReasoningContent(reasoning.into()));
    }
    if !text.is_empty() {
        msg.content.prepend(ContentPart::Text(text.into()));
    }
    msg
}

/// Parse a timestamp written in the persisted local-time format.
fn local_time(value: &str) -> chrono::DateTime<chrono::Local> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .expect("valid timestamp")
        .and_local_timezone(chrono::Local)
        .single()
        .expect("unambiguous local timestamp")
}

/// Format a timestamp at the precision persisted in session files.
fn time_text(time: chrono::DateTime<chrono::Local>) -> String {
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// History roles in order.
fn roles(session: &Session) -> Vec<ChatRole> {
    session.messages().map(|m| m.role.clone()).collect()
}

/// Display timestamps of all turns, including each tool result.
fn turn_times(session: &Session) -> Vec<String> {
    let mut times = Vec::new();
    for turn in session.dialogs.iter().flat_map(|dialog| &dialog.turns) {
        times.push(turn.timestamp.clone());
        if let TurnBody::Tool(tools) = &turn.body {
            times.extend(tools.iter().map(|tool| tool.timestamp.clone()));
        }
    }
    times
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

/// Reload a saved session from disk.
fn reload(session: &Session) -> Session {
    let path = session.save_path().expect("save path");
    Session::load(&path).expect("reload")
}

/// Assert a compacted assistant message kept its text and reasoning.
fn assert_preamble(msg: &ChatMessage, preamble: &str, reasoning: &str) {
    assert_eq!(msg.content.joined_texts().as_deref(), Some(preamble));
    assert_eq!(
        msg.content.joined_reasoning_content().as_deref(),
        Some(reasoning)
    );
}

/// Assert a compacted user prompt contains every snippet.
fn assert_prompt(msg: &ChatMessage, snippets: &[&str]) {
    let text = msg.content.joined_texts().expect("ask prompt text");
    for &snippet in snippets {
        assert!(text.contains(snippet), "missing {snippet:?} in {text:?}");
    }
}

#[test]
fn jsonl_round_trip_and_save_semantics() {
    let (_tmp, ws) = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws;
    session.title = "Initial".into();
    session.model = Some(ModelConfig {
        model_id: "test-model".into(),
        ..Default::default()
    });

    // Meta-only first save.
    session.save().expect("first save");
    let path = session.save_path().unwrap();
    assert!(path.extension().is_some_and(|e| e == "jsonl"));
    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.title, "Initial");
    assert_eq!(loaded.model.as_ref().unwrap().model_id, "test-model");
    assert_eq!(loaded.history_len(), 0);

    // Plain saves append messages but never a Tally.
    session.requests = 5;
    session.push_message(ChatMessage::user("prompt"));
    session.save().expect("message save");
    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.history_len(), 1);
    assert_eq!(loaded.requests, 0);

    // Renaming rewrites Meta; save_with_tally persists counters and updated_at.
    session.title = "Renamed".into();
    session.tokens.prompt = 10;
    let updated_at = local_time("2026-08-05 12:00:00");
    session.updated_at = updated_at;
    session.save_with_tally().expect("tally save");
    let loaded = Session::load(&path).expect("reload");
    assert_eq!(loaded.title, "Renamed");
    assert_eq!(loaded.requests, 5);
    assert_eq!(loaded.tokens.prompt, 10);
    assert_eq!(loaded.updated_at, updated_at);
    assert_eq!(loaded.persisted, loaded.history_len());
}

#[test]
fn workspace_switch_writes_full_history() {
    let (_tmp_a, ws_a) = temp_workspace();
    let (_tmp_b, ws_b) = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws_a;
    session.push_message(ChatMessage::user("first prompt"));
    session.push_message(ChatMessage::assistant("first reply"));
    session.save().expect("save in A");

    // The same session id continues in a new workspace directory.
    session.workspace = ws_b.clone();
    session.push_message(ChatMessage::user("second prompt"));
    session.save().expect("save in B");

    let loaded = Session::load(&session.save_path().unwrap()).expect("load from B");
    assert_eq!(loaded.history_len(), 3);
    assert_eq!(loaded.id, session.id);
}

#[test]
fn loads_only_jsonl_and_recovers_without_meta() {
    let (_tmp, ws) = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws.clone();
    session.save().expect("save");
    let path = session.save_path().unwrap();

    // Unsupported sidecars are ignored by the picker and rejected by the loader.
    std::fs::write(path.with_extension("json"), "{}").unwrap();
    std::fs::write(path.with_extension("txt"), "not a session").unwrap();
    let listed = list_session_paths(&ws, None).expect("list");
    let names: Vec<_> = listed
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec![format!("{}.jsonl", session.id)]);
    assert!(Session::load(&path.with_extension("json")).is_err());

    // A corrupt header falls back to the file stem; valid records still load.
    std::fs::write(
        &path,
        format!(
            "{{not json}}\n{}",
            serde_json::to_string(&SessionRecord::Message {
                message: ChatMessage::user("orphan"),
                timestamp: chrono::Local::now(),
            })
            .unwrap()
        ),
    )
    .unwrap();
    let loaded = Session::load(&path).expect("degraded load");
    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.history_len(), 1);
}

#[test]
fn timestamps_persist_across_reloads() {
    let (_tmp, ws) = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws;
    session.push_message(ChatMessage::user("hello"));
    session.push_message(tool_call(
        "c1",
        "read",
        serde_json::json!({ "path": "a.rs" }),
    ));
    session.push_message(tool_result_with("c1", "file contents"));
    session.push_message(ChatMessage::assistant("done"));
    session.push_notice("warning".into());
    session.updated_at = local_time("2026-08-05 12:00:00");
    session.rebuild_dialogs();

    let mut expected: Vec<String> = session
        .entries()
        .iter()
        .map(|entry| entry.timestamp.format("%H:%M:%S").to_string())
        .collect();
    expected.push(session.notices[0].timestamp.format("%H:%M:%S").to_string());
    assert_eq!(turn_times(&session), expected);

    session.save_with_tally().expect("save");
    let path = session.save_path().unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    let created = session.created_at_text();
    assert_eq!(records[0]["created_at"].as_str(), Some(created.as_str()));
    let message = records
        .iter()
        .find(|record| record["type"] == "message")
        .expect("message record");
    let first_timestamp = time_text(session.entries()[0].timestamp);
    assert_eq!(
        message["timestamp"].as_str(),
        Some(first_timestamp.as_str())
    );
    let tally = records
        .iter()
        .find(|record| record["type"] == "tally")
        .expect("tally record");
    assert_eq!(tally["updated_at"].as_str(), Some("2026-08-05 12:00:00"));

    let loaded = Session::load(&path).expect("reload");
    assert_eq!(turn_times(&loaded), expected);
    assert_eq!(
        time_text(loaded.notices[0].timestamp),
        time_text(session.notices[0].timestamp)
    );
    assert_eq!(loaded.updated_at, session.updated_at);
}

#[test]
fn rebuild_dialogs_groups_prompts_ask_and_attachments() {
    use crabot::chat::attachment_marker_message;
    use crabot::user::{AttachmentKind, UserAttachment, WorkMode};

    let attachments = vec![
        UserAttachment::new(AttachmentKind::Text, "notes.txt"),
        UserAttachment::new(AttachmentKind::Image, "screen.png"),
    ];
    let mut session = Session::new();
    session.push_message(user_prompt("code", "fix it"));
    session.push_message(attachment_marker_message(&attachments).expect("attachments"));
    session.push_message(tool_call(
        "c1",
        "read",
        serde_json::json!({ "path": "a.rs" }),
    ));
    session.push_message(tool_result("c1"));
    session.push_message(ChatMessage::assistant("which file?"));
    session.push_message(ask_call("c2", "Which file?"));
    session.push_message(tool_result_with("c2", "a.rs"));
    session.push_message(ChatMessage::assistant("fixed"));
    session.rebuild_dialogs();

    assert_eq!(session.dialogs.len(), 2);
    assert_eq!(session.dialogs[0].title, "fix it");
    assert_eq!(session.dialogs[0].mode, Some(WorkMode::from("code")));
    assert_eq!(session.dialogs[0].turns.len(), 4); // prompt, attachment, read, text
    let TurnBody::Attachment(items) = &session.dialogs[0].turns[1].body else {
        panic!("attachment marker should render an attachment turn");
    };
    assert_eq!(items, &attachments);

    assert_eq!(session.dialogs[1].title, "Which file?");
    assert_eq!(session.dialogs[1].mode, Some(WorkMode::from("code")));
    assert_eq!(session.dialogs[1].turns.len(), 2); // ask + final text
    assert!(matches!(
        &session.dialogs[1].turns[0].body,
        TurnBody::Tool(tools) if tools[0].result == Ok("a.rs".into())
    ));

    // An ask with no following turn merges into the preceding dialog.
    session.push_message(ask_call("c3", "Again?"));
    session.push_message(tool_result_with("c3", "no"));
    session.rebuild_dialogs();
    assert_eq!(session.dialogs.len(), 2);
    assert_eq!(session.dialogs[1].turns.len(), 3);

    // Live: ask opens and inherits the mode; other tools do not.
    assert_eq!(
        session.open_ask_dialog("read", &serde_json::json!({})),
        None
    );
    let index = session.open_ask_dialog(
        "ask",
        &serde_json::json!({ "question": "Continue?", "options": [] }),
    );
    assert_eq!(index, Some(2));
    assert_eq!(session.dialogs[2].title, "Continue?");
    assert_eq!(session.dialogs[2].mode, Some(WorkMode::from("code")));
}

#[test]
fn delete_dialog_rewrites_owned_history() {
    let (_tmp, ws) = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws;
    session.push_message(ChatMessage::user("first"));
    session.push_message(ChatMessage::assistant("one"));
    session.push_notice("first warning".into());
    session.push_message(ChatMessage::user("second"));
    session.push_message(ChatMessage::assistant("two"));
    session.push_notice("second warning".into());
    session.rebuild_dialogs();
    session.save().expect("save");

    assert!(session.delete_dialog(0));

    // Only the second dialog and its notice survive; the notice anchor shifts.
    assert_eq!(session.dialogs.len(), 1);
    assert_eq!(session.dialogs[0].title, "second");
    assert_eq!(roles(&session), vec![ChatRole::User, ChatRole::Assistant]);
    assert_eq!(session.notices.len(), 1);
    assert_eq!(session.notices[0].text, "second warning");
    assert_eq!(session.notices[0].at, 2);

    // The rewrite replaces the file rather than appending deleted records.
    let loaded = reload(&session);
    assert_eq!(loaded.history_len(), 2);
    assert_eq!(loaded.notices.len(), 1);
    assert_eq!(loaded.notices[0].at, 2);

    // UI-only dialogs can be removed; out-of-range deletes are rejected.
    session.add_dialog("empty".into(), None);
    assert!(session.delete_dialog(1));
    assert!(!session.delete_dialog(1));
}

#[test]
fn delete_ask_dialog_keeps_exchange() {
    let (_tmp, ws) = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws;
    session.push_message(user_prompt("code", "fix it"));
    session.push_message(tool_call(
        "c1",
        "read",
        serde_json::json!({ "path": "a.rs" }),
    ));
    session.push_message(tool_result("c1"));
    session.push_message(ask_call_with("c2", "Which file?", "which file?", None));
    session.push_message(tool_result_with("c2", "a.rs"));
    session.push_message(tool_call(
        "c3",
        "read",
        serde_json::json!({ "path": "b.rs" }),
    ));
    session.push_message(tool_result("c3"));
    session.push_message(ChatMessage::assistant("fixed"));
    session.rebuild_dialogs();
    assert_eq!(session.dialogs.len(), 2);
    assert!(session.dialog_is_ask(1));
    session.save().expect("save");

    assert!(session.delete_dialog(1));

    // The ask exchange stays; the turn after its response is removed.
    assert_eq!(session.history_len(), 5);
    assert_eq!(session.dialogs.len(), 1);
    assert_eq!(session.dialogs[0].turns.len(), 4); // prompt, read, preamble, ask tool
    assert!(matches!(
        &session.dialogs[0].turns[2].body,
        TurnBody::Text(text) if text.content == "which file?"
    ));
    assert!(session.messages().any(|msg| {
        msg.content
            .tool_calls()
            .iter()
            .any(|call| call.call_id == "c2")
    }));
    assert!(session.messages().any(|msg| {
        msg.content
            .tool_responses()
            .iter()
            .any(|response| response.call_id == "c2")
    }));
    assert!(
        !session
            .messages()
            .any(|msg| msg.content.joined_texts().as_deref() == Some("fixed"))
    );

    // Rebuild and reload both keep the ask dialog absent.
    session.rebuild_dialogs();
    let loaded = reload(&session);
    assert_eq!(loaded.history_len(), 5);
    assert_eq!(loaded.dialogs.len(), 1);
}

#[test]
fn compact_keeps_prompts_and_final_replies() {
    let mut session = Session::new();
    session.push_message(ChatMessage::system("audit"));
    // Dialog 1: prompt, tool round, intermediate and final text replies.
    session.push_message(ChatMessage::user("first prompt"));
    session.push_message(tool_call("c1", "bash", serde_json::json!({ "cmd": "ls" })));
    session.push_message(tool_result("c1"));
    session.push_message(ChatMessage::assistant("intermediate"));
    session.push_message(ChatMessage::assistant("final answer"));
    // Dialog 2: a text reply that also carries a tool call is dropped entirely.
    session.push_message(ChatMessage::user("second prompt"));
    session.push_message(ChatMessage::new(
        ChatRole::Assistant,
        MessageContent::from_parts(vec![
            "half-done".into(),
            ContentPart::ToolCall(ToolCall {
                call_id: "c2".into(),
                fn_name: "read".into(),
                fn_arguments: serde_json::json!({ "path": "a.txt" }),
                thought_signatures: None,
            }),
        ]),
    ));

    let compacted = session.compact();

    // The source is untouched; the copy keeps prompts and final replies only.
    assert_eq!(session.history_len(), 8);
    assert_ne!(compacted.id, session.id);
    assert_eq!(
        roles(&compacted),
        vec![ChatRole::User, ChatRole::Assistant, ChatRole::User]
    );
    assert_eq!(
        compacted.entries()[1]
            .message
            .content
            .joined_texts()
            .as_deref(),
        Some("final answer")
    );
    assert_eq!(tool_turns(&compacted), 0);
    assert_eq!(compacted.dialogs.len(), 2);
    assert_eq!(compacted.dialogs[0].turns.len(), 2); // prompt + final answer
    assert_eq!(compacted.dialogs[1].turns.len(), 1); // prompt only
}

#[test]
fn compact_preserves_ask_exchanges() {
    use crabot::tools::ask::TIMEOUT_REASON;

    let mut session = Session::new();
    session.push_message(user_prompt("code", "fix it"));
    // Auto-injected renew hints are not user prompts.
    session.push_message(user_prompt("code", RENEW_HINT_PROMPT));
    session.push_message(ask_call_with(
        "c1",
        "Which file?",
        "Let me check.",
        Some("Path unclear."),
    ));
    session.push_message(tool_result_with("c1", "a.rs"));
    session.push_message(ask_call_with(
        "c2",
        "Continue?",
        "Proceeding.",
        Some("No answer needed."),
    ));
    session.push_message(tool_result_with("c2", TIMEOUT_REASON));
    session.push_message(ask_call("c3", "Overwrite?"));
    session.push_message(tool_result_with("c3", "Error: Session cancelled by user."));
    session.push_message(ChatMessage::assistant("fixed a.rs"));

    let compacted = session.compact();

    // Answers and timeouts become prompts with their preambles; failures are dropped.
    assert_eq!(
        roles(&compacted),
        vec![
            ChatRole::User,
            ChatRole::Assistant,
            ChatRole::User,
            ChatRole::Assistant,
            ChatRole::User,
            ChatRole::Assistant
        ]
    );
    assert_preamble(
        &compacted.entries()[1].message,
        "Let me check.",
        "Path unclear.",
    );
    assert_prompt(
        &compacted.entries()[2].message,
        &["Which file?", "Answer: a.rs"],
    );
    assert_preamble(
        &compacted.entries()[3].message,
        "Proceeding.",
        "No answer needed.",
    );
    assert_prompt(
        &compacted.entries()[4].message,
        &["Continue?", TIMEOUT_REASON],
    );
    assert_eq!(
        compacted
            .dialogs
            .iter()
            .map(|dialog| dialog.title.as_str())
            .collect::<Vec<_>>(),
        ["fix it", "Which file?", "Continue?"]
    );
    assert_eq!(tool_turns(&compacted), 0);
}

#[test]
fn fork_drops_system_records_and_resets_persistence() {
    let mut session = Session::new();
    session.persisted = 42;
    session.push_message(ChatMessage::system("audit 1"));
    session.push_message(ChatMessage::user("first prompt"));
    session.push_message(ChatMessage::assistant("first reply"));
    session.push_message(ChatMessage::system("audit 2"));
    session.push_message(ChatMessage::user("second prompt"));
    session.push_message(ChatMessage::assistant("second reply"));
    session.rebuild_dialogs();

    let forked = session.fork();

    assert_ne!(forked.id, session.id);
    assert_eq!(forked.persisted, 0);
    assert_eq!(forked.history_len(), 4);
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
        forked.entries()[0]
            .message
            .content
            .joined_texts()
            .as_deref(),
        Some("first prompt")
    );
    assert_eq!(forked.dialogs.len(), 2);
    assert_eq!(session.history_len(), 6); // source untouched
}

#[test]
fn system_prompt_records_dedupe_and_stay_out_of_dialogs() {
    let (_tmp, ws) = temp_workspace();
    let mut session = Session::new();
    session.workspace = ws;

    assert!(session.record_system_prompt("preamble"));
    assert!(!session.record_system_prompt("preamble"));
    assert!(session.record_system_prompt("preamble v2"));
    assert!(!session.record_system_prompt("   "));
    assert_eq!(session.history_len(), 2);

    session.push_message(ChatMessage::user("hello"));
    session.save().expect("save");
    let mut loaded = Session::load(&session.save_path().unwrap()).expect("reload");
    assert_eq!(loaded.history_len(), 3);
    assert_eq!(loaded.dialogs.len(), 1);
    assert_eq!(loaded.dialogs[0].turns.len(), 1);
    assert_eq!(loaded.dialogs[0].turns[0].role, ChatRole::User);
    assert!(!loaded.record_system_prompt("preamble v2"));
}

#[test]
fn search_finds_dialog_and_turn_headers() {
    use crabot::chat::{ToolCall, ToolResult, Turn};
    use crabot::session::SearchHitKind;
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
    session.add_dialog("user request".into(), None);
    session.push_turn(Turn::user("user data"));

    let dialog_hits = |query: &str| -> Vec<usize> {
        session
            .search(query)
            .iter()
            .filter(|hit| hit.kind == SearchHitKind::DialogHeader)
            .map(|hit| hit.flat_idx)
            .collect()
    };
    let turn_hits = |query: &str| -> Vec<usize> {
        session
            .search(query)
            .iter()
            .filter(|hit| hit.kind == SearchHitKind::Turn)
            .map(|hit| hit.flat_idx)
            .collect()
    };

    // Dialog titles and work-mode badges are searchable; the header jump comes first.
    assert_eq!(dialog_hits("script"), vec![0]);
    assert_eq!(dialog_hits("CODE"), vec![0]);
    let hits = session.search("script");
    assert_eq!(hits[0].kind, SearchHitKind::DialogHeader);
    assert_eq!(hits[1].kind, SearchHitKind::Turn);
    assert_eq!(hits[1].flat_idx, 1);

    // Turn timestamps and tool badges/args/results are searchable.
    assert_eq!(turn_hits("12:34"), vec![0]);
    assert_eq!(turn_hits("bash"), vec![2]);
    assert_eq!(turn_hits("ls"), vec![2]);
    assert_eq!(turn_hits("Tool - edit"), vec![3]);
    assert_eq!(turn_hits("edit"), vec![1, 3]);
    assert_eq!(turn_hits("modified"), vec![3]);
    assert_eq!(turn_hits("main.rs"), vec![3]);

    // Blank queries and header-only dialogs produce no hits.
    assert!(session.search("  ").is_empty());
    assert!(session.search("empty dialog").is_empty());

    // Header and first turn share a jump target: only the turn hit remains.
    let hits = session.search("user");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, SearchHitKind::Turn);
    assert_eq!(hits[0].flat_idx, 4);
}

#[test]
fn pop_last_turn_drops_empty_dialogs() {
    use crabot::chat::Turn;

    let mut session = Session::new();
    session.add_dialog("test".into(), None);
    session.push_turn(Turn::user("hello"));
    session.push_turn(Turn::assistant("hi", None));

    assert!(session.pop_last_turn().is_some());
    assert!(session.pop_last_turn().is_some());
    assert!(session.pop_last_turn().is_none());
    assert!(session.is_empty());
}
