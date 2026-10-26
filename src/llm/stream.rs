//! LLM stream lifecycle: acquisition, event forwarding, the stall watchdog,
//! and transient-failure retry classification.

use futures::{StreamExt, future::BoxFuture};
use std::borrow::Cow;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use genai::Client;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStream, ChatStreamEvent,
    MessageContent, StopReason, StreamEnd,
};
use reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::app::session_state::{RetryInfo, SessionEvent};
use crabot::chat::assistant_msg_is_empty;
use crabot::model::ModelInfo;

/// Default wait between auto-retry attempts after a transient failure.
pub(super) const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(60);
/// Longest provider-requested delay we wait automatically.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);
/// Milliseconds variant of `Retry-After`, supported by OpenAI-style providers.
const RETRY_AFTER_MS: &str = "retry-after-ms";
/// Provider hint that overrides the status-code retry heuristic.
const X_SHOULD_RETRY: &str = "x-should-retry";
/// Max request attempts per turn (initial request + retries).
pub(super) const MAX_ATTEMPTS: u32 = 5;

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

// ── Cache management ───────────────────────────────────────────────

/// Move the rolling ephemeral cache breakpoint to the tail message.
/// Only touches `CacheControl::Ephemeral`; leaves other TTLs (e.g. `Ephemeral1h`) intact.
pub(super) fn mark_cache_tail(messages: &mut [ChatMessage]) {
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

// ── Retry classification ───────────────────────────────────────────

/// Whether an error code warrants an auto-retry (429 rate limit / 5xx server error).
fn is_retryable_code(code: u64) -> bool {
    code == 429 || (500..600).contains(&code)
}

/// Whether a transport failure is worth retrying.
fn is_retryable_reqwest(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() || e.is_decode()
}

/// Whether an SSE `error` payload carries a retryable code.
/// The code may sit at the top level or under `error`, as a number or a string.
fn is_retryable_body(body: &serde_json::Value) -> bool {
    body_error_code(body)
        .and_then(|code| code.trim().parse().ok())
        .is_some_and(is_retryable_code)
}

/// What the retry layer should do with a transient failure.
pub(super) enum RetryHint {
    /// `delay` is the provider's requested wait, if any.
    Retry { delay: Option<Duration> },
    /// The provider asked for a wait longer than we allow.
    TooLong { delay: Duration },
}

/// Classify a failure and read the provider's retry guidance.
/// Header hints override status heuristics; `retry-after-ms` wins over `retry-after`.
pub(super) fn retry_hint(e: &genai::Error) -> Option<RetryHint> {
    // Stream errors may wrap the real HTTP failure; keep its headers/delay.
    if let genai::Error::WebStream { error, .. } = e
        && let Some(inner) = error.downcast_ref::<genai::Error>()
    {
        return retry_hint(inner);
    }
    let headers = e.headers();
    // `x-should-retry` overrides the status heuristic in both directions.
    let retryable = match headers.and_then(should_retry_header) {
        Some(decision) => decision,
        None => match e.status() {
            Some(status) => is_retryable_code(u64::from(status.as_u16())),
            None => is_transient(e),
        },
    };
    if !retryable {
        return None;
    }
    // A `Retry-After` header only picks the wait; it never makes a failure retryable.
    match headers.and_then(retry_after) {
        Some(delay) if delay > MAX_RETRY_DELAY => Some(RetryHint::TooLong { delay }),
        Some(delay) => Some(RetryHint::Retry { delay: Some(delay) }),
        None => Some(RetryHint::Retry { delay: None }),
    }
}

/// `Retry-After` in milliseconds, then seconds, then HTTP-date.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    if let Some(ms) = header_value(headers, RETRY_AFTER_MS).and_then(|v| v.parse::<u64>().ok()) {
        return Some(Duration::from_millis(ms));
    }
    let value = header_value(headers, RETRY_AFTER.as_str())?;
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let secs = (when.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64;
    Some(Duration::from_secs(secs))
}

/// Read a header value as trimmed UTF-8 text.
fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

