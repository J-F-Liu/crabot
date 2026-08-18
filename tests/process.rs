#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
use crabot::tools::OutputSink;
use crabot::tools::Tool;
use crabot::tools::make_strict_schema;
use crabot::tools::process::{ProcessLogs, ProcessTool, parse_env};
#[cfg(unix)]
use serde_json::Value;
use serde_json::json;
#[cfg(unix)]
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use tokio_util::sync::CancellationToken;

/// Helper: create a fresh temp workspace dir cleaned up on drop.
#[cfg(unix)]
struct TempDir {
    path: PathBuf,
}

#[cfg(unix)]
impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut dir = std::env::temp_dir();
        dir.push(format!("crabot_process_{}_{}", prefix, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { path: dir }
    }
}

#[cfg(unix)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn execute(tool: &ProcessTool, args: Value, workspace: &std::path::Path) -> Result<String, String> {
    tool.execute(&args, workspace, &CancellationToken::new())
}

#[cfg(unix)]
fn process_id(result: &str) -> String {
    // "Started process proc-N (os pid ...) ..." → "proc-N"
    result
        .split_whitespace()
        .nth(2)
        .expect("start result should name the process id")
        .to_string()
}

#[cfg(unix)]
/// Poll `logs` until it contains `needle`, panicking after ~2 s.
fn wait_for_log(tool: &ProcessTool, id: &str, workspace: &std::path::Path, needle: &str) {
    for _ in 0..200 {
        let logs = execute(tool, json!({"action": "logs", "process_id": id}), workspace).unwrap();
        if logs.contains(needle) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("logs never contained {needle:?}");
}

// ── Schema ────────────────────────────────────────────────────────

#[test]
fn schema_requires_action_with_full_enum() {
    let schema = ProcessTool.schema();
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required.len(), 1);
    assert_eq!(required[0], "action");

    let actions: Vec<&str> = schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        vec![
            "start", "list", "status", "logs", "input", "wait", "stop", "restart"
        ]
    );

    let signals: Vec<&str> = schema["properties"]["signal"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(signals, vec!["terminate", "kill", "interrupt"]);
}

// ── ProcessLogs ───────────────────────────────────────────────────

