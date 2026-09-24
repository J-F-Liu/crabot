mod common;

use std::path::Path;

use common::TempDir;
#[cfg(unix)]
use crabot::tools::OutputSink;
use crabot::tools::Tool;
use crabot::tools::make_strict_schema;
use crabot::tools::process::{ProcessLogs, ProcessTool, parse_env};
use serde_json::{Value, json};
#[cfg(unix)]
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Numeric tokens of a `start`/`restart` result, in order.
fn pids(result: &str) -> impl DoubleEndedIterator<Item = u32> + '_ {
    result
        .split_whitespace()
        .filter_map(|w| w.trim_end_matches(':').parse().ok())
}

fn pid(result: &str) -> u32 {
    pids(result)
        .next()
        .expect("start result should name the pid")
}

#[cfg(unix)]
fn last_pid(result: &str) -> u32 {
    pids(result)
        .next_back()
        .expect("restart result should name the new pid")
}

// ── TempDir-backed process wrappers ─────────────────────────────
// Each collapses the old "run tool → parse pid → wait/stop/logs" boilerplate.

/// Run the process tool in `ws`.
fn run(ws: &TempDir, args: Value) -> Result<String, String> {
    ProcessTool.execute(&args, &ws.path, &CancellationToken::new())
}

/// Start a process with a full args object (env/cwd overrides), returning its pid.
fn start_with(ws: &TempDir, args: Value) -> u32 {
    pid(&run(ws, args).unwrap())
}

/// Start `command` in `ws`, returning its pid.
#[cfg(unix)]
fn start(ws: &TempDir, command: &str) -> u32 {
    start_with(ws, json!({"action": "start", "command": command}))
}

/// Wait up to 15s for `pid`; returns the result text.
fn wait_exit(ws: &TempDir, pid: u32) -> String {
    run(ws, json!({"action": "wait", "pid": pid, "timeout": 15000})).unwrap()
}

/// Fetch the accumulated logs for `pid`.
fn logs(ws: &TempDir, pid: u32) -> String {
    run(ws, json!({"action": "logs", "pid": pid})).unwrap()
}

/// Stop `pid` with `signal` (`""` = the tool's default), discarding the result.
#[cfg(unix)]
fn stop(ws: &TempDir, pid: u32, signal: &str) {
    let mut args = json!({"action": "stop", "pid": pid});
    if !signal.is_empty() {
        args["signal"] = json!(signal);
    }
    let _ = run(ws, args);
}

/// Poll `logs` until it contains `needle`, panicking after ~2 s.
#[cfg(unix)]
fn wait_for_log(ws: &TempDir, id: u32, needle: &str) {
    for _ in 0..200 {
        if logs(ws, id).contains(needle) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("logs never contained {needle:?}");
}

#[cfg(unix)]
#[test]
fn input_cancel_unblocks_and_stop_still_works() {
    let tmp = TempDir::new("input_cancel").unwrap();
    let id = start(&tmp, "/bin/sh -c \"sleep 30\"");

    // `sleep 30` never reads stdin, so a large write fills the pipe and blocks.
    let cancel = CancellationToken::new();
    let cancel_for_thread = cancel.clone();
    let path = tmp.path.clone();
    let handle = std::thread::spawn(move || {
        let thread_tool = ProcessTool; // ZST: fresh handle for this thread
        thread_tool.execute(
            &json!({"action": "input", "pid": id, "input": "x".repeat(1024 * 1024)}),
            &path,
            &cancel_for_thread,
        )
    });
    std::thread::sleep(std::time::Duration::from_millis(300)); // let the write start blocking
    cancel.cancel();
    let result = handle.join().unwrap().unwrap_err();
    assert_eq!(result, "Cancelled by user", "input result: {result}");

    // The stdin mutex must be free: stop (which takes stdin) must not hang.
    let stopped = run(&tmp, json!({"action": "stop", "pid": id, "signal": "kill"})).unwrap();
    assert!(stopped.contains("stopped"), "stop result: {stopped}");
}

#[cfg(unix)]
#[test]
fn wait_completes_when_daemon_grandchild_keeps_writing() {
    let tmp = TempDir::new("daemon_write").unwrap();

    // The parent exits immediately; a grandchild keeps the pipe open and writes
    // continuously, so readers must stop after the drain grace (EOF never comes).
    let id = start(
        &tmp,
        "/bin/sh -c \"(i=0; while [ $i -lt 100000 ]; do echo noise; i=$((i+1)); done) & echo started; exit 7\"",
    );

    let waited = wait_exit(&tmp, id);
    assert!(
        waited.contains("exited with code 7"),
        "wait result: {waited}"
    );

    stop(&tmp, id, "kill");
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

    assert_eq!(schema["properties"]["pid"]["type"], "integer");
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
    let tmp = TempDir::new("run").unwrap();
    let id = start(&tmp, "/bin/sh -c \"echo hello; echo world; sleep 0.2\"");

    let waited = wait_exit(&tmp, id);
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );

    let logs = logs(&tmp, id);
    assert!(logs.contains("hello"), "logs: {logs}");
    assert!(logs.contains("world"), "logs: {logs}");
}

