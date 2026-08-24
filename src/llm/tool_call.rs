//! Tool-call execution: parallel batches with serial barriers, live-output
//! forwarding, and the builtin `ask` tool interception.

use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use genai::chat::{ToolCall, ToolResponse};

use crate::app::session_state::{ASK_TIMEOUT_SECS, AskRequest, SessionEvent};
use crate::tools::{self, ToolRef};
use crabot::chat::{ToolResult as ChatToolResult, envelope_error};
use crabot::lock;

// ── Shared execution context ───────────────────────────────────────

/// Shared state for tool execution.
pub(super) struct ExecutionCtx<'a> {
    pub(super) tools: &'a [ToolRef],
    pub(super) workspace: &'a std::path::Path,
    pub(super) cancel_token: &'a CancellationToken,
    /// Session tab number, tagged onto global state tools create (e.g. process entries).
    pub(super) tab_number: usize,
    /// Shared ask-tool deadline — the UI extends it to give more time.
    pub(super) ask_deadline: &'a Arc<Mutex<Instant>>,
}

// ── Tool lookup, logging, and execution ────────────────────────────

/// Tools that must run serially (interactive or state-modifying).
pub(super) fn is_serial_tool(name: &str) -> bool {
    matches!(
        name,
        "ask" | "renew" | "write" | "edit" | "bash" | "process"
    )
}

/// Look up a registered tool by name.
fn find_tool(tools: &[ToolRef], name: &str) -> Option<ToolRef> {
    tools.iter().find(|t| t.name() == name).cloned()
}

/// Join a spawned tool task, mapping panics to an error result.
async fn await_tool(
    handle: tokio::task::JoinHandle<Result<String, String>>,
) -> Result<String, String> {
    handle
        .await
        .unwrap_or_else(|e| Err(format!("Tool execution panicked: {e}")))
}

/// Log a finished tool execution: name, elapsed time, and outcome.
fn log_tool_outcome(name: &str, result: &Result<String, String>, start: Instant) {
    let elapsed_ms = start.elapsed().as_millis() as u64;
    match result {
        Ok(_) => tracing::info!(tool = name, elapsed_ms, "tool finished"),
        Err(e) => tracing::warn!(tool = name, elapsed_ms, "tool failed: {e}"),
    }
}

/// Execute a tool on a blocking thread under the session-tab scope; a missing
/// tool yields an error result.
async fn call_tool(
    tool: Option<ToolRef>,
    tc: &ToolCall,
    workspace: std::path::PathBuf,
    cancel_token: CancellationToken,
    tab_number: usize,
) -> Result<String, String> {
    let name = tc.fn_name.clone();
    let start = Instant::now();
    let result = match tool {
        Some(t) => {
            let fn_arguments = tc.fn_arguments.clone();
            await_tool(tokio::task::spawn_blocking(move || {
                tools::with_tab_scope(tab_number, || {
                    t.execute(&fn_arguments, &workspace, &cancel_token)
                })
            }))
            .await
        }
        None => Err(tools::unknown_tool_message(&tc.fn_name)),
    };
    log_tool_outcome(&name, &result, start);
    result
}