/// Parse `x-should-retry`; unknown values fall back to the status heuristic.
fn should_retry_header(headers: &HeaderMap) -> Option<bool> {
    match header_value(headers, X_SHOULD_RETRY)?
        .to_ascii_lowercase()
        .as_str()
    {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Transport-level and SSE-body failures with no HTTP status to inspect.
fn is_transient(e: &genai::Error) -> bool {
    match e {
        genai::Error::ChatResponse { body, .. } => is_retryable_body(body),
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => match webc_error {
            genai::webc::Error::Reqwest(re) => is_retryable_reqwest(re),
            _ => false,
        },
        genai::Error::WebStream { error, .. } => {
            // Nested genai errors are unwrapped by `retry_hint`.
            if let Some(reqwest_err) = error.downcast_ref::<reqwest::Error>() {
                return is_retryable_reqwest(reqwest_err);
            }
            error.downcast_ref::<genai::webc::StreamTimeout>().is_some()
        }
        _ => false,
    }
}

// ── Failure status ─────────────────────────────────────────────────

/// Short failure label: HTTP status, transport error, or provider error code.
pub(super) fn failure_status(error: &genai::Error) -> Cow<'static, str> {
    // Stream errors may wrap the real failure; label the inner error.
    if let genai::Error::WebStream { error: inner, .. } = error
        && let Some(nested) = inner.downcast_ref::<genai::Error>()
    {
        return failure_status(nested);
    }
    if let Some(status) = error.status() {
        return match status.canonical_reason() {
            Some(reason) => Cow::Owned(format!("{} {reason}", status.as_u16())),
            None => Cow::Owned(format!("HTTP {}", status.as_u16())),
        };
    }
    if let Some(label) = transport_label(error) {
        return Cow::Borrowed(label);
    }
    if let genai::Error::ChatResponse { body, .. } = error
        && let Some(code) = body_error_code(body)
    {
        return Cow::Owned(code);
    }
    Cow::Borrowed("request failed")
}

/// Timeouts and connection failures carry no HTTP status.
fn transport_label(error: &genai::Error) -> Option<&'static str> {
    fn reqwest_label(e: &reqwest::Error) -> Option<&'static str> {
        if e.is_connect() {
            Some("connection error")
        } else if e.is_timeout() {
            Some("response timeout")
        } else {
            None
        }
    }
    match error {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => match webc_error {
            genai::webc::Error::Reqwest(e) => reqwest_label(e),
            _ => None,
        },
        genai::Error::WebStream { error: inner, .. } => {
            if inner.downcast_ref::<genai::webc::StreamTimeout>().is_some() {
                return Some("response timeout");
            }
            inner
                .downcast_ref::<reqwest::Error>()
                .and_then(reqwest_label)
        }
        _ => None,
    }
}