/// Reader output reaches `logs` as plain text: escapes stripped, tabs kept.
#[cfg(unix)]
#[test]
fn logs_strip_ansi_sequences() {
    let tmp = TempDir::new("ansi").unwrap();
    let id = start(
        &tmp,
        "/bin/sh -c \"printf '\\033[31mred\\033[0m\\ttext\\n'\"",
    );
    wait_exit(&tmp, id);

    let logs = logs(&tmp, id);
    assert!(logs.contains("red\ttext"), "logs: {logs:?}");
    assert!(!logs.contains('\u{1b}'), "logs kept an escape: {logs:?}");
}

// ── host command resolution ───────────────────────────────────────

/// A command behind a shell shim starts like a shell would run it: Windows
/// `.cmd`/`.bat` launchers (`npx` → `npx.cmd`, invisible to `CreateProcess`)
/// and extension-less executables on Unix.
#[test]
fn start_resolves_path_shim() {
    let tmp = TempDir::new("path_shim").unwrap();
    let bin = tmp.path.join("bin");
    std::fs::create_dir_all(&bin).unwrap();

    #[cfg(windows)]
    std::fs::write(
        bin.join("crabot-probe.cmd"),
        "@echo off\r\necho shim-ok\r\necho %PATH%\r\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let probe = bin.join("crabot-probe");
        std::fs::write(&probe, "#!/bin/sh\necho shim-ok\necho \"$PATH\"\n").unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // `PATH` as the tool's env override sees it (MSYS `/d/...` form on Windows).
    let vfs = crabot::tools::convert_path_to_unix_style(&bin);
    let id = start_with(
        &tmp,
        json!({"action": "start", "command": "crabot-probe", "env": {"PATH": vfs}}),
    );

    let waited = wait_exit(&tmp, id);
    assert!(waited.contains("exited with code 0"), "wait: {waited}");
    let logs = logs(&tmp, id);
    assert!(logs.contains("shim-ok"), "logs: {logs}");
    // The child sees the native list the lookup searched — a Windows child
    // cannot resolve anything through the MSYS `/d/...` override it was given.
    let native = bin.to_string_lossy().into_owned();
    assert!(logs.contains(&native), "logs: {logs}");
}

// ── registry change events ────────────────────────────────────────

/// A command that stays alive for ~5s on every platform.
#[cfg(unix)]
const SLEEP_CMD: &str = "sleep 5";
#[cfg(windows)]
const SLEEP_CMD: &str = "ping -n 6 127.0.0.1";

#[tokio::test]
async fn events_fire_on_start_and_exit() {
    use std::time::Duration;

    use futures::StreamExt;

    let cancel = CancellationToken::new();
    let mut events = crabot::tools::process::events();

    // The stream opens with a snapshot tick, before any process exists.
    assert!(
        tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .expect("no initial tick on subscribe")
            .is_some()
    );

    let result = ProcessTool
        .execute(
            &json!({"action": "start", "command": SLEEP_CMD}),
            Path::new("."),
            &cancel,
        )
        .unwrap();
    let pid = pid(&result);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), events.next())
            .await
            .expect("no registry tick after start")
            .is_some()
    );

    ProcessTool
        .execute(
            &json!({"action": "stop", "pid": pid}),
            Path::new("."),
            &cancel,
        )
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), events.next())
            .await
            .expect("no registry tick after exit")
            .is_some()
    );
}

