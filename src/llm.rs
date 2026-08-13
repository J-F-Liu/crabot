use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use genai::adapter::AdapterKind;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStream, ChatStreamEvent,
    MessageContent, ReasoningEffort, ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use reqwest::StatusCode;

use crate::app::session_state::{ASK_TIMEOUT_SECS, AskRequest, RetryInfo, SessionEvent};
use crate::tools::{self, ToolRef};
use crabot::chat::{ToolCall as ChatToolCall, ToolResult as ChatToolResult, envelope_error};
use crabot::model::ModelInfo;
use crabot::user::UserPrompt;

/// Seconds to wait between auto-retry attempts after a transient failure.
const RETRY_DELAY_SECS: u32 = 60;
/// Total number of connection attempts (initial request + retries).
const MAX_ATTEMPTS: u32 = 5;

// ── DialogPhase: tracks the current phase of an LLM interaction ────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogPhase {
    Idle,
    /// Establishing connection / sending request to the LLM server.
    LlmLoading,
    /// LLM is actively thinking / generating the response.
    LlmThinking,
    /// Locally executing a tool call.
    ToolExecuting,
}

/// Move the rolling ephemeral cache breakpoint to the tail message.
/// Only touches `CacheControl::Ephemeral`; leaves other TTLs (e.g. `Ephemeral1h`) intact.
fn mark_cache_tail(messages: &mut [ChatMessage]) {
    // Find and remove the most recent rolling ephemeral breakpoint (if any).
    if let Some(msg) = messages.iter_mut().rev().find(|msg| {
        msg.options.as_ref().and_then(|o| o.cache_control.as_ref())
            == Some(&CacheControl::Ephemeral)
    }) {
        msg.options.as_mut().unwrap().cache_control = None;
    }
    // Set the rolling breakpoint on the tail message.
    if let Some(last) = messages.last_mut() {
        last.options.get_or_insert_default().cache_control = Some(CacheControl::Ephemeral);
    }
}

/// A successfully established LLM stream plus its first event (if any).
struct AcquiredStream {
    stream: ChatStream,
    /// First event already pulled from the stream, if any.
    first: Option<ChatStreamEvent>,
}

/// Where a failed acquisition originated — used to label the error message.
enum AcquireStage {
    /// `exec_chat_stream` failed while setting up the request.
    Setup,
    /// The first stream poll failed (HTTP status / connection error).
    FirstPoll,
}

/// Whether an HTTP status warrants an auto-retry (429 rate limit / 5xx server error).
fn is_retryable_status(status: StatusCode) -> bool {
    status.as_u16() == 429 || (500..600).contains(&status.as_u16())
}

/// Whether a reqwest error is a connection-level failure worth retrying.
/// Note: reqwest classifies TLS handshake failures as connect-phase errors, so
/// they are retried too — harmless, since attempts are bounded by `MAX_ATTEMPTS`.
fn is_retryable_reqwest(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout()
}

/// Classify a genai error as transient (429 / 5xx / connection failure).
/// Only pre-first-event failures are retried; mid-stream errors surface as-is.
/// Statuses buried in `ChatResponseGeneration`/`ChatResponse` bodies aren't classified.
fn is_retryable(e: &genai::Error) -> bool {
    match e {
        genai::Error::HttpError { status, .. } => is_retryable_status(*status),
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => match webc_error {
            genai::webc::Error::ResponseFailedStatus { status, .. } => is_retryable_status(*status),
            genai::webc::Error::Reqwest(re) => is_retryable_reqwest(re),
            _ => false,
        },
        genai::Error::WebStream { error, .. } => {
            // Downcast to a nested genai error or a raw reqwest error.
            if let Some(genai_err) = error.downcast_ref::<genai::Error>() {
                return is_retryable(genai_err);
            }
            if let Some(reqwest_err) = error.downcast_ref::<reqwest::Error>() {
                return is_retryable_reqwest(reqwest_err);
            }
            false
        }
        _ => false,
    }
}

