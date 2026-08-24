//! LLM interaction engine: [`send_stream`] drives the agent loop — streaming
//! with retry, tool-call execution, and per-message persistence.

use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, MessageContent, ReasoningEffort,
    ToolCall, ToolResponse,
};

use crate::app::session_state::SessionEvent;
use crate::tools::{self, ToolRef};
use crabot::chat::ToolCall as ChatToolCall;
use crabot::lock;
use crabot::model::ModelInfo;
use crabot::user::UserPrompt;

mod client;
mod stream;
mod tool_call;

use client::build_client;
pub use stream::DialogPhase;
use stream::{
    AttemptOutcome, MAX_ATTEMPTS, StreamCtx, failure_message, is_retryable, mark_cache_tail,
    pause_before_retry, stream_attempt,
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

/// Inject a user prompt stashed during streaming; `false` stops the stream.
async fn inject_user_prompt(
    pending: &Mutex<Option<String>>,
    chat_req: &mut ChatRequest,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> bool {
    let Some(prompt) = lock(pending).take() else {
        return true;
    };
    if !record_msg(chat_req, ChatMessage::user(prompt.clone()), on_event).await {
        return false;
    }
    if !on_event(SessionEvent::UserPrompt(prompt)).await {
        on_event(SessionEvent::Cancelled).await;
        return false;
    }
    true
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
    } = config;

    let client = build_client(&model.base_url, &model.api_key, &model.api_type);

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

            // Terminal outcomes break or return; transient ones yield a retry reason.
            let reason = match outcome {
                AttemptOutcome::Finished { msg } => break msg,
                AttemptOutcome::Cancelled => {
                    tracing::info!(
                        model = %model.model_id,
                        session = %session_id,
                        "LLM request cancelled"
                    );
                    on_event(SessionEvent::Cancelled).await;
                    return;
                }
                AttemptOutcome::Failed { stage, error } => {
                    if attempt < MAX_ATTEMPTS && is_retryable(&error) {
                        error.to_string()
                    } else {
                        tracing::error!(
                            attempt,
                            model = %model.model_id,
                            error = %error,
                            "LLM request failed"
                        );
                        on_event(SessionEvent::Error(failure_message(stage, &error, attempt)))
                            .await;
                        return;
                    }
                }
                // Transient outcomes retried below like a failure.
                outcome @ (AttemptOutcome::Empty | AttemptOutcome::Stalled) => {
                    let (reason, error) = if matches!(outcome, AttemptOutcome::Stalled) {
                        let stalled =
                            format!("stream stalled: no data for {stream_stall_timeout_secs}s");
                        (
                            stalled.clone(),
                            format!("LLM {stalled}; the connection may have died. Please retry."),
                        )
                    } else {
                        (
                            "empty assistant response".into(),
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
                        on_event(SessionEvent::Error(error)).await;
                        return;
                    }
                    reason
                }
            };

            if !pause_before_retry(attempt, &model.model_id, &reason, on_event, &cancel_token).await
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
            if !inject_user_prompt(&injected_prompt, &mut chat_req, on_event).await {
                return;
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

        // Append tool responses to the request and record them.
        let tool_msg = ChatMessage::from(tool_responses);
        chat_req = chat_req.append_message(tool_msg.clone());
        if !on_event(SessionEvent::MessageReady(tool_msg)).await {
            // Cancelled while recording — `MessageReady` already persisted the
            // results, so history keeps them either way.
            on_event(SessionEvent::Cancelled).await;
            return;
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
        if !inject_user_prompt(&injected_prompt, &mut chat_req, on_event).await {
            return;
        }
    }

    if finished {
        tracing::info!(model = %model.model_id, session = %session_id, "LLM interaction complete");
        on_event(SessionEvent::Done).await;
    } else {
        tracing::error!(
            model = %model.model_id,
            max_iterations,
            "exceeded maximum tool-calling iterations"
        );
        on_event(SessionEvent::Error(format!(
            "Exceeded maximum tool-calling iterations ({max_iterations})"
        )))
        .await;
    }
}