/// The UI-facing snapshot lists running processes in start order, with the
/// owning tab captured from the tool-execution scope (`None` in the playground).
#[test]
fn running_processes_track_owner_and_lifecycle() {
    use std::time::Duration;

    use crabot::tools::{process, with_tab_scope};

    let cancel = CancellationToken::new();
    let find = |pid: u32| {
        process::running_processes()
            .into_iter()
            .find(|p| p.pid == pid)
    };

    // Started outside the LLM loop: no owning tab.
    let result = ProcessTool
        .execute(
            &json!({"action": "start", "command": SLEEP_CMD}),
            Path::new("."),
            &cancel,
        )
        .unwrap();
    let playground_pid = pid(&result);
    let entry = find(playground_pid).expect("started process listed");
    assert_eq!(entry.tab, None);

    // Started inside a tab scope: the entry carries the owning tab number.
    let result = with_tab_scope(7, || {
        ProcessTool.execute(
            &json!({"action": "start", "command": SLEEP_CMD}),
            Path::new("."),
            &cancel,
        )
    })
    .unwrap();
    let tab_pid = pid(&result);
    let entry = find(tab_pid).expect("started process listed");
    assert_eq!(entry.tab, Some(7));
    assert_eq!(entry.command, SLEEP_CMD);

    // Stopping a process removes it from the snapshot once the reaper records
    // the exit (which is also what pings the registry-change events).
    ProcessTool
        .execute(
            &json!({"action": "stop", "pid": tab_pid}),
            Path::new("."),
            &cancel,
        )
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while find(tab_pid).is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "process still listed after stop"
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    // Clean up the remaining playground process.
    ProcessTool
        .execute(
            &json!({"action": "stop", "pid": playground_pid}),
            Path::new("."),
            &cancel,
        )
        .unwrap();
}