/// Establish the stream, racing the request and first poll against cancellation.
/// Returns `Ok(None)` if cancelled, `Err((stage, e))` on failure.
async fn try_acquire_stream(
    client: &Client,
    model: &crabot::model::ModelInfo,
    chat_req: &ChatRequest,
    chat_options: &ChatOptions,
    cancel_token: &CancellationToken,
) -> Result<Option<AcquiredStream>, (AcquireStage, genai::Error)> {
    let stream_result = tokio::select! {
        biased;
        _ = cancel_token.cancelled() => return Ok(None),
        res = client.exec_chat_stream(&model.model_id, chat_req.clone(), Some(chat_options)) => res,
    };
    let mut stream = match stream_result {
        Ok(chat_res) => chat_res.stream,
        Err(e) => return Err((AcquireStage::Setup, e)),
    };

    // SSE adapters emit a synthetic Start before the request is sent; HTTP
    // errors (429/5xx/connect) surface on the poll after it. Pull past Start
    // so the first real event (or error) decides acquisition success.
    let first = loop {
        let ev = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => return Ok(None),
            ev = stream.next() => ev,
        };
        match ev {
            Some(Ok(ChatStreamEvent::Start)) => continue,
            other => break other,
        }
    };
    Ok(Some(AcquiredStream {
        stream,
        first: first
            .transpose()
            .map_err(|e| (AcquireStage::FirstPoll, e))?,
    }))
}

/// Establish the stream, auto-retrying transient failures (429/5xx/connect)
/// with a per-second countdown. Returns `Ok(None)` if cancelled, `Err(msg)` on failure.
async fn acquire_stream_with_retry(
    client: &Client,
    model: &crabot::model::ModelInfo,
    chat_req: &ChatRequest,
    chat_options: &ChatOptions,
    cancel_token: &CancellationToken,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> Result<Option<AcquiredStream>, String> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match try_acquire_stream(client, model, chat_req, chat_options, cancel_token).await {
            Ok(Some(acquired)) => return Ok(Some(acquired)),
            Ok(None) => return Ok(None),
            Err((_, e)) if attempt < MAX_ATTEMPTS && is_retryable(&e) => {
                // Count down one second at a time, keeping Stop responsive.
                for seconds_left in (1..=RETRY_DELAY_SECS).rev() {
                    if !on_event(SessionEvent::RetryCountdown(RetryInfo {
                        attempt: attempt + 1,
                        max_attempts: MAX_ATTEMPTS,
                        seconds_left,
                    }))
                    .await
                    {
                        return Ok(None);
                    }
                    tokio::select! {
                        biased;
                        _ = cancel_token.cancelled() => return Ok(None),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                }
                // Countdown finished — clear the stale countdown status before the next attempt.
                if !on_event(SessionEvent::PhaseChange(DialogPhase::LlmLoading)).await {
                    return Ok(None);
                }
            }
            Err((stage, e)) => {
                // Report where the failure surfaced and how many attempts ran.
                let message = match stage {
                    AcquireStage::Setup => "Failed to start the LLM request",
                    AcquireStage::FirstPoll => "The LLM request failed",
                };
                let attempts = if attempt == 1 {
                    "1 attempt".to_string()
                } else {
                    format!("{attempt} attempts")
                };
                return Err(format!("{message} after {attempts}: {e}"));
            }
        }
    }
}

/// Tools that must run serially (interactive or state-modifying).
fn is_serial_tool(name: &str) -> bool {
    matches!(name, "ask" | "renew" | "write" | "edit" | "bash")
}

/// Look up a registered tool by name.
fn find_tool(tools: &[ToolRef], name: &str) -> Option<ToolRef> {
    tools.iter().find(|t| t.name() == name).cloned()
}

/// Lock a mutex, recovering from poisoning (a panicked holder).
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Join a spawned tool task, mapping panics to an error result.
async fn await_tool(
    handle: tokio::task::JoinHandle<Result<String, String>>,
) -> Result<String, String> {
    handle
        .await
        .unwrap_or_else(|e| Err(format!("Tool execution panicked: {e}")))
}

/// Execute a tool on a blocking thread; unknown tools report an error result.
async fn exec_tool(
    tool: Option<ToolRef>,
    tc: &ToolCall,
    workspace: std::path::PathBuf,
    cancel_token: CancellationToken,
) -> Result<String, String> {
    match tool {
        Some(t) => {
            let fn_arguments = tc.fn_arguments.clone();
            await_tool(tokio::task::spawn_blocking(move || {
                t.execute(&fn_arguments, &workspace, &cancel_token)
            }))
            .await
        }
        None => Err(tools::unknown_tool_message(&tc.fn_name)),
    }
}

