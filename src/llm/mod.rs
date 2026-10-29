//! LLM interaction engine: [`send_stream`] drives the agent loop — streaming
//! with retry, tool-call execution, and per-message persistence.

use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use genai::adapter::AdapterKind;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, MessageContent, ReasoningEffort,
    ToolCall, ToolResponse,
};

use crate::app::session_state::SessionEvent;
use crate::tools::{self, ToolRef};
use crabot::chat::{ToolCall as ChatToolCall, attachment_marker_message, image_marker_message};
use crabot::lock;
use crabot::model::ModelInfo;
use crabot::tools::{ImageAttachment, ImageBudget, RouteImages, set_route_images};
use crabot::user::UserPrompt;

mod attachment;
mod client;
mod image;
mod stream;
mod tool_call;

use client::build_client;
pub(crate) use image::ImageCache;
pub use stream::DialogPhase;
use stream::{
    AttemptOutcome, DEFAULT_RETRY_DELAY, MAX_ATTEMPTS, RetryHint, StreamCtx, failure_message,
    failure_status, mark_cache_tail, pause_before_retry, retry_hint, stream_attempt,
};
use tool_call::{ExecutionCtx, is_serial_tool, run_parallel_batch, run_serial_tool};

/// Configuration for a send request to the LLM.
pub struct SendConfig {
    pub model: ModelInfo,
    pub workspace: std::path::PathBuf,
    /// Session id keying this stream's state: file snapshots and logs.
    pub session_id: String,
    /// Session tab number keying this stream's process-ownership tags.
    pub tab_number: usize,
    pub system_prompt: String,
    pub user_prompt: Option<UserPrompt>,
    pub tools: Vec<ToolRef>,
    /// Shared slot for a user prompt injected during streaming (tool execution / thinking).
    pub injected_prompt: Arc<Mutex<Option<UserPrompt>>>,
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
    /// Session-scoped encoded-image cache, reused across streams of the tab.
    pub image_cache: Arc<tokio::sync::Mutex<ImageCache>>,
}

/// Shown once the final response was cut off by the output-token limit.
const TRUNCATED_NOTICE: &str = concat!(
    "⚠️ The response was truncated by the model's max output token limit. ",
    "Increase the model's max tokens or continue the conversation."
);

