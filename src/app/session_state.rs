use std::borrow::Cow;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::Task;
use iced::widget::scrollable::Viewport;
use tokio::sync::mpsc;

use crate::app::{SessionEndStatus, SessionTab};
use crate::llm::DialogPhase;
use crate::model::Cost;
use crate::model::TokenAmount;
use crate::views::ASK_INPUT;
use crate::views::scroll_to_end;
use crabot::chat::{TextContent, ToolCall, ToolResult, Turn, TurnBody, replace_emoji};
use crabot::session::Session;
use crabot::user::UserPrompt;
use genai::chat::{ChatMessage, ChatRole};

/// Minimum context-window size (tokens) for which the auto-injected renew hint is eligible.
const MIN_CW_FOR_RENEW_HINT: u32 = 1_000_000;

/// Streaming session state bundled together for the LLM interaction lifecycle.
#[derive(Debug)]
pub(crate) struct SessionState {
    /// Current phase of the LLM interaction.
    pub(crate) phase: DialogPhase,
    /// Index (flat turn count) where the current stream's placeholders begin.
    pub(crate) start_index: usize,
    /// Cancellation token to stop an in-progress stream early.
    pub(crate) cancel_token: Arc<AtomicBool>,
    /// Shared slot for a raw user prompt injected during streaming.
    pub(crate) injected_prompt: Arc<Mutex<Option<String>>>,
    /// Parked full `UserPrompt` (work mode, workspace tree) to be dispatched
    /// later when this tab is no longer blocked by another tab's stream.
    pub(crate) pending_prompt: Option<UserPrompt>,
    /// Active ask-tool request shown in the tool turn.
    pub(crate) ask_request: Option<AskRequest>,
    pub(crate) ask_input: String,
    /// Sender for the builtin ask tool — the UI calls `send()` to deliver
    /// the user's response to the streaming task's receiver.
    pub(crate) ask_sender: Option<mpsc::UnboundedSender<Result<String, String>>>,
    /// Sender for task-tool reports, tagged with the originating call_id so
    /// parallel task calls can be correlated.
    pub(crate) task_sender: Option<mpsc::UnboundedSender<(String, Result<String, String>)>>,
    /// Whether to auto-scroll the message view to the bottom during streaming.
    pub(crate) auto_scroll: Arc<AtomicBool>,
    /// Timestamp of the last auto-scroll snap, throttled to avoid jitter.
    pub(crate) scroll_throttle: Cell<Instant>,
    /// Cooldown counter for renew hints — only inject every N ToolExecuting phases.
    pub(crate) renew_hint_cooldown: Cell<u32>,
    /// Auto-retry countdown after a transient LLM failure (429/5xx/connection).
    pub(crate) retry: Option<RetryInfo>,
}

impl SessionState {
    /// Create a fresh session state.
    pub(crate) fn new() -> Self {
        Self {
            phase: DialogPhase::Idle,
            start_index: 0,
            cancel_token: Arc::new(AtomicBool::new(false)),
            injected_prompt: Arc::new(Mutex::new(None)),
            pending_prompt: None,
            ask_request: None,
            ask_input: String::new(),
            ask_sender: None,
            task_sender: None,
            auto_scroll: Arc::new(AtomicBool::new(true)),
            scroll_throttle: Cell::new(Instant::now()),
            renew_hint_cooldown: Cell::new(0),
            retry: None,
        }
    }

    /// Signal this session to stop streaming.
    pub(crate) fn stop(&self) {
        self.cancel_token.store(true, Ordering::Release);
    }

    /// Store the raw content of a user prompt into the shared lock for
    /// interrupt injection into the ongoing stream on this tab.
    pub(crate) fn inject_prompt(&mut self, prompt: UserPrompt) {
        if let Ok(mut pending) = self.injected_prompt.lock() {
            *pending = Some(prompt.content.clone());
        }
        // for display on UI
        self.pending_prompt = Some(prompt);
    }