#[cfg(unix)]
#[test]
fn input_is_written_to_stdin() {
    let tmp = TempDir::new("input").unwrap();
    let id = start(&tmp, "/bin/sh -c \"read line; echo got:$line\"");

    let sent = run(
        &tmp,
        json!({"action": "input", "pid": id, "input": "hi there"}),
    )
    .unwrap();
    assert!(sent.contains("bytes"));

    let waited = wait_exit(&tmp, id);
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );

    let logs = logs(&tmp, id);
    assert!(logs.contains("got:hi there"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn stop_terminates_process() {
    let tmp = TempDir::new("stop").unwrap();
    let id = start(&tmp, "/bin/sh -c \"sleep 30\"");

    let status = run(&tmp, json!({"action": "status", "pid": id})).unwrap();
    assert!(status.contains("running"), "status: {status}");

    let stopped = run(
        &tmp,
        json!({"action": "stop", "pid": id, "signal": "terminate"}),
    )
    .unwrap();
    assert!(stopped.contains("stopped"), "stop result: {stopped}");
}

#[cfg(unix)]
#[test]
fn list_and_status_report_state() {
    let tmp = TempDir::new("list").unwrap();
    let id = start(&tmp, "/bin/sh -c \"sleep 30\"");

    let list = run(&tmp, json!({"action": "list"})).unwrap();
    assert!(list.contains(&id.to_string()), "list: {list}");

    let status = run(&tmp, json!({"action": "status", "pid": id})).unwrap();
    assert!(status.contains("running"), "status: {status}");
    assert!(status.contains("pid:"), "status: {status}");

    stop(&tmp, id, "kill");
}

#[cfg(unix)]
#[test]
fn restart_replaces_process() {
    let tmp = TempDir::new("restart").unwrap();
    let id = start(&tmp, "/bin/sh -c \"sleep 30\"");

    let restarted = run(
        &tmp,
        json!({"action": "restart", "pid": id, "command": "/bin/sh -c \"echo again\""}),
    )
    .unwrap();
    assert!(
        restarted.contains("replacement started"),
        "restart result: {restarted}"
    );
    let new_id = last_pid(&restarted);

    let waited = wait_exit(&tmp, new_id);
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );
}

#[cfg(unix)]
#[test]
fn logs_immediately_after_exit_includes_tail() {
    let tmp = TempDir::new("tail").unwrap();
    let id = start(&tmp, "/bin/sh -c \"echo early\"");

    // Read logs right away — before the reaper has necessarily recorded the
    // exit — and still see the full output (logs waits for the reader drain).
    let logs = logs(&tmp, id);
    assert!(logs.contains("early"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn logs_follow_reports_exit_with_tail() {
    let tmp = TempDir::new("follow_exit").unwrap();
    let id = start(&tmp, "/bin/sh -c \"echo bye\"");

    let logs = run(
        &tmp,
        json!({"action": "logs", "pid": id, "follow": true, "timeout": 15000}),
    )
    .unwrap();
    assert!(logs.contains("exited with code 0"), "logs: {logs}");
    assert!(logs.contains("bye"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn logs_follow_reports_still_running_on_timeout() {
    let tmp = TempDir::new("follow_timeout").unwrap();
    let id = start(&tmp, "/bin/sh -c \"sleep 30\"");

    let logs = run(
        &tmp,
        json!({"action": "logs", "pid": id, "follow": true, "timeout": 100}),
    )
    .unwrap();
    assert!(logs.contains("still running after 100ms"), "logs: {logs}");

    stop(&tmp, id, "kill");
}

#[cfg(unix)]
#[test]
fn wait_completes_for_daemonising_child() {
    // The grandchild inherits the pipes and outlives its parent, so EOF never
    // arrives; wait must still report the exit. (The orphan exits on its own
    // a few seconds later.)
    let tmp = TempDir::new("daemon").unwrap();
    let id = start(&tmp, "/bin/sh -c \"(sleep 5 &); echo started; exit 7\"");

    let waited = run(&tmp, json!({"action": "wait", "pid": id, "timeout": 5000})).unwrap();
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
    let tmp = TempDir::new("follow_sink").unwrap();

    // Ample lead-in so the follow loop has definitely started before the
    // first line is emitted (the sink assertion fails if it misses "first").
    let id = start(
        &tmp,
        "/bin/sh -c \"sleep 1; echo first; sleep 0.3; echo second\"",
    );

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: OutputSink = {
        let received = Arc::clone(&received);
        Arc::new(move |text: &str| received.lock().unwrap().push(text.to_string()))
    };
    let result = ProcessTool
        .execute_streaming(
            &json!({"action": "logs", "pid": id, "follow": true, "timeout": 5000}),
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
    let tmp = TempDir::new("unknown").unwrap();

    let err = run(&tmp, json!({"action": "stop", "pid": 999999})).unwrap_err();
    assert!(err.contains("Unknown pid"), "err: {err}");
}

#[cfg(unix)]
#[test]
fn restart_after_exit_starts_replacement() {
    let tmp = TempDir::new("restart_exit").unwrap();
    let id = start(&tmp, "/bin/sh -c \"echo first\"");
    wait_exit(&tmp, id);

    let restarted = run(
        &tmp,
        json!({"action": "restart", "pid": id, "command": "/bin/sh -c \"echo second\""}),
    )
    .unwrap();
    assert!(
        restarted.contains("had already exited"),
        "restart result: {restarted}"
    );
    let new_id = last_pid(&restarted);

    let waited = wait_exit(&tmp, new_id);
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );
    let logs = logs(&tmp, new_id);
    assert!(logs.contains("second"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn restart_escalates_to_kill_when_terminate_ignored() {
    let tmp = TempDir::new("restart_kill").unwrap();

    // The shell ignores TERM and loops forever, so restart must escalate to kill.
    let id = start(
        &tmp,
        "/bin/sh -c \"trap '' TERM; echo ready; while :; do sleep 1; done\"",
    );

    // Wait for the trap to be installed, else TERM kills the shell outright.
    wait_for_log(&tmp, id, "ready");

    let restarted = run(&tmp, json!({"action": "restart", "pid": id})).unwrap();
    assert!(
        restarted.contains("kill after terminate ignored"),
        "restart result: {restarted}"
    );

    let new_id = last_pid(&restarted);
    stop(&tmp, new_id, "kill");
}

#[cfg(unix)]
#[test]
fn restart_honors_cwd_and_env_overrides() {
    let tmp = TempDir::new("restart_overrides").unwrap();
    tmp.mkdir("sub").unwrap();
    let id = start(&tmp, "/bin/sh -c \"sleep 30\"");

    // Override both cwd and env on restart; the replacement must use them.
    let restarted = run(
        &tmp,
        json!({
            "action": "restart",
            "pid": id,
            "command": "/bin/sh -c 'echo cwd=$(pwd) env=$MY_VAR'",
            "cwd": "sub",
            "env": {"MY_VAR": "hello"}
        }),
    )
    .unwrap();
    assert!(
        restarted.contains("replacement started"),
        "restart result: {restarted}"
    );
    let new_id = last_pid(&restarted);

    let waited = wait_exit(&tmp, new_id);
    assert!(
        waited.contains("exited with code 0"),
        "wait result: {waited}"
    );

    let logs = logs(&tmp, new_id);
    assert!(logs.contains("cwd="), "logs: {logs}");
    assert!(logs.contains("env=hello"), "logs: {logs}");
}

#[cfg(unix)]
#[test]
fn restart_inherits_env_when_unset_or_blank_and_clears_on_empty_object() {
    let tmp = TempDir::new("restart_env").unwrap();

    // Start with an env override that a strict-mode restart must preserve.
    let mut id = start_with(
        &tmp,
        json!({"action": "start", "command": "/bin/sh -c \"sleep 30\"", "env": {"MY_VAR": "kept"}}),
    );

    // Strict-mode schemas make `env` a required string, so "no override"
    // arrives as null, "", or "null"; all inherit rather than wipe MY_VAR.
    for raw in [json!(null), json!(""), json!("null")] {
        let restarted = run(
            &tmp,
            json!({
                "action": "restart",
                "pid": id,
                "command": "/bin/sh -c 'echo val=$MY_VAR'",
                "env": raw
            }),
        )
        .unwrap();
        assert!(
            restarted.contains("replacement started"),
            "restart result: {restarted}"
        );
        id = last_pid(&restarted);
        wait_exit(&tmp, id);
        let logs = logs(&tmp, id);
        assert!(
            logs.contains("val=kept"),
            "env lost on restart with env={raw}: {logs}"
        );
    }

    // An explicit empty object is a deliberate clear, distinct from "unset".
    let restarted = run(
        &tmp,
        json!({
            "action": "restart",
            "pid": id,
            "command": "/bin/sh -c 'echo val=${MY_VAR:-unset}'",
            "env": {}
        }),
    )
    .unwrap();
    id = last_pid(&restarted);
    wait_exit(&tmp, id);
    let logs = logs(&tmp, id);
    assert!(
        logs.contains("val=unset"),
        "empty env did not clear: {logs}"
    );
}

#[cfg(unix)]
#[test]
fn restart_rejects_bad_command_without_stopping() {
    let tmp = TempDir::new("restart_bad").unwrap();
    let id = start(&tmp, "/bin/sh -c \"sleep 30\"");

    // A bad override must fail validation before the running process is stopped.
    let err = run(
        &tmp,
        json!({"action": "restart", "pid": id, "command": "echo 'unterminated"}),
    )
    .unwrap_err();
    assert!(err.contains("Failed to parse command"), "err: {err}");

    // Same for the empty override.
    let err = run(&tmp, json!({"action": "restart", "pid": id, "command": ""})).unwrap_err();
    assert!(err.contains("Empty command"), "err: {err}");

    let status = run(&tmp, json!({"action": "status", "pid": id})).unwrap();
    assert!(
        status.contains("running"),
        "original process was stopped: {status}"
    );

    stop(&tmp, id, "kill");
}