/// Push a message into the request and record it in history.
async fn record_msg(
    chat_req: &mut ChatRequest,
    user_msg: ChatMessage,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> bool {
    chat_req.messages.push(user_msg.clone());
    if !on_event(SessionEvent::MessageReady(user_msg)).await {
        // Stopped mid-record — end the stream so the tab returns to Idle.
        on_event(SessionEvent::Cancelled).await;
        return false;
    }
    true
}

/// Inject a user prompt stashed during streaming.
/// `None`: nothing pending. `Some(true)`: injected — keep the agent loop
/// running so a follow-up request answers it. `Some(false)`: stopped.
async fn inject_user_prompt(
    pending: &Mutex<Option<UserPrompt>>,
    chat_req: &mut ChatRequest,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> Option<bool> {
    let prompt = lock(pending).take()?;
    let user_msg = ChatMessage::user(MessageContent::from_parts(prompt.to_content_parts()));
    if !record_msg(chat_req, user_msg, on_event).await {
        return Some(false);
    }
    if !on_event(SessionEvent::UserPrompt(prompt.content.clone())).await {
        on_event(SessionEvent::Cancelled).await;
        return Some(false);
    }
    if !record_attachments(chat_req, &prompt.attachments, on_event).await {
        return Some(false);
    }
    Some(true)
}

/// Record the attachment marker for a prompt's files, if it has any.
async fn record_attachments(
    chat_req: &mut ChatRequest,
    attachments: &[crabot::user::UserAttachment],
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> bool {
    match attachment_marker_message(attachments) {
        Some(marker) => record_msg(chat_req, marker, on_event).await,
        None => true,
    }
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
        tab_number,
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
        image_cache,
    } = config;

    let client = match build_client(&model) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(
                model = %model.model_id,
                session = %session_id,
                error = %error,
                "failed to build LLM client"
            );
            on_event(SessionEvent::error(
                format!("Failed to set up the LLM client: {error}"),
                "client setup failed",
            ))
            .await;
            return;
        }
    };

    // Build chat request from genai history directly.
    // System prompt as a message with 1h cache TTL (rarely changes, large).
    let sys_msg = ChatMessage::system(system_prompt).with_options(CacheControl::Ephemeral1h);
    let mut chat_req = ChatRequest::default().with_tools(tools::build_tools(&tools, model.strict));
    // Record the prompt (audit-only) and put it on the wire first.
    if !record_msg(&mut chat_req, sys_msg, on_event).await {
        return;
    }
    // Prior audit records must not be re-sent to the LLM.
    chat_req = chat_req.append_messages(history.into_iter().filter(|m| m.role != ChatRole::System));

    // Optionally add a new user message (None when resending history as-is).
    if let Some(prompt) = &user_prompt {
        let user_msg = ChatMessage::user(MessageContent::from_parts(prompt.to_content_parts()));
        // Record it immediately so the prompt survives a failed stream.
        if !record_msg(&mut chat_req, user_msg, on_event).await {
            return;
        }
        if !record_attachments(&mut chat_req, &prompt.attachments, on_event).await {
            return;
        }
    }

    let adapter = client::adapter_kind(&model.api_type);
    let chat_options = build_chat_options(
        &model,
        adapter,
        user_agent,
        stream_stall_timeout_secs,
        &session_id,
    );

    // Agent loop: keep calling the LLM until it responds without tool calls.
    let mut finished = false;
    let mut truncated = false;

    // Publish what this route can consume, so `read` refuses an image the
    // model could not see and reports the dimensions the request will carry.
    let image_budget = ImageBudget::for_model(&model);
    set_route_images(
        tab_number,
        RouteImages {
            vision: model.vision,
            model_id: model.model_id.clone(),
            budget: image_budget,
        },
    );

    // Execution context for the tool loop below (loop-invariant).
    let exec_ctx = ExecutionCtx {
        tools: &tools,
        workspace: &workspace,
        cancel_token: &cancel_token,
        tab_number,
        ask_deadline: &ask_deadline,
    };

    tracing::info!(
        model = %model.model_id,
        session = %session_id,
        messages = chat_req.messages.len(),
        tools = tools.len(),
        "starting LLM interaction"
    );

    // Loop-invariant inputs shared by every streaming attempt.
    let stream_ctx = StreamCtx {
        client: &client,
        model: &model,
        session_id: &session_id,
        chat_options: &chat_options,
        cancel_token: &cancel_token,
    };

    for _ in 0..max_iterations {
        // Signal that we're connecting to the LLM.
        on_event(SessionEvent::PhaseChange(DialogPhase::LlmLoading)).await;

        // Deliver text files and images attached by the user before the tail
        // cache breakpoint is set. Text markers become text parts; image
        // markers are handed to `attach_images` below.
        attachment::attach_text_files(&mut chat_req.messages, &workspace).await;

        // Deliver any images read by tools before the tail cache breakpoint is
        // set (a merged tool turn replaces the marker message). The lock is
        // scoped so no guard lives across the request's await points.
        {
            let mut cache = image_cache.lock().await;
            image::attach_images(
                &mut chat_req.messages,
                &workspace,
                model.vision,
                adapter,
                &mut cache,
                image_budget,
            )
            .await;
        }

        // Keep a single rolling cache breakpoint at the conversation tail
        // (Anthropic limit: 4 breakpoints; system prompt uses 1 for Ephemeral1h).
        mark_cache_tail(&mut chat_req.messages);

        // Retry transient failures (setup, first poll, mid-stream, stall,
        // empty response) by re-sending the same request; `attempt` counts
        // requests this turn.
        let mut attempt: u32 = 0;
        let assistant_msg = loop {
            attempt += 1;
            let outcome =
                stream_attempt(&stream_ctx, &chat_req, stream_stall_timeout_secs, on_event).await;

            // Transient outcomes yield a reason, status label, and delay.
            let (reason, status, retry_delay) = match outcome {
                AttemptOutcome::Finished {
                    msg,
                    truncated: was_truncated,
                } => {
                    truncated = was_truncated;
                    break msg;
                }
                AttemptOutcome::Cancelled => {
                    tracing::info!(
                        model = %model.model_id,
                        session = %session_id,
                        "LLM request cancelled"
                    );
                    on_event(SessionEvent::Cancelled).await;
                    return;
                }
                AttemptOutcome::Failed { stage, error } => match retry_hint(&error) {
                    Some(RetryHint::Retry { delay }) if attempt < MAX_ATTEMPTS => (
                        error.to_string(),
                        failure_status(&error),
                        delay.unwrap_or(DEFAULT_RETRY_DELAY),
                    ),
                    Some(RetryHint::TooLong { delay }) if attempt < MAX_ATTEMPTS => {
                        tracing::warn!(
                            attempt,
                            model = %model.model_id,
                            delay_secs = delay.as_secs(),
                            "provider requested a retry delay beyond the automatic limit"
                        );
                        on_event(SessionEvent::error(
                            format!(
                                "The provider asked to retry after {}s, longer than the automatic retry limit. {error}",
                                delay.as_secs()
                            ),
                            failure_status(&error),
                        ))
                        .await;
                        return;
                    }
                    _ => {
                        tracing::error!(
                            attempt,
                            model = %model.model_id,
                            error = %error,
                            "LLM request failed"
                        );
                        on_event(SessionEvent::error(
                            failure_message(stage, &error, attempt),
                            failure_status(&error),
                        ))
                        .await;
                        return;
                    }
                },
                // Transient outcomes retried below like a failure.
                outcome @ (AttemptOutcome::Empty | AttemptOutcome::Stalled) => {
                    let (reason, status, error) = if matches!(outcome, AttemptOutcome::Stalled) {
                        let stalled =
                            format!("stream stalled: no data for {stream_stall_timeout_secs}s");
                        (
                            stalled.clone(),
                            "response timeout".into(),
                            format!("LLM {stalled}; the connection may have died. Please retry."),
                        )
                    } else {
                        (
                            "empty assistant response".into(),
                            "empty response".into(),
                            format!(
                                "LLM returned an empty response {attempt} times. Please retry."
                            ),
                        )
                    };
                    if attempt >= MAX_ATTEMPTS {
                        tracing::warn!(
                            attempt,
                            model = %model.model_id,
                            session = %session_id,
                            "giving up: {reason}"
                        );
                        on_event(SessionEvent::error(error, status)).await;
                        return;
                    }
                    (reason, status, DEFAULT_RETRY_DELAY)
                }
            };

            if !pause_before_retry(
                attempt,
                retry_delay,
                &model.model_id,
                &reason,
                status,
                on_event,
                &cancel_token,
            )
            .await
            {
                on_event(SessionEvent::Cancelled).await;
                return;
            }
        };

        // The finished message is guaranteed non-empty (empty responses retry
        // inside the loop), so it is always resent and persisted.
        let mut tool_calls: Vec<ToolCall> = assistant_msg
            .content
            .tool_calls()
            .into_iter()
            .cloned()
            .collect();

        if !record_msg(&mut chat_req, assistant_msg.clone(), on_event).await {
            return;
        }

        if tool_calls.is_empty() {
            // Check for a user prompt sent during LlmLoading / LlmThinking.
            match inject_user_prompt(&injected_prompt, &mut chat_req, on_event).await {
                // A prompt was injected — loop again so the next request answers it.
                Some(true) => continue,
                Some(false) => return,
                None => {}
            }
            // Final assistant response — no more tool calls.
            tracing::debug!(
                model = %model.model_id,
                "LLM responded without tool calls"
            );
            finished = true;
            break;
        } else if tool_calls.len() > 1 {
            tools::move_renews_to_end(&mut tool_calls);
        }

        tracing::info!(
            model = %model.model_id,
            tool_calls = tool_calls.len(),
            "LLM requested tool calls"
        );

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
            on_event(SessionEvent::Cancelled).await;
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
            on_event(SessionEvent::Cancelled).await;
            return;
        }

        // Parallel tools run in batches; serial tools (ask/renew/write/edit/bash) are barriers.
        // Every tool call runs and produces a result, so the tool calls in the
        // assistant message always have matching results in history.
        let mut tool_responses: Vec<ToolResponse> = Vec::with_capacity(tool_calls.len());
        let mut attachments: Vec<ImageAttachment> = Vec::new();
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
                &mut attachments,
                on_event,
            )
            .await;
            run_serial_tool(
                tc,
                &exec_ctx,
                &mut ask_receiver,
                &mut renew_executed,
                &mut tool_responses,
                &mut attachments,
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
            &mut attachments,
            on_event,
        )
        .await;

        // Append tool responses to the request and record them.
        let tool_msg = ChatMessage::from(tool_responses);
        chat_req = chat_req.append_message(tool_msg.clone());
        if !on_event(SessionEvent::MessageReady(tool_msg)).await {
            // Cancelled while recording — `MessageReady` already persisted the
            // results, so history keeps them either way.
            on_event(SessionEvent::Cancelled).await;
            return;
        }

        // Images read by tools are recorded as path markers; the bytes are
        // attached when the next request is built (folded into the tool turn
        // for providers that require strict role alternation).
        let mut images: Vec<String> = Vec::new();
        for a in &attachments {
            if !images.contains(&a.path) {
                images.push(a.path.clone());
            }
        }
        if !images.is_empty() {
            let marker = image_marker_message(&images);
            chat_req = chat_req.append_message(marker.clone());
            if !on_event(SessionEvent::MessageReady(marker)).await {
                on_event(SessionEvent::Cancelled).await;
                return;
            }
        }

        // When renew was called, stop the current session — no more requests.
        if renew_executed {
            tracing::info!(model = %model.model_id, session = %session_id, "renew called, ending session for successor");
            on_event(SessionEvent::Done).await;
            return;
        }

        // Check cancellation after tool calls so to keep tool results match in history.
        if cancel_token.is_cancelled() {
            tracing::info!(model = %model.model_id, session = %session_id, "cancelled after tool execution");
            on_event(SessionEvent::Cancelled).await;
            return;
        }

        // Inject any user prompt sent during tool execution.
        if inject_user_prompt(&injected_prompt, &mut chat_req, on_event).await == Some(false) {
            return;
        }
    }

    if finished {
        if truncated && !on_event(SessionEvent::Notice(TRUNCATED_NOTICE.into())).await {
            return;
        }
        tracing::info!(model = %model.model_id, session = %session_id, "LLM interaction complete");
        on_event(SessionEvent::Done).await;
    } else {
        tracing::error!(
            model = %model.model_id,
            max_iterations,
            "exceeded maximum tool-calling iterations"
        );
        on_event(SessionEvent::error(
            format!("Exceeded maximum tool-calling iterations ({max_iterations})"),
            "max iterations reached",
        ))
        .await;
    }
}