    /// Human-readable status label for the current streaming phase.
    pub(crate) fn status(&self, session_empty: bool) -> Cow<'static, str> {
        // While auto-retrying, surface the countdown in the status bar.
        if let Some(retry) = self.retry {
            return Cow::Owned(format!(
                "Retry in {} second{} ({}/{})",
                retry.seconds_left,
                if retry.seconds_left == 1 { "" } else { "s" },
                retry.attempt,
                retry.max_attempts
            ));
        }
        match self.phase {
            DialogPhase::LlmLoading => Cow::Borrowed("⏳ Loading LLM…"),
            DialogPhase::LlmThinking => Cow::Borrowed("💭 LLM thinking…"),
            DialogPhase::ToolExecuting => Cow::Borrowed("🛠️ Tool executing…"),
            DialogPhase::Idle => {
                if session_empty {
                    Cow::Borrowed("Send user prompt to start dialog with LLM")
                } else {
                    Cow::Borrowed("✅ Ready")
                }
            }
        }
    }
}

/// Auto-retry countdown shown in the status bar.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryInfo {
    /// Attempt that runs after the countdown.
    pub(crate) attempt: u32,
    /// Total attempts (initial request + retries).
    pub(crate) max_attempts: u32,
    /// Seconds remaining until the next attempt.
    pub(crate) seconds_left: u32,
}

/// Request displayed by the builtin ask tool.
#[derive(Debug, Clone)]
pub(crate) struct AskRequest {
    pub question: String,
    pub options: Vec<String>,
}

/// Action taken from the builtin ask tool UI controls.
#[derive(Debug, Clone)]
pub(crate) enum AskAction {
    /// User submitted an answer (text read from `ask_input`).
    Ok,
    /// User chose to skip the question.
    Skip,
    /// User selected one of the provided options.
    OptionSelected(String),
}

/// Task-tool spawn request — defined in the lib crate next to the tool.
pub(crate) use crabot::tools::TaskRequest;

/// Events emitted from the streaming runtime channel.
#[derive(Debug, Clone)]
pub(crate) enum SessionEvent {
    ToolCalls(Vec<ToolCall>),
    AskRequest(AskRequest),
    /// Prompt string for creating a new session to continue the task.
    RenewRequest(String),
    /// Spawn a sub-agent session; its final report answers the tool call.
    TaskRequest(TaskRequest),
    Content(String),
    Reasoning(String),
    ToolResult(ToolResult),
    /// A user prompt injected during streaming (consumed by `send_stream`).
    UserPrompt(String),
    TokenUsage(Option<genai::chat::Usage>),
    /// Auto-retry countdown after a transient failure (429/5xx/connection).
    RetryCountdown(RetryInfo),
    Done(Vec<ChatMessage>),
    Error(String, Vec<ChatMessage>),
    Cancelled(Vec<ChatMessage>),
    PhaseChange(DialogPhase),
    Stop,
}

