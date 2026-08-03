use futures::{StreamExt, future::BoxFuture};
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Duration;

use genai::adapter::AdapterKind;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStream, ChatStreamEvent,
    MessageContent, ReasoningEffort, ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use reqwest::StatusCode;

use crate::app::session_state::{AskRequest, RetryInfo, SessionEvent, TaskRequest};
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

/// Resolve once the cancel token is set.
async fn wait_cancelled(cancel_token: &AtomicBool) {
    loop {
        if cancel_token.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
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
    cancel_token: &AtomicBool,
) -> Result<Option<AcquiredStream>, (AcquireStage, genai::Error)> {
    let stream_result = tokio::select! {
        res = client.exec_chat_stream(&model.model_id, chat_req.clone(), Some(chat_options)) => res,
        _ = wait_cancelled(cancel_token) => return Ok(None),
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
            ev = stream.next() => ev,
            _ = wait_cancelled(cancel_token) => return Ok(None),
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
    cancel_token: &AtomicBool,
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
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                        _ = wait_cancelled(cancel_token) => return Ok(None),
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

/// Configuration for a send request to the LLM.
pub struct SendConfig {
    pub model: ModelInfo,
    pub workspace: std::path::PathBuf,
    pub system_prompt: String,
    pub user_prompt: Option<UserPrompt>,
    pub tools: Vec<ToolRef>,
    /// Shared slot for a user prompt injected during streaming (tool execution / thinking).
    pub injected_prompt: Arc<Mutex<Option<String>>>,
    /// Receiver for the builtin ask tool's user response.
    pub ask_receiver: tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    /// Receiver for the builtin task tool's sub-agent report.
    pub task_receiver: tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    pub user_agent: String,
    /// When set to `true`, in-progress tool execution is cancelled.
    pub cancel_token: Arc<AtomicBool>,
    /// Max agent-loop iterations (tool-calling rounds) before giving up.
    pub max_iterations: usize,
}

/// Stream an LLM interaction with tool-execution loop.
///
/// Text and reasoning chunks are emitted immediately via the `on_event` callback.
/// Tool calls are executed after the stream ends for that turn, and results
/// are emitted. The loop continues until the LLM responds without tool calls.
///
/// The callback receives each [`Event`] and returns a future. If the
/// future resolves to `false`, streaming stops early.
pub async fn send_stream(
    config: SendConfig,
    history: Vec<ChatMessage>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) {
    let SendConfig {
        model,
        workspace,
        system_prompt,
        user_prompt,
        tools,
        injected_prompt,
        mut ask_receiver,
        mut task_receiver,
        user_agent,
        cancel_token,
        max_iterations,
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

    // Set reasoning effort, When thinking is off, omit it entirely
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
        let prompt = pending.lock().unwrap_or_else(|e| e.into_inner()).take()?;
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

        // Race each read against cancellation; the first event was already pulled.
        let mut pending_event = first_event;
        loop {
            let event = match pending_event.take() {
                Some(event) => Some(Ok(event)),
                None => tokio::select! {
                    ev = stream.next() => ev,
                    _ = wait_cancelled(&cancel_token) => {
                        on_event(SessionEvent::Cancelled(genai_messages)).await;
                        return;
                    }
                },
            };
            let Some(event) = event else { break };
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
                // ignore Start, ThoughtSignature, ToolCallChunk, empty chunks
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
        let tool_calls: Vec<ToolCall> = assistant_content
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
        }

        // Signal tool execution state to the UI *before* we start
        // executing so the status bar updates even when tools run
        // synchronously on a worker thread.
        on_event(SessionEvent::PhaseChange(DialogPhase::ToolExecuting)).await;

        // Yield once so the iced event loop can pick up the state change
        // and re-render before we proceed to tool execution.
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
        if !on_event(SessionEvent::ToolCalls(calls)).await {
            on_event(SessionEvent::Cancelled(genai_messages)).await;
            return;
        }

        // Execute each tool call and record results.
        // Unknown tools are reported back to the LLM as an error result
        // rather than aborting the loop, giving the model a chance to recover.
        let mut tool_responses: Vec<ToolResponse> = Vec::with_capacity(tool_calls.len());
        let mut renew_executed = false;
        for tc in tool_calls {
            // Resolve the tool on this thread so we don't have to clone the
            // name into the blocking closure. Unknown tools short-circuit to
            // an error result without spawning a task.
            let result = match tools.iter().find(|t| t.name() == tc.fn_name).cloned() {
                Some(_) if matches!(tc.fn_name.as_str(), "ask" | "task") => {
                    // Interactive tools: ask/task are intercepted here and routed
                    // to the UI, which answers through its mpsc channel.
                    let handled = if tc.fn_name == "ask" {
                        handle_ask_tool(&tc, &mut ask_receiver, &cancel_token, on_event).await
                    } else {
                        handle_task_tool(&tc, &mut task_receiver, &cancel_token, on_event).await
                    };
                    match handled {
                        Some(result) => result,
                        None => {
                            on_event(SessionEvent::Cancelled(genai_messages)).await;
                            return;
                        }
                    }
                }
                Some(_) if tc.fn_name == "renew" && !renew_executed => {
                    let prompt = tc
                        .fn_arguments
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if prompt.is_empty() {
                        Err("Renew called with an empty prompt — no new session created.".into())
                    } else if !on_event(SessionEvent::RenewRequest(prompt)).await {
                        Err("Renew event channel closed.".into())
                    } else {
                        renew_executed = true;
                        Ok("New session created with the provided prompt.".into())
                    }
                }
                Some(tool) => {
                    // Run tool execution on a blocking thread so the async
                    // task yields while the tool runs – this keeps the iced
                    // UI responsive and lets the "Tool executing…" status be
                    // painted.
                    let fn_args = tc.fn_arguments.clone();
                    let workspace = workspace.clone();
                    let cancel = cancel_token.clone();
                    tokio::task::spawn_blocking(move || {
                        tool.execute(&fn_args, &workspace, cancel.as_ref())
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("Tool execution panicked: {e}")))
                }
                None => Err(tools::unknown_tool_message(&tc.fn_name)),
            };

            // Flatten for genai's ToolResponse (genai expects plain String).
            // Errors get a uniform "Error: " envelope so both the LLM and the
            // session reload path can distinguish success from failure.
            let result_flat = match result.clone() {
                Ok(s) => s,
                Err(e) => envelope_error(&e),
            };
            tool_responses.push(ToolResponse::from_tool_call(&tc, result_flat));

            let tr = ChatToolResult {
                name: tc.fn_name,
                call_id: Some(tc.call_id),
                args: tc.fn_arguments,
                result,
                timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            };
            on_event(SessionEvent::ToolResult(tr)).await;
        }

        // Append tool responses to the request and genai history.
        chat_req = chat_req.append_message(tool_responses.clone());
        genai_messages.push(ChatMessage::from(tool_responses));

        // When renew was called, stop the current session — no more requests.
        if renew_executed {
            on_event(SessionEvent::Done(genai_messages)).await;
            return;
        }

        // Check cancellation after executing tool calls to keep tool results match in history.
        if cancel_token.load(std::sync::atomic::Ordering::Acquire) {
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

/// Drain stale results, emit `event` to the UI, then wait for the interactive
/// result or cancellation — optionally bounded by `timeout` (whose message is
/// returned to the caller as an `Ok` result).
///
/// Returns `None` when the event channel is closed (caller should emit
/// `Cancelled` and stop the agent loop).
async fn wait_for_result(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
    cancel_token: &AtomicBool,
    event: SessionEvent,
    timeout: Option<(std::time::Duration, &'static str)>,
) -> Option<Result<String, String>> {
    // Drain any stale result left over from a previous wait (e.g. a response
    // that arrived after its stream had already been cancelled).
    while receiver.try_recv().is_ok() {}
    if !on_event(event).await {
        return None;
    }
    Some(if let Some((duration, message)) = timeout {
        tokio::select! {
            result = receiver.recv() => match result {
                Some(result) => result,
                None => Err("Response channel closed.".into()),
            },
            _ = tokio::time::sleep(duration) => Ok(message.into()),
            _ = wait_cancelled(cancel_token) => Err("Cancelled by user.".into()),
        }
    } else {
        tokio::select! {
            result = receiver.recv() => match result {
                Some(result) => result,
                None => Err("Response channel closed.".into()),
            },
            _ = wait_cancelled(cancel_token) => Err("Cancelled by user.".into()),
        }
    })
}

/// Handle a builtin task-tool call: parse arguments, emit the request to the
/// UI (which spawns a sub-agent session tab), then wait for the sub-agent's
/// final report or cancellation. There is no timeout — subtasks may run for
/// many minutes; the Stop button remains the escape hatch.
async fn handle_task_tool(
    tc: &ToolCall,
    task_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    cancel_token: &AtomicBool,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> Option<Result<String, String>> {
    let prompt = tc
        .fn_arguments
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if prompt.trim().is_empty() {
        return Some(Err(
            "Task called with an empty prompt — no subtask spawned.".into(),
        ));
    }
    let arg = |key: &str| {
        tc.fn_arguments
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    };
    let request = TaskRequest {
        title: arg("title"),
        prompt,
        mode: arg("mode"),
        difficulty: arg("difficulty"),
    };
    wait_for_result(
        task_receiver,
        on_event,
        cancel_token,
        SessionEvent::TaskRequest(request),
        None,
    )
    .await
}

/// Handle a builtin ask-tool call: parse arguments, emit the question to
/// the UI, then wait for user response, cancellation, or timeout (120 s).
async fn handle_ask_tool(
    tc: &ToolCall,
    ask_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    cancel_token: &AtomicBool,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> Option<Result<String, String>> {
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
        cancel_token,
        SessionEvent::AskRequest(AskRequest { question, options }),
        Some((
            std::time::Duration::from_secs(120),
            "User did not respond before the timeout.",
        )),
    )
    .await
}