/// Execute a tool while forwarding its live output chunks as
/// [`SessionEvent::ToolOutput`]. If the UI stops accepting events, chunks are
/// drained silently but the tool still runs to completion, so its final result
/// always replaces the streaming placeholder.
async fn exec_tool_streaming(
    tool: Option<ToolRef>,
    tc: &ToolCall,
    workspace: std::path::PathBuf,
    cancel_token: CancellationToken,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> Result<String, String> {
    let Some(tool) = tool else {
        return Err(tools::unknown_tool_message(&tc.fn_name));
    };
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let sink: tools::OutputSink = Arc::new(move |chunk| {
        let _ = chunk_tx.send(chunk.to_string());
    });
    let fn_arguments = tc.fn_arguments.clone();
    let call_id = tc.call_id.clone();
    let handle = tokio::task::spawn_blocking(move || {
        tool.execute_streaming(&fn_arguments, &workspace, &cancel_token, &sink)
    });

    // Forward chunks until the tool finishes (its sink drop closes the channel).
    let mut forwarding = true;
    while let Some(chunk) = chunk_rx.recv().await {
        if forwarding {
            forwarding = on_event(SessionEvent::ToolOutput {
                call_id: Some(call_id.clone()),
                chunk,
            })
            .await;
        }
    }

    await_tool(handle).await
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

/// Shared state for tool execution.
struct ExecutionCtx<'a> {
    tools: &'a [ToolRef],
    workspace: &'a std::path::Path,
    cancel_token: &'a CancellationToken,
    /// Shared ask-tool deadline — the UI extends it to give more time.
    ask_deadline: &'a Arc<Mutex<Instant>>,
}

/// Run a batch of parallel tool calls, emitting results in completion order.
/// Task calls are emitted up front so all sub-agent tabs spawn before any waits;
/// tagged reports route from `task_receiver` to their oneshot (genai matches by call_id).
async fn run_parallel_batch(
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
        let result = exec_tool(tool, tc, workspace, cancel).await;
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
            let tool = find_tool(ctx.tools, &tc.fn_name);
            let workspace = ctx.workspace.to_path_buf();
            let tc = tc.clone();
            Box::pin(async move {
                let result = exec_tool(tool, &tc, workspace, cancel).await;
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

/// Run one serial tool (ask/renew/write/edit/bash) and emit its result;
/// failures are recorded as tool results rather than aborting the run.
async fn run_serial_tool(
    tc: &ToolCall,
    ctx: &ExecutionCtx<'_>,
    ask_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    renew_executed: &mut bool,
    tool_responses: &mut Vec<ToolResponse>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) {
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
            // bash streams its output live like LLM text chunks.
            if tc.fn_name == "bash" {
                exec_tool_streaming(tool, tc, workspace, cancel, on_event).await
            } else {
                exec_tool(tool, tc, workspace, cancel).await
            }
        }
    };
    let (response, result) = build_tool_result(tc, result);
    tool_responses.push(response);
    on_event(SessionEvent::ToolResult(result)).await;
}

/// Configuration for a send request to the LLM.
pub struct SendConfig {
    pub model: ModelInfo,
    pub workspace: std::path::PathBuf,
    /// Session id keying this stream's file snapshots.
    pub session_id: String,
    pub system_prompt: String,
    pub user_prompt: Option<UserPrompt>,
    pub tools: Vec<ToolRef>,
    /// Shared slot for a user prompt injected during streaming (tool execution / thinking).
    pub injected_prompt: Arc<Mutex<Option<String>>>,
    /// Receiver for the builtin ask tool's user response.
    pub ask_receiver: tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    /// Shared ask-tool deadline — the UI extends it to give more time.
    pub ask_deadline: Arc<Mutex<Instant>>,
    /// Receiver for task-tool reports, tagged with the originating call_id.
    pub task_receiver: tokio::sync::mpsc::UnboundedReceiver<(String, Result<String, String>)>,
    pub user_agent: String,
    /// In-progress tool execution stops when this token is cancelled.
    pub cancel_token: CancellationToken,
    /// Max agent-loop iterations (tool-calling rounds) before giving up.
    pub max_iterations: usize,
    /// Seconds of stream silence before giving up (0 = off).
    pub stream_stall_timeout_secs: u64,
}

/// Stream an LLM interaction with a tool-execution loop.
///
/// Text/reasoning chunks and tool results are emitted via `on_event`; the loop
/// repeats until the LLM responds without tool calls. A `false` return stops early.
pub async fn send_stream(
    config: SendConfig,
    history: Vec<ChatMessage>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) {
    let SendConfig {
        model,
        workspace,
        session_id,
        system_prompt,
        user_prompt,
        tools,
        injected_prompt,
        mut ask_receiver,
        ask_deadline,
        mut task_receiver,
        user_agent,
        cancel_token,
        max_iterations,
        stream_stall_timeout_secs,
    } = config;

    let client = build_client(&model.base_url, &model.api_key, &model.api_type);

    // Build chat request from genai history directly.
    // System prompt as a message with 1h cache TTL (rarely changes, large).
    let sys_msg = ChatMessage::system(system_prompt).with_options(CacheControl::Ephemeral1h);
    let mut chat_req = ChatRequest::default()
        .append_message(sys_msg)
        .with_tools(tools::build_tools(&tools, model.strict));
    chat_req = chat_req.append_messages(history);

    // Optionally add a new user message (None when resending history as-is).
    let mut genai_messages: Vec<ChatMessage> = Vec::new();
    if let Some(prompt) = &user_prompt {
        let parts = prompt.to_content_parts();
        let user_msg = ChatMessage::user(MessageContent::from_parts(parts));
        chat_req.messages.push(user_msg.clone());
        genai_messages.push(user_msg);
    }

    // Chat options: capture content for tool-call extraction, normalize reasoning.
    let mut chat_options = ChatOptions::default()
        .with_normalize_reasoning_content(true)
        .with_capture_content(true)
        .with_capture_reasoning_content(true)
        .with_capture_tool_calls(true)
        .with_capture_usage(true)
        .with_extra_headers(("user-agent", user_agent));

    if model.max_tokens > 0 {
        chat_options = chat_options.with_max_tokens(model.max_tokens);
    }

    // Set reasoning effort; omit it entirely when thinking is off.
    if model.thinking {
        let reasoning_effort = model
            .thinking_level
            .to_lowercase()
            .parse::<ReasoningEffort>()
            .unwrap_or(ReasoningEffort::Medium);
        chat_options = chat_options.with_reasoning_effort(reasoning_effort);
    }

    // Agent loop: keep calling the LLM until it responds without tool calls.
    let mut finished = false;

    /// Check for and inject a user prompt stashed during streaming.
    async fn inject_user_prompt(
        pending: &Mutex<Option<String>>,
        chat_req: &mut ChatRequest,
        genai_messages: &mut Vec<ChatMessage>,
        on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
    ) -> Option<bool> {
        let prompt = lock(pending).take()?;
        let user_msg = ChatMessage::user(prompt.clone());
        chat_req.messages.push(user_msg.clone());
        genai_messages.push(user_msg);
        if !on_event(SessionEvent::UserPrompt(prompt)).await {
            let drained = std::mem::take(genai_messages);
            on_event(SessionEvent::Cancelled(drained)).await;
            return Some(false);
        }
        Some(true)
    }

    /// Sleep until `deadline`, or wait forever when the stall watchdog is off.
    async fn stall_sleep(deadline: Option<Instant>) {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
            None => std::future::pending::<()>().await,
        }
    }

    // Execution context for the tool loop below (loop-invariant).
    let exec_ctx = ExecutionCtx {
        tools: &tools,
        workspace: &workspace,
        cancel_token: &cancel_token,
        ask_deadline: &ask_deadline,
    };

    for _ in 0..max_iterations {
        // Signal that we're connecting to the LLM.
        on_event(SessionEvent::PhaseChange(DialogPhase::LlmLoading)).await;

        // Keep a single rolling cache breakpoint at the conversation tail
        // (Anthropic limit: 4 breakpoints; system prompt uses 1 for Ephemeral1h).
        mark_cache_tail(&mut chat_req.messages);

        // Establish the stream, auto-retrying transient failures with a countdown.
        let (mut stream, first_event) = match acquire_stream_with_retry(
            &client,
            &model,
            &chat_req,
            &chat_options,
            &cancel_token,
            on_event,
        )
        .await
        {
            Ok(Some(acquired)) => (acquired.stream, acquired.first),
            Ok(None) => {
                on_event(SessionEvent::Cancelled(genai_messages)).await;
                return;
            }
            Err(msg) => {
                on_event(SessionEvent::Error(msg, genai_messages)).await;
                return;
            }
        };

        // Accumulate reasoning from chunks (captured_content covers text + tool calls).
        let mut captured_content: Option<MessageContent> = None;
        let mut captured_reasoning: Option<String> = None;
        let mut thinking_signaled = false;

        // Stall watchdog: Anthropic heartbeats every ~15-30s, so silence past
        // the window means the stream died. Any event resets the deadline.
        let stall_timeout = Duration::from_secs(stream_stall_timeout_secs);
        let mut stall_deadline =
            (stall_timeout > Duration::ZERO).then(|| Instant::now() + stall_timeout);

        // First event was already pulled; race the rest against cancellation
        // and the stall watchdog.
        let mut pending_event = first_event;
        loop {
            let event = match pending_event.take() {
                Some(event) => Some(Ok(event)),
                None => tokio::select! {
                    // Biased so ties resolve deterministically: cancel > stream data > stall timeout.
                    biased;
                    _ = cancel_token.cancelled() => {
                        on_event(SessionEvent::Cancelled(genai_messages)).await;
                        return;
                    }
                    ev = stream.next() => ev,
                    _ = stall_sleep(stall_deadline) => {
                        on_event(SessionEvent::Error(
                            format!(
                                "LLM stream stalled: no data for {stream_stall_timeout_secs}s; the connection may have died. Please retry.",
                            ),
                            genai_messages,
                        ))
                        .await;
                        return;
                    }
                },
            };
            let Some(event) = event else { break };
            // Any event is proof of life — reset the stall deadline.
            stall_deadline = stall_deadline.map(|_| Instant::now() + stall_timeout);
            match event {
                // Skip empty chunk, so a UI placeholder isn't created for it.
                Ok(ChatStreamEvent::Chunk(chunk)) if !chunk.content.is_empty() => {
                    if !thinking_signaled {
                        thinking_signaled = true;
                        on_event(SessionEvent::PhaseChange(DialogPhase::LlmThinking)).await;
                    }
                    if !on_event(SessionEvent::Content(chunk.content)).await {
                        on_event(SessionEvent::Cancelled(genai_messages)).await;
                        return;
                    }
                }
                Ok(ChatStreamEvent::ReasoningChunk(chunk)) if !chunk.content.is_empty() => {
                    if !thinking_signaled {
                        thinking_signaled = true;
                        on_event(SessionEvent::PhaseChange(DialogPhase::LlmThinking)).await;
                    }
                    if !on_event(SessionEvent::Reasoning(chunk.content)).await {
                        on_event(SessionEvent::Cancelled(genai_messages)).await;
                        return;
                    }
                }
                Ok(ChatStreamEvent::End(end)) => {
                    captured_content = end.captured_content;
                    captured_reasoning = end.captured_reasoning_content;
                    if !on_event(SessionEvent::TokenUsage(end.captured_usage)).await {
                        on_event(SessionEvent::Cancelled(genai_messages)).await;
                        return;
                    }
                }
                // ignore Start, Heartbeat, ThoughtSignature, ToolCallChunk, empty chunks
                Ok(_) => {}
                Err(e) => {
                    on_event(SessionEvent::Error(
                        format!("stream error: {e}"),
                        genai_messages,
                    ))
                    .await;
                    return;
                }
            }
        }

        // captured_content has full text + tool calls thanks to ChatOptions.
        let assistant_content =
            captured_content.unwrap_or_else(|| MessageContent::from_text(String::new()));
        let mut tool_calls: Vec<ToolCall> = assistant_content
            .tool_calls()
            .into_iter()
            .cloned()
            .collect();

        // Drop an empty reasoning capture.
        let assistant_msg = ChatMessage::assistant(assistant_content)
            .with_reasoning_content(captured_reasoning.filter(|r| !r.is_empty()));

        // Append assistant message to request + genai history.
        chat_req = chat_req.append_message(assistant_msg.clone());
        genai_messages.push(assistant_msg);

        if tool_calls.is_empty() {
            // Check for a user prompt sent during LlmLoading / LlmThinking.
            let result = inject_user_prompt(
                &injected_prompt,
                &mut chat_req,
                &mut genai_messages,
                on_event,
            )
            .await;
            match result {
                Some(true) => continue,
                Some(false) => return,
                None => {}
            }
            // Final assistant response — no more tool calls.
            finished = true;
            break;
        } else if tool_calls.len() > 1 {
            tools::move_renews_to_end(&mut tool_calls);
        }

        // Signal tool execution before starting, so the status bar updates even for sync tools.
        on_event(SessionEvent::PhaseChange(DialogPhase::ToolExecuting)).await;

        // Yield so iced re-renders the state change before tool execution.
        tokio::task::yield_now().await;

        // Notify the UI of ALL pending tool calls at once
        let calls: Vec<ChatToolCall> = tool_calls
            .iter()
            .map(|tc| ChatToolCall {
                name: tc.fn_name.clone(),
                call_id: Some(tc.call_id.clone()),
                args: tc.fn_arguments.clone(),
            })
            .collect();
        if !on_event(SessionEvent::ToolCalls(calls.clone())).await {
            on_event(SessionEvent::Cancelled(genai_messages)).await;
            return;
        }

        // Snapshot `write`/`edit` targets before tools run — blocking thread,
        // awaited so the pre-image read beats the tool's own write.
        let snapshotted = crate::app::snapshot::capture_tool_targets(
            workspace.clone(),
            session_id.clone(),
            &calls,
        )
        .await;
        if !snapshotted.is_empty() && !on_event(SessionEvent::SnapshotsCaptured(snapshotted)).await
        {
            on_event(SessionEvent::Cancelled(genai_messages)).await;
            return;
        }

        // Parallel tools run in batches; serial tools (ask/renew/write/edit/bash) are barriers.
        // Every tool call runs and produces a result, so the tool calls in the
        // assistant message always have matching results in history.
        let mut tool_responses: Vec<ToolResponse> = Vec::with_capacity(tool_calls.len());
        let mut renew_executed = false;
        let mut batch: Vec<&ToolCall> = Vec::new();

        for tc in &tool_calls {
            if !is_serial_tool(&tc.fn_name) {
                batch.push(tc);
                continue;
            }
            // Serial tools are barriers — run the pending parallel batch first.
            run_parallel_batch(
                &mut batch,
                &exec_ctx,
                &mut task_receiver,
                &mut tool_responses,
                on_event,
            )
            .await;
            run_serial_tool(
                tc,
                &exec_ctx,
                &mut ask_receiver,
                &mut renew_executed,
                &mut tool_responses,
                on_event,
            )
            .await;
        }

        // Drain any remaining parallel batch.
        run_parallel_batch(
            &mut batch,
            &exec_ctx,
            &mut task_receiver,
            &mut tool_responses,
            on_event,
        )
        .await;

        // Append tool responses to the request and genai history.
        chat_req = chat_req.append_message(tool_responses.clone());
        genai_messages.push(ChatMessage::from(tool_responses));

        // When renew was called, stop the current session — no more requests.
        if renew_executed {
            on_event(SessionEvent::Done(genai_messages)).await;
            return;
        }

        // Check cancellation after tool calls so to keep tool results match in history.
        if cancel_token.is_cancelled() {
            on_event(SessionEvent::Cancelled(genai_messages)).await;
            return;
        }

        // Inject any user prompt sent during tool execution.
        let result = inject_user_prompt(
            &injected_prompt,
            &mut chat_req,
            &mut genai_messages,
            on_event,
        )
        .await;
        if let Some(false) = result {
            return;
        }
    }

    if finished {
        on_event(SessionEvent::Done(genai_messages)).await;
    } else {
        on_event(SessionEvent::Error(
            format!("Exceeded maximum tool-calling iterations ({max_iterations})"),
            genai_messages,
        ))
        .await;
    }
}

/// Build a genai `Client` with custom auth, endpoint, and adapter kind.
fn build_client(base_url: &str, api_key: &str, api_type: &str) -> Client {
    let adapter_kind = AdapterKind::from_lower_str(api_type).unwrap_or(AdapterKind::OpenAI);
    let has_custom_endpoint = !base_url.is_empty();
    let has_custom_key = !api_key.is_empty();

    if !has_custom_endpoint && !has_custom_key {
        return Client::default();
    }

    let mut base_url = base_url.to_string();
    // Ensure trailing slash so genai's URL join appends rather than replaces
    // the last path segment (e.g. "/v1/" + "chat/completions" → "/v1/chat/completions").
    if !base_url.ends_with('/') {
        base_url.push('/');
    }

    let api_key = crabot::model::resolve_api_key(api_key);

    let target_resolver = ServiceTargetResolver::from_resolver_fn(
        move |target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
            let ServiceTarget {
                endpoint: default_endpoint,
                auth: default_auth,
                model,
            } = target;

            let endpoint = if has_custom_endpoint {
                Endpoint::from_owned(Arc::from(base_url.as_str()))
            } else {
                default_endpoint
            };

            let auth = if has_custom_key {
                AuthData::from_single(api_key.as_str())
            } else {
                default_auth
            };
            Ok(ServiceTarget {
                endpoint,
                auth,
                model: ModelIden::new(adapter_kind, model.model_name),
            })
        },
    );

    Client::builder()
        .with_service_target_resolver(target_resolver)
        .build()
}

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
