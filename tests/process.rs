#[cfg(unix)]
use std::path::PathBuf;

use crabot::tools::Tool;
use crabot::tools::process::{ProcessLogs, ProcessTool};
#[cfg(unix)]
use serde_json::{Value, json};
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

    // The shell ignores TERM and its background child inherits the ignored
    // disposition, so a graceful terminate cannot stop it; restart must
    // escalate to kill.
    let started = execute(
        &tool,
        json!({"action": "start", "command": "/bin/sh -c \"trap '' TERM; sleep 30 & wait\""}),
        &tmp.path,
    )
    .unwrap();
    let id = process_id(&started);

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
