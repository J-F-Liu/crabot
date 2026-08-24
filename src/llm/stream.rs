//! LLM stream lifecycle: acquisition, event forwarding, the stall watchdog,
//! and transient-failure retry classification.

use futures::{StreamExt, future::BoxFuture};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use genai::Client;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStream, ChatStreamEvent,
    MessageContent,
};
use reqwest::StatusCode;

use crate::app::session_state::{RetryInfo, SessionEvent};
use crabot::chat::assistant_msg_is_empty;
use crabot::model::ModelInfo;

/// Seconds to wait between auto-retry attempts after a transient failure.
const RETRY_DELAY_SECS: u32 = 60;
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

/// Whether an HTTP status warrants an auto-retry (429 rate limit / 5xx server error).
fn is_retryable_status(status: StatusCode) -> bool {
    status.as_u16() == 429 || (500..600).contains(&status.as_u16())
}

/// Whether a reqwest error is a transport-level failure worth retrying.
/// Even deterministic failures (request-build, decode) share this bounded retry path.
fn is_retryable_reqwest(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() || e.is_decode()
}

/// Classify a genai error as transient (429 / 5xx / transport failure).
/// Statuses buried in `ChatResponseGeneration`/`ChatResponse` bodies aren't classified.
pub(super) fn is_retryable(e: &genai::Error) -> bool {
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
    Finished { msg: ChatMessage },
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

/// Wrap a completed stream's capture into an assistant message; an empty
/// response becomes a retryable `Empty` outcome instead of a finished turn.
fn attempt_finished(
    ctx: &StreamCtx<'_>,
    content: Option<MessageContent>,
    reasoning: Option<String>,
) -> AttemptOutcome {
    let msg =
        ChatMessage::assistant(content.unwrap_or_else(|| MessageContent::from_text(String::new())))
            .with_reasoning_content(reasoning.filter(|r| !r.is_empty()));
    if assistant_msg_is_empty(&msg) {
        tracing::info!(
            model = %ctx.model.model_id,
            session = %ctx.session_id,
            "received empty assistant message"
        );
        AttemptOutcome::Empty
    } else {
        AttemptOutcome::Finished { msg }
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
                if !on_event(SessionEvent::TokenUsage(end.captured_usage)).await {
                    return AttemptOutcome::Cancelled;
                }
                return attempt_finished(ctx, end.captured_content, end.captured_reasoning_content);
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

/// Warn, then count down `RETRY_DELAY_SECS` one second at a time so Stop
/// stays responsive. False = cancelled.
pub(super) async fn pause_before_retry(
    attempt: u32,
    model_id: &str,
    reason: &str,
    on_event: &mut (dyn FnMut(SessionEvent) -> BoxFuture<'static, bool> + Send),
    cancel_token: &CancellationToken,
) -> bool {
    tracing::warn!(
        attempt,
        model = %model_id,
        error = %reason,
        "transient LLM failure, retrying in {RETRY_DELAY_SECS}s"
    );
    for seconds_left in (1..=RETRY_DELAY_SECS).rev() {
        if !on_event(SessionEvent::RetryCountdown(RetryInfo {
            attempt: attempt + 1,
            max_attempts: MAX_ATTEMPTS,
            seconds_left,
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