/// Build per-request options from the model config.
fn build_chat_options(
    model: &ModelInfo,
    adapter: AdapterKind,
    user_agent: String,
    header_timeout_secs: u64,
    session_id: &str,
) -> ChatOptions {
    // Lowercase the names so a provider header replaces the built-in user agent
    // instead of arriving as a second, order-dependent value.
    let mut headers = std::collections::BTreeMap::from([("user-agent".to_string(), user_agent)]);
    headers.extend(
        model
            .headers
            .iter()
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())),
    );
    let mut options = ChatOptions::default()
        .with_normalize_reasoning_content(true)
        .with_capture_content(true)
        .with_capture_reasoning_content(true)
        .with_capture_tool_calls(true)
        .with_capture_usage(true)
        .with_extra_headers(headers.into_iter().collect::<Vec<_>>());

    if model.max_tokens > 0 {
        options = options.with_max_tokens(model.max_tokens);
    }
    if header_timeout_secs > 0 {
        options = options.with_stream_header_timeout(Duration::from_secs(header_timeout_secs));
    }
    if let Some(body) = extra_body(model) {
        options = options.with_extra_body(body);
    }

    if model.thinking {
        let effort = model
            .thinking_level
            .to_lowercase()
            .parse::<ReasoningEffort>()
            .unwrap_or(ReasoningEffort::Medium);
        options = options.with_reasoning_effort(effort);
    } else if adapter == AdapterKind::Anthropic {
        // Anthropic models may think by default; `Zero` is the only opt-out.
        options = options.with_reasoning_effort(ReasoningEffort::Zero);
    }

    trace_raw_frames(options, session_id, &model.model_id)
}