/// Handle a streaming event for a specific tab.
pub(crate) fn update(
    event: SessionEvent,
    tab: &mut SessionTab,
    model_cost: Option<Cost>,
    context_window: Option<u32>,
    fill_ratio_threshold: f32,
    viewing: bool,
) -> Task<()> {
    let SessionTab {
        session_state,
        session,
        search,
        latest_tokens,
        expanded_dialogs,
        end_status,
        task_path,
        ..
    } = tab;
    let state: &mut SessionState = session_state;

    // Any non-countdown event resumes/ends streaming — clear the retry countdown.
    if !matches!(&event, SessionEvent::RetryCountdown(_)) {
        state.retry = None;
    }

    match event {
        SessionEvent::RetryCountdown(retry) => {
            state.retry = Some(retry);
            return Task::none();
        }
        SessionEvent::ToolCalls(tcs) => {
            session.push_turn(Turn::from_tool_results(vec![]));
            session.push_turn(Turn::from_tool_calls(tcs));
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::AskRequest(request) => {
            let no_options = request.options.is_empty();
            state.ask_request = Some(request);
            state.ask_input.clear();
            if no_options && viewing {
                return iced::widget::operation::focus(ASK_INPUT.clone());
            }
        }
        SessionEvent::Content(chunk) => {
            if let Some(last) = session.last_turn_mut()
                && let TurnBody::Text(tc) = &mut last.body
            {
                tc.content.push_str(&chunk);
            }
            return if viewing {
                maybe_scroll_to_end_throttled(&state.auto_scroll, &state.scroll_throttle)
            } else {
                Task::none()
            };
        }
        SessionEvent::Reasoning(chunk) => {
            if let Some(last) = session.last_turn_mut()
                && let TurnBody::Text(tc) = &mut last.body
            {
                tc.reasoning
                    .get_or_insert_with(String::new)
                    .push_str(&chunk);
            }
            return if viewing {
                maybe_scroll_to_end_throttled(&state.auto_scroll, &state.scroll_throttle)
            } else {
                Task::none()
            };
        }
        SessionEvent::ToolResult(tr) => {
            // Clear the ask UI when the ask tool completes (covers both
            // user-response and timeout paths).
            if tr.name == "ask" {
                state.ask_request = None;
            }
            if let Some(path_str) = tr.get_modified_file()
                && !session.modified_files.iter().any(|p| p == path_str)
            {
                session.modified_files.push(path_str.to_string());
            }
            if let Some(dialog) = session.dialogs.last_mut() {
                dialog.push_tool_result(tr);
            }
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::UserPrompt(content) => {
            // Take the mode from the pending prompt before clearing it.
            let mode = state.pending_prompt.as_ref().and_then(|p| p.mode);
            // User message always create a new dialog.
            session.add_dialog(Session::derive_title(&content), mode);
            expanded_dialogs.insert(session.dialogs.len() - 1);
            session.push_turn(Turn::user(content));
            state.pending_prompt = None;
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::TokenUsage(usage) => {
            let u = usage.unwrap_or_default();
            let tokens = TokenAmount::from_genai(&u);
            session.accumulate_tokens(&tokens, model_cost);
            *latest_tokens = tokens;
            // Refresh the markdown cache after all chunks are collected.
            if let Some(last) = session.last_turn_mut()
                && let TurnBody::Text(tc) = &mut last.body
            {
                tc.refresh_md_cache();
            }
        }
        SessionEvent::Done(genai_messages) => {
            state.ask_request = None;
            *end_status = Some(SessionEndStatus::Done);
            handle_stream_done(state, session, genai_messages);
            search.invalidate_offsets();
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::Error(err, genai_messages) => {
            state.ask_request = None;
            *end_status = Some(SessionEndStatus::Error);
            handle_stream_error(state, session, err, genai_messages);
            search.invalidate_offsets();
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::Cancelled(genai_messages) => {
            state.ask_request = None;
            *end_status = Some(SessionEndStatus::Cancelled);
            state.phase = DialogPhase::Idle;
            clear_pending_with_notice(state, session);
            session.history.extend(genai_messages);
            if let Some(last) = session.last_turn_mut()
                && let TurnBody::Text(tc) = &mut last.body
            {
                tc.refresh_md_cache();
            }
            search.invalidate_offsets();
            let _ = session.save();
        }
        SessionEvent::PhaseChange(phase) => {
            if phase == DialogPhase::LlmThinking {
                session.push_turn(Turn::assistant(String::new(), None));
                search.invalidate_offsets();
                // Back-date so the first content chunk scrolls immediately.
                state.scroll_throttle.set(Instant::now() - SCROLL_THROTTLE);
            } else if phase == DialogPhase::ToolExecuting {
                session.stamp_response();
                // ToolExecuting means LLM conversation still not finished.
                // When context fill ratio exceeds the threshold, suggest the LLM renew.
                // Only inject if no prompt is already pending to avoid overwriting an existing one.
                // Sub-agent sessions have the renew tool stripped, so skip the hint there.
                let cooldown = state.renew_hint_cooldown.get();
                if cooldown > 0 {
                    state.renew_hint_cooldown.set(cooldown - 1);
                } else if task_path.is_none()
                    && state.pending_prompt.is_none()
                    && let Some(cw) = context_window
                    && cw >= MIN_CW_FOR_RENEW_HINT
                    && latest_tokens.context_fill_ratio(cw) >= fill_ratio_threshold
                {
                    let prompt = UserPrompt::new(
                        session.dialogs.last().and_then(|d| d.mode),
                        "Context fill ratio is near its limit, consider calling the renew tool to continue current task."
                            .into(),
                        None,
                    );
                    state.inject_prompt(prompt);
                    state.renew_hint_cooldown.set(5);
                }
            }
            state.phase = phase;
        }
        SessionEvent::Stop => {
            state.stop();
        }
        // RenewRequest / TaskRequest are intercepted in conversation.rs before reaching here.
        SessionEvent::RenewRequest(_) | SessionEvent::TaskRequest(_) => {
            unreachable!("handled in conversation::session_event")
        }
    }
    Task::none()
}

/// Handle session-view scroll tracking — while streaming, toggle auto-scroll
/// based on whether the user has scrolled away from / back to the bottom.
pub(crate) fn handle_scroll(state: &SessionState, viewport: Viewport) {
    if state.phase != DialogPhase::Idle {
        let y = viewport.relative_offset().y;
        let at_bottom = if y.is_nan() { true } else { y >= 0.99 };
        state.auto_scroll.store(at_bottom, Ordering::Relaxed);
    }
}

// ── private helpers ───────────────────────────────────────────────

/// Clear and report injected prompt after a stream ended without consuming it.
fn clear_pending_with_notice(state: &mut SessionState, session: &mut Session) {
    if let Ok(mut pending) = state.injected_prompt.lock()
        && let Some(prompt) = pending.take()
    {
        let msg = format!(
            "⚠️ Stream ended — the following prompt was **not executed**:\n\n> {}",
            prompt
        );
        session.push_turn(Turn::assistant(msg, None));
    }
    state.pending_prompt = None;
}

/// Minimum interval between auto-scroll snaps during streaming.
const SCROLL_THROTTLE: Duration = Duration::from_millis(500);

fn maybe_scroll_to_end(auto_scroll: &AtomicBool) -> Task<()> {
    if auto_scroll.load(Ordering::Relaxed) {
        scroll_to_end()
    } else {
        Task::none()
    }
}

/// Throttled variant for scroll to end, preventing jitter from rapid-fire updates.
fn maybe_scroll_to_end_throttled(auto_scroll: &AtomicBool, last: &Cell<Instant>) -> Task<()> {
    if !auto_scroll.load(Ordering::Relaxed) {
        return Task::none();
    }
    let now = Instant::now();
    if now.duration_since(last.get()) < SCROLL_THROTTLE {
        return Task::none();
    }
    last.set(now);
    scroll_to_end()
}

/// Backfill streaming placeholders with captured content from genai,
/// extend session history, and persist the session.
fn handle_stream_done(
    state: &mut SessionState,
    session: &mut Session,
    genai_messages: Vec<ChatMessage>,
) {
    state.phase = DialogPhase::Idle;

    let mut genai_asst_iter = genai_messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant)
        .filter_map(|m| {
            let text = m.content.joined_texts().unwrap_or_default();
            let reasoning = m.content.first_reasoning_content().map(|s| s.to_string());
            if !text.is_empty() || reasoning.is_some() {
                Some((text, reasoning))
            } else {
                None
            }
        });

    for turn in session.turns_from_mut(state.start_index) {
        if turn.role != ChatRole::Assistant {
            continue;
        }
        if let TurnBody::Text(tc) = &mut turn.body
            && let Some((joined_text, reasoning)) = genai_asst_iter.next()
        {
            if !joined_text.is_empty() {
                tc.content = replace_emoji(&joined_text);
            }
            // Some providers omit ReasoningChunk events and only expose
            // reasoning via captured_reasoning_content at stream end.
            if tc.reasoning.is_none() {
                tc.reasoning = reasoning;
            }
            tc.refresh_md_cache();
        }
    }

    session.history.extend(genai_messages);
    session.stamp_response();
    let _ = session.save();
}

/// Replace the last-message empty assistant placeholder with this error,
/// or push a new error message if no placeholder exists.
fn handle_stream_error(
    state: &mut SessionState,
    session: &mut Session,
    err: String,
    genai_messages: Vec<ChatMessage>,
) {
    state.phase = DialogPhase::Idle;
    session.history.extend(genai_messages);
    session.stamp_response();
    let _ = session.save();

    let error_msg = format!("Error: {err}");
    if let Some(turn) = session.last_turn_mut()
        && turn.role == ChatRole::Assistant
        && matches!(
            &turn.body,
            TurnBody::Text(TextContent { content, reasoning: None, .. }) if content.is_empty()
        )
    {
        turn.body = TurnBody::Text(TextContent {
            content: error_msg,
            ..Default::default()
        });
    } else {
        session.push_turn(Turn::assistant(error_msg, None));
    }
    clear_pending_with_notice(state, session);
}