/// Execute a tool, forwarding its live output as [`SessionEvent::ToolOutput`],
/// coalesced and capped at `max_output_bytes` so noisy tools can't flood the UI.
async fn call_tool_streaming(
    tool: Option<ToolRef>,
    tc: &ToolCall,
    workspace: std::path::PathBuf,
    cancel_token: CancellationToken,
    tab_number: usize,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> Result<String, String> {
    let Some(tool) = tool else {
        return Err(tools::unknown_tool_message(&tc.fn_name));
    };
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let sink = tools::capping_sink(move |out| {
        let _ = chunk_tx.send(out);
    });
    let fn_arguments = tc.fn_arguments.clone();
    let call_id = tc.call_id.clone();
    let name = tc.fn_name.clone();
    let start = Instant::now();
    let handle = tokio::task::spawn_blocking(move || {
        tools::with_tab_scope(tab_number, || {
            tool.execute_streaming(&fn_arguments, &workspace, &cancel_token, &sink)
        })
    });

    // Coalesce chunks into batches, flushing on size or age.
    let mut forwarding = true;
    let mut pending = String::new();
    let mut last_flush = Instant::now();
    loop {
        let chunk = if pending.is_empty() {
            chunk_rx.recv().await
        } else {
            tokio::select! {
                chunk = chunk_rx.recv() => chunk,
                _ = tokio::time::sleep_until((last_flush + tools::COALESCE_MS).into()) => None,
            }
        };
        match chunk {
            Some(chunk) => {
                pending.push_str(&chunk);
                if pending.len() < tools::COALESCE_BYTES {
                    continue;
                }
            }
            None if pending.is_empty() => break, // tool finished
            None => {}                           // batch aged out: flush
        }
        if forwarding {
            forwarding = on_event(SessionEvent::ToolOutput {
                call_id: Some(call_id.clone()),
                chunk: std::mem::take(&mut pending),
            })
            .await;
        } else {
            pending.clear();
        }
        last_flush = Instant::now();
    }

    let result = await_tool(handle).await;
    log_tool_outcome(&name, &result, start);
    result
}

/// Build genai `ToolResponse` and UI `ChatToolResult` from an execution result.
fn build_tool_result(
    tc: &ToolCall,
    result: Result<String, String>,
) -> (ToolResponse, ChatToolResult) {
    let result_flat = match &result {
        Ok(s) => s.clone(),
        Err(e) => envelope_error(e),
    };
    let response = ToolResponse {
        call_id: tc.call_id.clone(),
        fn_name: Some(tc.fn_name.clone()),
        content: result_flat,
    };
    let result = ChatToolResult {
        name: tc.fn_name.clone(),
        call_id: Some(tc.call_id.clone()),
        args: tc.fn_arguments.clone(),
        result,
        timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        streaming: false,
    };
    (response, result)
}

// ── Parallel batches and serial barriers ───────────────────────────

/// Run a batch of parallel tool calls, emitting results in completion order.
/// Task calls are emitted up front so all sub-agent tabs spawn before any waits;
/// tagged reports route from `task_receiver` to their oneshot (genai matches by call_id).
pub(super) async fn run_parallel_batch(
    batch: &mut Vec<&ToolCall>,
    ctx: &ExecutionCtx<'_>,
    task_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Result<String, String>)>,
    tool_responses: &mut Vec<ToolResponse>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) {
    if batch.is_empty() {
        return;
    }

    // Fast path: a single non-task tool needs no parallel machinery.
    if batch.len() == 1 && batch[0].fn_name != "task" {
        let tc = batch.pop().unwrap();
        let cancel = ctx.cancel_token.clone();
        let tool = find_tool(ctx.tools, &tc.fn_name);
        let workspace = ctx.workspace.to_path_buf();
        let result = call_tool(tool, tc, workspace, cancel, ctx.tab_number).await;
        let (response, result) = build_tool_result(tc, result);
        tool_responses.push(response);
        on_event(SessionEvent::ToolResult(result)).await;
        return;
    }

    let mut futures = FuturesUnordered::new();
    let mut task_waiters: HashMap<String, tokio::sync::oneshot::Sender<Result<String, String>>> =
        HashMap::new();
    for tc in batch.drain(..) {
        let cancel = ctx.cancel_token.clone();

        let future: BoxFuture<'static, (ToolResponse, ChatToolResult)> = if tc.fn_name == "task" {
            let request = tools::task_request_from_call(&tc.call_id, &tc.fn_arguments);
            let (tx, rx) = tokio::sync::oneshot::channel();
            if request.prompt.trim().is_empty() {
                let _ = tx.send(Err(
                    "Task called with an empty prompt — no subtask spawned.".into(),
                ));
            } else {
                // Emit up front so every sub-agent tab spawns before any future waits.
                if !on_event(SessionEvent::TaskRequest(request)).await {
                    // The UI can't spawn the subtask — fail it instead of hanging forever.
                    let _ = tx.send(Err("Task request event channel closed.".into()));
                } else {
                    task_waiters.insert(tc.call_id.clone(), tx);
                }
            }
            let tc = tc.clone();
            Box::pin(async move {
                let result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(crate::tools::CANCEL_REASON.into()),
                    result = rx => result.unwrap_or_else(|_| Err("Task response channel closed.".into())),
                };
                build_tool_result(&tc, result)
            })
        } else {
            let tab_number = ctx.tab_number;
            let tool = find_tool(ctx.tools, &tc.fn_name);
            let workspace = ctx.workspace.to_path_buf();
            let tc = tc.clone();
            Box::pin(async move {
                let result = call_tool(tool, &tc, workspace, cancel, tab_number).await;
                build_tool_result(&tc, result)
            })
        };
        futures.push(future);
    }

    // Tool futures finish on their own; task futures via the routed report below.
    let mut receiver_open = true;
    loop {
        tokio::select! {
            item = futures.next() => {
                let Some((response, result)) = item else { break };
                tool_responses.push(response);
                on_event(SessionEvent::ToolResult(result)).await;
            }
            report = task_receiver.recv(), if receiver_open => match report {
                Some((id, result)) => {
                    if let Some(waiter) = task_waiters.remove(&id) {
                        let _ = waiter.send(result);
                    }
                }
                // Channel closed — fail any task futures still waiting.
                None => {
                    receiver_open = false;
                    task_waiters.clear();
                }
            },
        }
    }
}