/// Provider error code carried at the top level or under `error`.
fn body_error_code(body: &serde_json::Value) -> Option<String> {
    match body.get("code").or_else(|| body.pointer("/error/code"))? {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

// ── Streaming attempt ──────────────────────────────────────────────

/// Loop-invariant inputs shared by every streaming attempt.
pub(super) struct StreamCtx<'a> {
    pub(super) client: &'a Client,
    pub(super) model: &'a ModelInfo,
    pub(super) session_id: &'a str,
    pub(super) chat_options: &'a ChatOptions,
    pub(super) cancel_token: &'a CancellationToken,
}

/// A successfully established LLM stream plus its first event (if any).
struct AcquiredStream {
    stream: ChatStream,
    /// First event already pulled from the stream, if any.
    first: Option<ChatStreamEvent>,
}

/// Where a failed acquisition originated — used to label the error message.
pub(super) enum AcquireStage {
    /// `exec_chat_stream` failed while setting up the request.
    Setup,
    /// The first stream poll failed (HTTP status / connection error).
    FirstPoll,
}

/// Establish the stream, racing the request and first poll against cancellation.
/// Returns `Ok(None)` if cancelled, `Err((stage, e))` on failure.
async fn try_acquire_stream(
    ctx: &StreamCtx<'_>,
    chat_req: &ChatRequest,
) -> Result<Option<AcquiredStream>, (AcquireStage, genai::Error)> {
    let stream_result = tokio::select! {
        biased;
        _ = ctx.cancel_token.cancelled() => return Ok(None),
        res = ctx.client.exec_chat_stream(&ctx.model.model_id, chat_req.clone(), Some(ctx.chat_options)) => res,
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
            _ = ctx.cancel_token.cancelled() => return Ok(None),
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

/// Outcome of one streaming attempt.
pub(super) enum AttemptOutcome {
    /// Stream finished; the captured assistant message (text, tool calls, reasoning).
    Finished { msg: ChatMessage, truncated: bool },
    /// Stream finished but captured nothing (no text, reasoning, or tool calls);
    /// retried by the caller like a transient failure.
    Empty,
    /// Cancelled by the user or the UI channel closing.
    Cancelled,
    /// Failure; `stage` is `None` for mid-stream errors.
    Failed {
        stage: Option<AcquireStage>,
        error: genai::Error,
    },
    /// Stall watchdog fired; retried by the caller like a transient failure.
    Stalled,
}

/// Rebuild the assistant message from a completed stream without duplicating
/// provider continuation metadata.
fn finished_message(end: StreamEnd) -> ChatMessage {
    let has_tool_calls = end
        .captured_content
        .as_ref()
        .is_some_and(|content| !content.tool_calls().is_empty());
    if has_tool_calls {
        // Provider handoff helper: keeps thought signatures and paired reasoning
        // blocks in content, and never appends the reasoning a second time.
        return end
            .into_assistant_message_for_tool_use()
            .expect("tool calls imply captured content");
    }

    let StreamEnd {
        captured_content,
        captured_reasoning_content,
        ..
    } = end;
    let mut msg = ChatMessage::assistant(
        captured_content.unwrap_or_else(|| MessageContent::from_text(String::new())),
    );
    // Duplicating paired reasoning breaks Anthropic's signature pairing.
    if !msg.content.contains_reasoning_content() {
        msg = msg.with_reasoning_content(captured_reasoning_content.filter(|r| !r.is_empty()));
    }
    msg
}

/// Wrap a completed stream's capture into an assistant message; an empty
/// response becomes a retryable `Empty` outcome instead of a finished turn.
fn attempt_finished(ctx: &StreamCtx<'_>, end: StreamEnd) -> AttemptOutcome {
    let truncated = end
        .captured_stop_reason
        .as_ref()
        .is_some_and(|reason| matches!(reason, StopReason::MaxTokens(_)));
    let msg = finished_message(end);
    if assistant_msg_is_empty(&msg) {
        tracing::info!(
            model = %ctx.model.model_id,
            session = %ctx.session_id,
            "received empty assistant message"
        );
        AttemptOutcome::Empty
    } else {
        AttemptOutcome::Finished { msg, truncated }
    }
}

/// Terminal error message for a failed attempt, labeled by where it surfaced.
pub(super) fn failure_message(
    stage: Option<AcquireStage>,
    error: &genai::Error,
    attempt: u32,
) -> String {
    let attempts = format!("{attempt} attempt{}", if attempt == 1 { "" } else { "s" });
    match stage {
        Some(AcquireStage::Setup) => {
            format!("Failed to start the LLM request after {attempts}: {error}")
        }
        Some(AcquireStage::FirstPoll) => {
            format!("The LLM request failed after {attempts}: {error}")
        }
        // Mid-stream failures keep the bare format.
        None => format!("stream error: {error}"),
    }
}

/// Emit the thinking phase once, then `event`; false = cancelled.
async fn emit_chunk(
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
    thinking_signaled: &mut bool,
    event: SessionEvent,
) -> bool {
    if !*thinking_signaled {
        *thinking_signaled = true;
        if !on_event(SessionEvent::PhaseChange(DialogPhase::LlmThinking)).await {
            return false;
        }
    }
    on_event(event).await
}

/// Sleep until `deadline`, or wait forever when the stall watchdog is off.
async fn stall_sleep(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending::<()>().await,
    }
}

/// Run one streaming attempt: acquire, then forward events until End, failure,
/// or cancellation. The caller owns the session history and terminal events.
pub(super) async fn stream_attempt(
    ctx: &StreamCtx<'_>,
    chat_req: &ChatRequest,
    stall_timeout_secs: u64,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
) -> AttemptOutcome {
    let (mut stream, mut pending_event) = match try_acquire_stream(ctx, chat_req).await {
        Ok(Some(acquired)) => (acquired.stream, acquired.first),
        Ok(None) => return AttemptOutcome::Cancelled,
        Err((stage, error)) => {
            return AttemptOutcome::Failed {
                stage: Some(stage),
                error,
            };
        }
    };

    // Stall watchdog: Anthropic heartbeats every ~15-30s, so silence past
    // the window means the stream died. Any event resets the deadline.
    let stall_timeout = Duration::from_secs(stall_timeout_secs);
    let mut stall_deadline =
        (stall_timeout > Duration::ZERO).then(|| Instant::now() + stall_timeout);
    let mut thinking_signaled = false;

    // First event was already pulled; race the rest against cancellation
    // and the stall watchdog.
    loop {
        let event = match pending_event.take() {
            Some(event) => Some(Ok(event)),
            None => tokio::select! {
                // Biased so ties resolve deterministically: cancel > stream data > stall timeout.
                biased;
                _ = ctx.cancel_token.cancelled() => return AttemptOutcome::Cancelled,
                ev = stream.next() => ev,
                _ = stall_sleep(stall_deadline) => return AttemptOutcome::Stalled,
            },
        };
        let Some(event) = event else { break };
        // Any event is proof of life — reset the stall deadline.
        stall_deadline = stall_deadline.map(|_| Instant::now() + stall_timeout);

        // Forward content/reasoning chunks; End returns the captured content.
        let session_event = match event {
            // Skip empty chunks, so a UI placeholder isn't created for them.
            Ok(ChatStreamEvent::Chunk(chunk)) if !chunk.content.is_empty() => {
                Some(SessionEvent::Content(chunk.content))
            }
            Ok(ChatStreamEvent::ReasoningChunk(chunk)) if !chunk.content.is_empty() => {
                Some(SessionEvent::Reasoning(chunk.content))
            }
            Ok(ChatStreamEvent::End(end)) => {
                tracing::debug!(
                    model = %ctx.model.model_id,
                    "LLM stream ended, usage: {:?}",
                    end.captured_usage
                );
                let usage = end.captured_usage.clone();
                if !on_event(SessionEvent::TokenUsage(usage)).await {
                    return AttemptOutcome::Cancelled;
                }
                return attempt_finished(ctx, end);
            }
            // Ignore Start, Heartbeat, ThoughtSignature, ToolCallChunk, empty chunks.
            Ok(_) => None,
            Err(error) => {
                return AttemptOutcome::Failed { stage: None, error };
            }
        };
        let Some(session_event) = session_event else {
            continue;
        };
        if !emit_chunk(on_event, &mut thinking_signaled, session_event).await {
            return AttemptOutcome::Cancelled;
        }
    }
    // Stream closed without an End event — nothing was captured.
    AttemptOutcome::Empty
}

// ── Retry pause ────────────────────────────────────────────────────

/// Count down `delay` one second at a time so Stop stays responsive.
pub(super) async fn pause_before_retry(
    attempt: u32,
    delay: Duration,
    model_id: &str,
    reason: &str,
    status: Cow<'static, str>,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
    cancel_token: &CancellationToken,
) -> bool {
    // The countdown UI works in whole seconds.
    let delay_secs = delay.as_millis().div_ceil(1000).clamp(1, u32::MAX as u128) as u32;
    tracing::warn!(
        attempt,
        model = %model_id,
        error = %reason,
        "transient LLM failure, retrying in {delay_secs}s"
    );
    for seconds_left in (1..=delay_secs).rev() {
        if !on_event(SessionEvent::RetryCountdown(RetryInfo {
            attempt: attempt + 1,
            max_attempts: MAX_ATTEMPTS,
            seconds_left,
            status: status.clone(),
        }))
        .await
        {
            return false;
        }
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => return false,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
    // Countdown finished — clear the stale countdown status before the next attempt.
    on_event(SessionEvent::PhaseChange(DialogPhase::LlmLoading)).await
}