#[test]
fn logs_tail_returns_recent_lines() {
    let logs = ProcessLogs::new(1024 * 1024);
    for i in 0..10 {
        logs.push(format!("line {i}\n"));
    }
    assert_eq!(logs.tail(3), "line 7\nline 8\nline 9");
    assert_eq!(
        logs.tail(100),
        (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn logs_are_bounded_drop_oldest() {
    let logs = ProcessLogs::new(100);
    for i in 0..1000 {
        logs.push(format!("{i}\n"));
    }
    // Lines 975..=999 are each 4 bytes ("NNN\n") and fill the 100-byte cap
    // exactly; every earlier line has been dropped.
    assert_eq!(
        logs.tail(1000),
        (975..1000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn logs_merge_in_arrival_order() {
    let logs = ProcessLogs::new(1024 * 1024);
    logs.push("a\n".into());
    logs.push("b\n".into());
    logs.push("c\n".into());
    assert_eq!(logs.tail(3), "a\nb\nc");
}

// ── Strict schema coercion ────────────────────────────────────────

#[test]
fn strict_mode_coerces_propertyless_objects_to_string() {
    // `env` declares `additionalProperties`, not `properties`, so strict
    // mode coerces it to a string; `parse_env` accepts that form.
    let mut schema = ProcessTool.schema();
    make_strict_schema(&mut schema);
    assert_eq!(schema["properties"]["env"]["type"], "string");
    assert!(
        schema["properties"]["env"]
            .get("additionalProperties")
            .is_none()
    );
    let required = schema["required"].as_array().unwrap();
    assert!(required.iter().any(|k| k.as_str() == Some("env")));
}

#[test]
fn strict_mode_coerces_bare_objects_to_string() {
    // MCP free-form objects stay callable under strict mode: the schema is
    // coerced to string, and `decode_stringified_args` restores the object
    // before forwarding to the server.
    let mut schema = json!({
        "type": "object",
        "properties": { "payload": { "type": "object", "additionalProperties": true } },
        "required": ["payload"]
    });
    make_strict_schema(&mut schema);
    assert_eq!(schema["properties"]["payload"]["type"], "string");
    assert!(
        schema["properties"]["payload"]
            .get("additionalProperties")
            .is_none()
    );
}

// ── parse_env ─────────────────────────────────────────────────────

#[test]
fn parse_env_accepts_object() {
    let env = parse_env(&json!({ "FOO": "bar", "EMPTY": "" })).unwrap();
    assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
    assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
}

#[test]
fn parse_env_accepts_json_object_string() {
    let env = parse_env(&json!(r#"{"FOO":"bar"}"#)).unwrap();
    assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
}

#[test]
fn parse_env_treats_null_and_blank_string_as_no_override() {
    for raw in [json!(null), json!(""), json!("  "), json!("null")] {
        assert!(parse_env(&raw).unwrap().is_empty());
    }
}

#[test]
fn parse_env_rejects_invalid_strings() {
    for raw in ["not json", "[]", "42", r#""str""#] {
        assert!(parse_env(&json!(raw)).unwrap_err().contains("JSON-encoded"));
    }
}

#[test]
fn parse_env_rejects_non_object_and_non_string_values() {
    for raw in [json!(42), json!(true), json!([1, 2])] {
        assert!(parse_env(&raw).is_err());
    }
    let err = parse_env(&json!({ "FOO": 1 })).unwrap_err();
    assert!(err.contains("must be a string"));
}

// ── Process lifecycle (Unix) ──────────────────────────────────────

#[cfg(unix)]
#[test]
fn start_wait_and_logs() {
    let tmp = TempDir::new("run");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"echo hello; echo world; sleep 0.2\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let waited = execute(
        &tool,
        json!({"action": "wait", "process_id": id, "timeout": 15000}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );

    let logs = execute(
        &tool,
        json!({"action": "logs", "process_id": id}),
        &tmp.path,
    )
    .unwrap();
    assert!(logs.contains("hello"), "logs: {logs}");
    assert!(logs.contains("world"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn input_is_written_to_stdin() {
    let tmp = TempDir::new("input");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"read line; echo got:$line\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let sent = execute(
        &tool,
        json!({"action": "input", "process_id": id, "input": "hi there"}),
        &tmp.path,
    )
    .unwrap();
    assert!(sent.contains("bytes"));

    let waited = execute(
        &tool,
        json!({"action": "wait", "process_id": id, "timeout": 15000}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );

    let logs = execute(
        &tool,
        json!({"action": "logs", "process_id": id}),
        &tmp.path,
    )
    .unwrap();
    assert!(logs.contains("got:hi there"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn stop_terminates_process() {
    let tmp = TempDir::new("stop");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let status = execute(
        &tool,
        json!({"action": "status", "process_id": id}),
        &tmp.path,
    )
    .unwrap();
    assert!(status.contains("running"), "status: {status}");

    let stopped = execute(
        &tool,
        json!({"action": "stop", "process_id": id, "signal": "terminate"}),
        &tmp.path,
    )
    .unwrap();
    assert!(stopped.contains("stopped"), "stop result: {stopped}");
}

#[cfg(unix)]
#[test]
fn list_and_status_report_state() {
    let tmp = TempDir::new("list");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let list = execute(&tool, json!({"action": "list"}), &tmp.path).unwrap();
    assert!(list.contains(&id), "list: {list}");

    let status = execute(
        &tool,
        json!({"action": "status", "process_id": id}),
        &tmp.path,
    )
    .unwrap();
    assert!(status.contains("running"), "status: {status}");
    assert!(status.contains("os pid"), "status: {status}");

    // Clean up.
    let _ = execute(
        &tool,
        json!({"action": "stop", "process_id": id, "signal": "kill"}),
        &tmp.path,
    );
}

#[cfg(unix)]
#[test]
fn restart_replaces_process() {
    let tmp = TempDir::new("restart");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let restarted = execute(
        &tool,
        json!({"action": "restart", "process_id": id, "command": "/bin/sh -c \"echo again\""}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        restarted.contains("replacement started"),
        "restart result: {restarted}"
    );
    let new_id = restarted
        .split_whitespace()
        .find(|w| w.starts_with("proc-") && *w != id)
        .unwrap()
        .to_string();
    assert_ne!(id, new_id);

    let waited = execute(
        &tool,
        json!({"action": "wait", "process_id": new_id, "timeout": 15000}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );
}

#[cfg(unix)]
#[test]
fn logs_immediately_after_exit_includes_tail() {
    let tmp = TempDir::new("tail");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"echo early\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    // Read logs right away — before the reaper has necessarily recorded the
    // exit — and still see the full output (logs waits for the reader drain).
    let logs = execute(
        &tool,
        json!({"action": "logs", "process_id": id}),
        &tmp.path,
    )
    .unwrap();
    assert!(logs.contains("early"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn logs_follow_reports_exit_with_tail() {
    let tmp = TempDir::new("follow_exit");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"echo bye\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let logs = execute(
        &tool,
        json!({"action": "logs", "process_id": id, "follow": true, "timeout": 15000}),
        &tmp.path,
    )
    .unwrap();
    assert!(logs.contains("exited with code 0"), "logs: {logs}");
    assert!(logs.contains("bye"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn logs_follow_reports_still_running_on_timeout() {
    let tmp = TempDir::new("follow_timeout");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let logs = execute(
        &tool,
        json!({"action": "logs", "process_id": id, "follow": true, "timeout": 100}),
        &tmp.path,
    )
    .unwrap();
    assert!(logs.contains("still running after 100ms"), "logs: {logs}");

    let _ = execute(
        &tool,
        json!({"action": "stop", "process_id": id, "signal": "kill"}),
        &tmp.path,
    );
}

#[cfg(unix)]
#[test]
fn wait_completes_for_daemonising_child() {
    // The grandchild inherits the pipes and outlives its parent, so EOF never
    // arrives; wait must still report the exit. (The orphan exits on its own
    // a few seconds later.)
    let tmp = TempDir::new("daemon");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"(sleep 5 &); echo started; exit 7\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let waited = execute(
        &tool,
        json!({"action": "wait", "process_id": id, "timeout": 5000}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        waited.contains("exited with code 7"),
        "wait result: {waited}"
    );
}

#[cfg(unix)]
#[test]
fn logs_follow_streams_every_line_to_sink() {
    // The first line must reach the sink too (it used to be indistinguishable
    // from "nothing yet" and was dropped).
    let tmp = TempDir::new("follow_sink");
    let tool = ProcessTool;

    // Ample lead-in so the follow loop has definitely started before the
    // first line is emitted (the sink assertion fails if it misses "first").
    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 1; echo first; sleep 0.3; echo second\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: OutputSink = {
        let received = Arc::clone(&received);
        Arc::new(move |text: &str| received.lock().unwrap().push(text.to_string()))
    };
    let result = tool
        .execute_streaming(
            &json!({"action": "logs", "process_id": id, "follow": true, "timeout": 5000}),
            &tmp.path,
            &CancellationToken::new(),
            &sink,
        )
        .unwrap();
    assert!(result.contains("exited with code 0"), "result: {result}");
    let streamed = received.lock().unwrap().join("|");
    assert!(
        streamed.contains("first"),
        "sink never received the first line: {streamed}"
    );
    assert!(
        streamed.contains("second"),
        "sink never received the second line: {streamed}"
    );
}

#[cfg(unix)]
#[test]
fn stop_unknown_process_reports_error() {
    let tmp = TempDir::new("unknown");
    let tool = ProcessTool;

    let err = execute(
        &tool,
        json!({"action": "stop", "process_id": "proc-999999"}),
        &tmp.path,
    )
    .unwrap_err();
    assert!(err.contains("Unknown process_id"), "err: {err}");
}

#[cfg(unix)]
#[test]
fn restart_after_exit_starts_replacement() {
    let tmp = TempDir::new("restart_exit");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"echo first\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);
    let _ = execute(
        &tool,
        json!({"action": "wait", "process_id": id, "timeout": 15000}),
        &tmp.path,
    )
    .unwrap();

    let restarted = execute(
        &tool,
        json!({"action": "restart", "process_id": id, "command": "/bin/sh -c \"echo second\""}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        restarted.contains("had already exited"),
        "restart result: {restarted}"
    );
    let new_id = restarted
        .split_whitespace()
        .find(|w| w.starts_with("proc-") && *w != id)
        .unwrap()
        .to_string();

    let waited = execute(
        &tool,
        json!({"action": "wait", "process_id": new_id, "timeout": 15000}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );
    let logs = execute(
        &tool,
        json!({"action": "logs", "process_id": new_id}),
        &tmp.path,
    )
    .unwrap();
    assert!(logs.contains("second"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn restart_escalates_to_kill_when_terminate_ignored() {
    let tmp = TempDir::new("restart_kill");
    let tool = ProcessTool;

    // The shell ignores TERM and loops forever, so restart must escalate to kill.
    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"trap '' TERM; echo ready; while :; do sleep 1; done\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    // Wait for the trap to be installed, else TERM kills the shell outright.
    wait_for_log(&tool, &id, &tmp.path, "ready");

    let restarted = execute(
        &tool,
        json!({"action": "restart", "process_id": id}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        restarted.contains("kill after terminate ignored"),
        "restart result: {restarted}"
    );

    let new_id = restarted
        .split_whitespace()
        .find(|w| w.starts_with("proc-") && *w != id)
        .unwrap()
        .to_string();
    let _ = execute(
        &tool,
        json!({"action": "stop", "process_id": new_id, "signal": "kill"}),
        &tmp.path,
    );
}

#[cfg(unix)]
#[test]
fn restart_honors_cwd_and_env_overrides() {
    let tmp = TempDir::new("restart_overrides");
    std::fs::create_dir_all(tmp.path.join("sub")).unwrap();
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    // Override both cwd and env on restart; the replacement must use them.
    let restarted = execute(
        &tool,
        json!({
            "action": "restart",
            "process_id": id,
            "command": "/bin/sh -c 'echo cwd=$(pwd) env=$MY_VAR'",
            "cwd": "sub",
            "env": {"MY_VAR": "hello"}
        }),
        &tmp.path,
    )
    .unwrap();
    assert!(
        restarted.contains("replacement started"),
        "restart result: {restarted}"
    );
    let new_id = restarted
        .split_whitespace()
        .find(|w| w.starts_with("proc-") && *w != id)
        .unwrap()
        .to_string();

    let waited = execute(
        &tool,
        json!({"action": "wait", "process_id": new_id, "timeout": 15000}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );

    let logs = execute(
        &tool,
        json!({"action": "logs", "process_id": new_id}),
        &tmp.path,
    )
    .unwrap();
    assert!(logs.contains("cwd="), "logs: {logs}");
    assert!(logs.contains("env=hello"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn restart_inherits_env_when_unset_or_blank_and_clears_on_empty_object() {
    let tmp = TempDir::new("restart_env");
    let tool = ProcessTool;

    // Start with an env override that a strict-mode restart must preserve.
    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\"", "env": {"MY_VAR": "kept"}}),
        &tmp.path,
    )
    .unwrap();
    let mut id = process_id(&started);

    // Strict-mode schemas make `env` a required string, so "no override"
    // arrives as null or ""; both must inherit rather than wipe MY_VAR.
    for raw in [json!(null), json!("")] {
        let restarted = execute(
            &tool,
            json!({
                "action": "restart",
                "process_id": id,
                "command": "/bin/sh -c 'echo val=$MY_VAR'",
                "env": raw
            }),
            &tmp.path,
        )
        .unwrap();
        assert!(
            restarted.contains("replacement started"),
            "restart result: {restarted}"
        );
        id = restarted
            .split_whitespace()
            .find(|w| w.starts_with("proc-"))
            .unwrap()
            .to_string();
        let _ = execute(
            &tool,
            json!({ "action": "wait", "process_id": id, "timeout": 15000 }),
            &tmp.path,
        )
        .unwrap();
        let logs = execute(
            &tool,
            json!({ "action": "logs", "process_id": id }),
            &tmp.path,
        )
        .unwrap();
        assert!(
            logs.contains("val=kept"),
            "env lost on restart with env={raw}: {logs}"
        );
    }

    // An explicit empty object is a deliberate clear, distinct from "unset".
    let restarted = execute(
        &tool,
        json!({
            "action": "restart",
            "process_id": id,
            "command": "/bin/sh -c 'echo val=${MY_VAR:-unset}'",
            "env": {}
        }),
        &tmp.path,
    )
    .unwrap();
    id = restarted
        .split_whitespace()
        .find(|w| w.starts_with("proc-"))
        .unwrap()
        .to_string();
    let _ = execute(
        &tool,
        json!({ "action": "wait", "process_id": id, "timeout": 15000 }),
        &tmp.path,
    )
    .unwrap();
    let logs = execute(
        &tool,
        json!({ "action": "logs", "process_id": id }),
        &tmp.path,
    )
    .unwrap();
    assert!(
        logs.contains("val=unset"),
        "empty env did not clear: {logs}"
    );
}

#[cfg(unix)]
#[test]
fn restart_rejects_bad_command_without_stopping() {
    let tmp = TempDir::new("restart_bad");
    let tool = ProcessTool;

    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

    // A bad override must fail validation before the running process is stopped.
    let err = execute(
        &tool,
        json!({"action": "restart", "process_id": id, "command": "echo 'unterminated"}),
        &tmp.path,
    )
    .unwrap_err();
    assert!(err.contains("Failed to parse command"), "err: {err}");

    // Same for the empty override.
    let err = execute(
        &tool,
        json!({"action": "restart", "process_id": id, "command": ""}),
        &tmp.path,
    )
    .unwrap_err();
    assert!(err.contains("Empty command"), "err: {err}");

    let status = execute(
        &tool,
        json!({"action": "status", "process_id": id}),
        &tmp.path,
    )
    .unwrap();
    assert!(
        status.contains("running"),
        "original process was stopped: {status}"
    );

    let _ = execute(
        &tool,
        json!({"action": "stop", "process_id": id, "signal": "kill"}),
        &tmp.path,
    );
}