/// Run one serial tool (ask/renew/write/edit/bash/process) and emit its result;
/// failures are recorded as tool results rather than aborting the run.
pub(super) async fn run_serial_tool(
    tc: &ToolCall,
    ctx: &ExecutionCtx<'_>,
    ask_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    renew_executed: &mut bool,
    tool_responses: &mut Vec<ToolResponse>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) {
    let start = Instant::now();
    let result = match tc.fn_name.as_str() {
        "ask" => handle_ask_tool(tc, ask_receiver, ctx, on_event).await,
        "renew" => {
            let prompt = tc
                .fn_arguments
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if prompt.is_empty() {
                Err("Renew called with an empty prompt — no new session created.".into())
            } else if *renew_executed {
                Err("Only the first 'renew' call is effective.".into())
            } else if !on_event(SessionEvent::RenewRequest(prompt.into())).await {
                Err("Renew event channel closed.".into())
            } else {
                *renew_executed = true;
                Ok("New session created with the provided prompt.".into())
            }
        }
        _ => {
            let tool = find_tool(ctx.tools, &tc.fn_name);
            let workspace = ctx.workspace.to_path_buf();
            let cancel = ctx.cancel_token.clone();
            // bash and process stream their output live like LLM text chunks.
            if matches!(tc.fn_name.as_str(), "bash" | "process") {
                call_tool_streaming(tool, tc, workspace, cancel, ctx.tab_number, on_event).await
            } else {
                call_tool(tool, tc, workspace, cancel, ctx.tab_number).await
            }
        }
    };
    if tc.fn_name == "ask" || tc.fn_name == "renew" {
        // ask/renew don't pass through `call_tool`, so log their outcome here.
        let name = tc.fn_name.clone();
        log_tool_outcome(&name, &result, start);
    }
    let (response, result) = build_tool_result(tc, result);
    tool_responses.push(response);
    on_event(SessionEvent::ToolResult(result)).await;
}

// ── Builtin ask tool ───────────────────────────────────────────────

/// Error for a failed event delivery: user Stop vs. a dead UI channel.
fn event_send_error(cancel_token: &CancellationToken) -> String {
    if cancel_token.is_cancelled() {
        "Session cancelled by user.".into()
    } else {
        "Ask event channel closed.".into()
    }
}

/// Drain stale results, emit `event`, then wait for the response, cancellation,
/// or the shared `ask_deadline` timeout — ticking `AskCountdown` each second so
/// the UI can extend the deadline.
async fn wait_for_result(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
    cancel_token: &CancellationToken,
    event: SessionEvent,
    ask_deadline: &Arc<Mutex<Instant>>,
    timeout: (Duration, &'static str),
) -> Result<String, String> {
    let (timeout_dur, timeout_msg) = timeout;
    while receiver.try_recv().is_ok() {} // drain a stale result from a previous wait
    if !on_event(event).await {
        return Err(event_send_error(cancel_token));
    }
    // Anchor the deadline now; the UI pushes it later via `ask_deadline`.
    *lock(ask_deadline) = Instant::now() + timeout_dur;
    loop {
        // Re-read the shared deadline so UI extensions take effect immediately.
        let remaining = lock(ask_deadline).saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(timeout_msg.into());
        }
        if !on_event(SessionEvent::AskCountdown(remaining.as_secs())).await {
            return Err(event_send_error(cancel_token));
        }
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => return Err(crate::tools::CANCEL_REASON.into()),
            result = receiver.recv() => return result.unwrap_or_else(|| Err("Response channel closed.".into())),
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

/// Handle a builtin ask-tool call: parse arguments, emit the question to the
/// UI, then wait for user response, cancellation, or `ASK_TIMEOUT_SECS`.
async fn handle_ask_tool(
    tc: &ToolCall,
    ask_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    ctx: &ExecutionCtx<'_>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> Result<String, String> {
    let question = tc
        .fn_arguments
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let options: Vec<String> = tc
        .fn_arguments
        .get("options")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    wait_for_result(
        ask_receiver,
        on_event,
        ctx.cancel_token,
        SessionEvent::AskRequest(AskRequest { question, options }),
        ctx.ask_deadline,
        (
            Duration::from_secs(ASK_TIMEOUT_SECS),
            "User did not respond before the timeout.",
        ),
    )
    .await
}
