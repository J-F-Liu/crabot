use futures::{StreamExt, future::BoxFuture};
use std::sync::{Arc, Mutex, atomic::AtomicBool};

use genai::adapter::AdapterKind;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, MessageContent,
    ReasoningEffort, ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};

use crate::tools::{self, ToolRef};
use crate::views::session_state::SessionEvent;
use crabot::chat::{ToolCall as ChatToolCall, ToolResult as ChatToolResult};
use crabot::model::ModelInfo;
use crabot::user::UserPrompt;

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

/// Max agent loop iterations to prevent infinite tool-calling cycles.
const MAX_ITERATIONS: usize = 100;

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
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
    pub pending_user_prompt: Arc<Mutex<Option<String>>>,
    /// Receiver for the builtin ask tool's user response.
    pub ask_receiver: tokio::sync::mpsc::UnboundedReceiver<Result<String, String>>,
    pub user_agent: String,
    /// When set to `true`, in-progress tool execution is cancelled.
    pub cancel_token: Arc<AtomicBool>,
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
        pending_user_prompt,
        mut ask_receiver,
        user_agent,
        cancel_token,
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
        let prompt = pending.lock().unwrap().take()?;
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

    for _ in 0..MAX_ITERATIONS {
        // Signal that we're connecting to the LLM.
        on_event(SessionEvent::PhaseChange(DialogPhase::LlmLoading)).await;

        // Keep a single rolling cache breakpoint at the conversation tail
        // (Anthropic limit: 4 breakpoints; system prompt uses 1 for Ephemeral1h).
        mark_cache_tail(&mut chat_req.messages);

        // Race the connect against cancellation so Stop takes effect promptly.
        let stream_result = tokio::select! {
            res = client.exec_chat_stream(&model.model_id, chat_req.clone(), Some(&chat_options)) => res,
            _ = wait_cancelled(&cancel_token) => {
                on_event(SessionEvent::Cancelled(genai_messages)).await;
                return;
            }
        };

        let mut stream = match stream_result {
            Ok(chat_res) => chat_res.stream,
            Err(e) => {
                on_event(SessionEvent::Error(
                    format!("exec_chat_stream: {e}"),
                    genai_messages,
                ))
                .await;
                return;
            }
        };

        // Accumulate reasoning from chunks (captured_content covers text + tool calls).
        let mut captured_content: Option<MessageContent> = None;
        let mut captured_reasoning: Option<String> = None;
        let mut thinking_signaled = false;

        // Race each read against cancellation so Stop works during stream idle.
        loop {
            let event = tokio::select! {
                ev = stream.next() => ev,
                _ = wait_cancelled(&cancel_token) => {
                    on_event(SessionEvent::Cancelled(genai_messages)).await;
                    return;
                }
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
                &pending_user_prompt,
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
        for tc in tool_calls {
            // Resolve the tool on this thread so we don't have to clone the
            // name into the blocking closure. Unknown tools short-circuit to
            // an error result without spawning a task.
            let result = match tools.iter().find(|t| t.name() == tc.fn_name).cloned() {
                Some(_) if tc.fn_name == "ask" => {
                    match handle_ask_tool(&tc, &mut ask_receiver, &cancel_token, on_event).await {
                        Some(result) => result,
                        None => {
                            on_event(SessionEvent::Cancelled(genai_messages)).await;
                            return;
                        }
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
            let result_flat = result.clone().unwrap_or_else(|e| e);
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

        // Check cancellation after executing tool calls to keep tool results match in history.
        if cancel_token.load(std::sync::atomic::Ordering::Acquire) {
            on_event(SessionEvent::Cancelled(genai_messages)).await;
            return;
        }

        // Inject any user prompt sent during tool execution.
        let result = inject_user_prompt(
            &pending_user_prompt,
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
            format!("Exceeded maximum tool-calling iterations ({MAX_ITERATIONS})"),
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

/// Handle a builtin ask-tool call: parse arguments, emit the question to
/// the UI, then wait for user response, cancellation, or timeout (120 s).
///
/// Returns `None` when the event channel is closed (caller should emit
/// `Cancelled` and stop the agent loop).
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

    // Drain any stale messages left over from a previous ask (e.g. the user
    // answered after the previous timeout had already fired).
    while ask_receiver.try_recv().is_ok() {}

    if !on_event(SessionEvent::AskRequest(
        crate::views::session_state::AskRequest { question, options },
    ))
    .await
    {
        return None;
    }

    // Await the user's response with a 120 s timeout and cancellation
    // support.  `select!` races the mpsc receiver against a deadline and a
    // cancel-token poller so that the user's answer, the timeout, or the
    // Stop button can each break the wait promptly — no busy-polling.
    Some(tokio::select! {
        answer = ask_receiver.recv() => match answer {
            Some(answer) => answer,
            None => Err("Ask response channel closed.".into()),
        },
        _ = tokio::time::sleep(std::time::Duration::from_secs(120)) => {
            Ok("User did not respond before the timeout.".into())
        }
        _ = wait_cancelled(cancel_token) => {
            Err("Cancelled by user.".into())
        }
    })
}