/// Provider-specific body JSON; invalid input is ignored with a warning.
fn extra_body(model: &ModelInfo) -> Option<serde_json::Value> {
    let text = model.extra_body.trim();
    if text.is_empty() {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) if value.is_object() => Some(value),
        Ok(_) => {
            tracing::warn!(
                model = %model.model_id,
                "provider extra_body must be a JSON object; ignoring"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                model = %model.model_id,
                %error,
                "invalid provider extra_body JSON; ignoring"
            );
            None
        }
    }
}

/// Attach a raw-frame logger when `CRABOT_TRACE_RAW_FRAMES` is set.
fn trace_raw_frames(options: ChatOptions, session_id: &str, model_id: &str) -> ChatOptions {
    let enabled = std::env::var("CRABOT_TRACE_RAW_FRAMES").is_ok_and(|value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off" | "no"
        )
    });
    if !enabled {
        return options;
    }
    let session = session_id.to_string();
    let model = model_id.to_string();
    options.with_raw_frame_fn(move |ctx, frame| {
        const LIMIT: usize = 4096;
        let truncated = frame.data.len() > LIMIT;
        let data: String = frame.data.chars().take(LIMIT).collect();
        tracing::info!(
            target: "crabot::raw_frame",
            session = %session,
            model = %model,
            stream_id = ctx.stream_id,
            index = frame.index,
            event = frame.event.unwrap_or_default(),
            elapsed_us = frame.elapsed_us,
            truncated,
            data = %data,
            "raw provider frame"
        );
    })
}
