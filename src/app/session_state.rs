use std::borrow::Cow;
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::Task;
use iced::widget::scrollable::Viewport;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app::attention;
use crate::app::conversation::{dispatch_pending, switch_tab};
use crate::app::{App, ConversationEvent, Message, SessionEndStatus, SessionTab};
use crate::llm::DialogPhase;
use crate::model::{Cost, ModelConfig, TokenAmount};
use crate::tools::TASK_MODES;
use crate::views::ASK_INPUT;
use crate::views::scroll_to_end;
use crate::views::search_bar::SearchState;
use crabot::HashSetExt;
use crabot::chat::{TextContent, ToolCall, ToolResult, Turn, TurnBody, replace_emoji};
use crabot::lock;
use crabot::session::Session;
use crabot::user::{UserPrompt, WorkMode};
use genai::chat::{ChatMessage, ChatRole};

/// Minimum context-window size (tokens) for which the auto-injected renew hint is eligible.
const MIN_CW_FOR_RENEW_HINT: u32 = 1_000_000;
/// Seconds the builtin ask tool waits for user input before timing out.
pub(crate) const ASK_TIMEOUT_SECS: u64 = 120;
/// Seconds added to the ask deadline each time the user clicks "Extend".
pub(crate) const ASK_EXTEND_SECS: u64 = 300;

/// Streaming session state bundled together for the LLM interaction lifecycle.
#[derive(Debug)]
pub(crate) struct SessionState {
    /// Current phase of the LLM interaction.
    pub(crate) phase: DialogPhase,
    /// Flat turn index of the next assistant placeholder to backfill.
    pub(crate) backfill_from: usize,
    /// Cancellation token to stop an in-progress stream early.
    pub(crate) cancel_token: CancellationToken,
    /// Shared slot for a raw user prompt injected during streaming.
    pub(crate) injected_prompt: Arc<Mutex<Option<String>>>,
    /// Parked full `UserPrompt` (work mode, workspace tree) to be dispatched
    /// later when this tab is no longer blocked by another tab's stream.
    pub(crate) pending_prompt: Option<UserPrompt>,
    /// Active ask-tool request shown in the tool turn.
    pub(crate) ask_request: Option<AskRequest>,
    pub(crate) ask_input: String,
    /// Shared ask-tool deadline — the UI extends it while a question is pending.
    pub(crate) ask_deadline: Arc<Mutex<Instant>>,
    /// Seconds left on the active ask countdown (ticked by `AskCountdown`).
    pub(crate) ask_seconds_left: u64,
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
            backfill_from: 0,
            cancel_token: CancellationToken::new(),
            injected_prompt: Arc::new(Mutex::new(None)),
            pending_prompt: None,
            ask_request: None,
            ask_input: String::new(),
            ask_deadline: Arc::new(Mutex::new(Instant::now())),
            ask_seconds_left: 0,
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
        self.cancel_token.cancel();
    }

    /// Push the ask deadline back by `ASK_EXTEND_SECS` and refresh the
    /// countdown immediately instead of waiting for the next tick.
    pub(crate) fn extend_ask_deadline(&mut self) {
        let mut deadline = lock(&self.ask_deadline);
        *deadline += Duration::from_secs(ASK_EXTEND_SECS);
        self.ask_seconds_left = deadline.saturating_duration_since(Instant::now()).as_secs();
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
    /// User chose none of the provided options.
    NoneApply,
    /// User selected one of the provided options.
    OptionSelected(String),
    /// User extended the response deadline.
    Extend,
}

/// Task-tool spawn request — defined in the lib crate next to the tool.
pub(crate) use crabot::tools::TaskRequest;

/// Events emitted from the streaming runtime channel.
#[derive(Debug, Clone)]
pub(crate) enum SessionEvent {
    ToolCalls(Vec<ToolCall>),
    /// Paths snapshotted before tool execution — populate the right-pane Revert list.
    SnapshotsCaptured(Vec<String>),
    AskRequest(AskRequest),
    /// Seconds remaining on the active ask-tool deadline (ticked every second).
    AskCountdown(u64),
    /// Prompt string for creating a new session to continue the task.
    RenewRequest(String),
    /// Spawn a sub-agent session; its final report answers the tool call.
    TaskRequest(TaskRequest),
    Content(String),
    Reasoning(String),
    ToolResult(ToolResult),
    /// Incremental output chunk from a running tool; `call_id` matches the
    /// pending call in the `Temp` turn.
    ToolOutput {
        call_id: Option<String>,
        chunk: String,
    },
    /// A user prompt injected during streaming (consumed by `send_stream`).
    UserPrompt(String),
    /// A complete genai message — recorded in history and persisted immediately.
    MessageReady(ChatMessage),
    TokenUsage(Option<genai::chat::Usage>),
    /// Auto-retry countdown after a transient failure (429/5xx/connection).
    RetryCountdown(RetryInfo),
    Done,
    Error(String),
    Cancelled,
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
        expanded_turns,
        end_status,
        task_path,
        snapshot_files,
        modified_files_error,
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
            *modified_files_error = None;
            session.push_turn(Turn::from_tool_results(vec![]));
            session.push_turn(Turn::from_tool_calls(tcs));
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::SnapshotsCaptured(files) => {
            snapshot_files.extend(files);
        }
        SessionEvent::AskRequest(request) => {
            let no_options = request.options.is_empty();
            state.ask_request = Some(request);
            state.ask_input.clear();
            state.ask_seconds_left = ASK_TIMEOUT_SECS;
            if no_options && viewing {
                return iced::widget::operation::focus(ASK_INPUT.clone());
            }
        }
        SessionEvent::AskCountdown(seconds_left) => {
            state.ask_seconds_left = seconds_left;
            return Task::none();
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
            tr.track_modified_file(&mut session.modified_files);
            tr.track_read_file(&mut session.accessed_files);
            if let Some(dialog) = session.dialogs.last_mut() {
                dialog.push_tool_result(tr);
            }
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::ToolOutput { call_id, chunk } => {
            let Some(dialog) = session.dialogs.last_mut() else {
                return Task::none();
            };
            // Drops stale chunks; creates the placeholder on the first chunk.
            let Some((idx, created)) = dialog.push_tool_output(call_id.as_deref(), &chunk) else {
                return Task::none();
            };
            // First chunk reveals the live output; later chunks must not
            // re-expand what the user collapsed.
            if created {
                expanded_turns.set((session.total_turns() - 2, idx), true);
            }
            return if viewing {
                maybe_scroll_to_end_throttled(&state.auto_scroll, &state.scroll_throttle)
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
        SessionEvent::MessageReady(msg) => {
            if msg.role == ChatRole::System {
                // Audit only: never a UI turn; deduped on record.
                if let Some(text) = msg.content.joined_texts() {
                    session.record_system_prompt(&text);
                }
                return Task::none();
            }
            // Only non-empty assistant messages are emitted, so record as-is.
            backfill_assistant_turn(state, session, &msg);
            session.history.push(msg);
            let _ = session.save();
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
        SessionEvent::Done => {
            session.stamp_response();
            end_stream(state, session, end_status, search, SessionEndStatus::Done);
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::Error(err) => {
            record_error_turn(session, &err);
            // Stamp after the error turn so the Tally's updated_at covers it.
            clear_pending_with_notice(state, session);
            session.stamp_response();
            end_stream(state, session, end_status, search, SessionEndStatus::Error);
            return if viewing {
                maybe_scroll_to_end(&state.auto_scroll)
            } else {
                Task::none()
            };
        }
        SessionEvent::Cancelled => {
            clear_pending_with_notice(state, session);
            if let Some(last) = session.last_turn_mut()
                && let TurnBody::Text(tc) = &mut last.body
            {
                tc.refresh_md_cache();
            }
            end_stream(
                state,
                session,
                end_status,
                search,
                SessionEndStatus::Cancelled,
            );
        }
        SessionEvent::PhaseChange(phase) => {
            if phase == DialogPhase::LlmThinking {
                // A retry re-emits LlmThinking — replace the failed attempt's placeholder.
                replace_stale_placeholder(session, state.backfill_from);
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
        // RenewRequest / TaskRequest are intercepted earlier in session_event.
        SessionEvent::RenewRequest(_) | SessionEvent::TaskRequest(_) => {
            unreachable!("intercepted in session_event")
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

/// Wrap up a stream: clear the ask, set end status, persist, refresh search.
fn end_stream(
    state: &mut SessionState,
    session: &mut Session,
    end_status: &mut Option<SessionEndStatus>,
    search: &mut SearchState,
    status: SessionEndStatus,
) {
    state.ask_request = None;
    *end_status = Some(status);
    state.phase = DialogPhase::Idle;
    let _ = session.save_with_tally();
    search.invalidate_offsets();
}

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

/// Put the error into the stream's empty assistant placeholder, or push it fresh.
fn record_error_turn(session: &mut Session, err: &str) {
    let msg = format!("Error: {err}");
    if let Some(turn) = session.last_turn_mut()
        && turn.role == ChatRole::Assistant
        && matches!(
            &turn.body,
            TurnBody::Text(TextContent { content, reasoning: None, .. }) if content.is_empty()
        )
    {
        turn.body = TurnBody::Text(TextContent {
            content: msg,
            ..Default::default()
        });
        return;
    }
    session.push_turn(Turn::assistant(msg, None));
}

/// Fill the next stream placeholder with the captured assistant text/reasoning.
/// The slot is claimed even when empty so the next message targets the next one.
fn backfill_assistant_turn(state: &mut SessionState, session: &mut Session, msg: &ChatMessage) {
    if msg.role != ChatRole::Assistant {
        return;
    }
    let Some((offset, turn)) = session
        .turns_from_mut(state.backfill_from)
        .enumerate()
        .find(|(_, t)| t.role == ChatRole::Assistant)
    else {
        return;
    };
    state.backfill_from += offset + 1;
    let text = msg.content.joined_texts().unwrap_or_default();
    let reasoning = msg.content.first_reasoning_content().map(str::to_string);
    if text.is_empty() && reasoning.is_none() {
        return;
    }
    let TurnBody::Text(tc) = &mut turn.body else {
        return;
    };
    if !text.is_empty() {
        tc.content = replace_emoji(&text);
    }
    if tc.reasoning.is_none() {
        tc.reasoning = reasoning;
    }
    tc.refresh_md_cache();
}

/// Drop the failed attempt's stale placeholder so the retried response
/// backfills it. Stream turns only — a resend's last reply must stay put.
fn replace_stale_placeholder(session: &mut Session, backfill_from: usize) {
    if session.total_turns() > backfill_from
        && let Some(last) = session.last_turn_mut()
        && last.role == ChatRole::Assistant
        && matches!(&last.body, TurnBody::Text(_))
    {
        session.pop_last_turn();
    }
}

// ── App-level stream-event routing ─────────────────────────────────

/// The tab's effective model config: the session's model, or tab's selected model label.
fn tab_model_config(app: &App, tab: &SessionTab) -> Option<ModelConfig> {
    tab.session
        .model
        .clone()
        .or_else(|| app.models.get_config(&tab.selected_model).cloned())
}

/// The workspace a tab's session actually runs in: the session's stored
/// workspace, falling back to the current prompt workspace.
fn tab_workspace(app: &App, tab: &SessionTab) -> PathBuf {
    let ws = &tab.session.workspace;
    if ws.as_os_str().is_empty() || !ws.is_dir() {
        app.prompt.workspace.1.clone()
    } else {
        ws.clone()
    }
}

/// Whether the session started with a workspace layout — a successor session
/// should then receive a freshly rebuilt tree. Cheap: no filesystem access.
fn session_started_with_tree(tab: &SessionTab) -> bool {
    tab.session.first_user_message().is_some_and(|m| {
        m.content.parts().iter().any(|p| {
            p.as_text()
                .is_some_and(|t| t.starts_with("Working directory layout"))
        })
    })
}

/// Send the task-tool result, tagged with its call_id, to the parent tab's
/// waiting stream (if still open).
fn deliver_task_report(
    app: &mut App,
    parent_number: usize,
    call_id: String,
    result: Result<String, String>,
) {
    let delivered = if let Some(pos) = app.conversation.tab_pos(parent_number)
        && let Some(sender) = app.conversation.session_tabs[pos]
            .session_state
            .task_sender
            .as_ref()
    {
        sender.send((call_id.clone(), result)).is_ok()
    } else {
        false
    };
    if !delivered {
        // Parent tab closed or its stream ended — the report has nowhere to go.
        tracing::warn!(parent_tab = parent_number, call_id = %call_id, "task report dropped: parent stream no longer waiting");
    }
}

/// Deliver a failure report for a task spawn and stop.
fn fail_task_spawn(
    app: &mut App,
    number: usize,
    call_id: String,
    message: String,
) -> Task<Message> {
    deliver_task_report(app, number, call_id, Err(message));
    Task::none()
}

/// Which tool triggered this successor spawn — each carries its own payload.
#[derive(Debug, Clone)]
pub(crate) enum SpawnKind {
    Renew {
        mode: Option<WorkMode>,
    },
    Task {
        call_id: String,
        title: Option<String>,
        preamble: Option<String>,
    },
}

/// Prepared successor-session spawn (renew/task tool) whose blocking workspace
/// scan has completed off the UI thread; the continuation message completes the spawn.
#[derive(Debug, Clone)]
pub(crate) struct SuccessorSpawn {
    /// Origin/parent tab number.
    pub(crate) number: usize,
    pub(crate) selected_model: String,
    pub(crate) selected_preamble: String,
    pub(crate) model: ModelConfig,
    pub(crate) workspace: PathBuf,
    /// Rebuilt "Working directory layout" tree (None when the origin session
    /// didn't start with one).
    pub(crate) workspace_tree: Option<String>,
    /// Which tool triggered the spawn and its mode-specific data.
    pub(crate) kind: SpawnKind,
    /// Prompt to launch in the new session.
    pub(crate) prompt: String,
}

/// Handle a renew-tool request: create a new session tab and launch the
/// continuation prompt on it, using the same model and work mode as the
/// originating session. The workspace tree is rebuilt off the UI thread; the
/// spawn itself happens in [`super::conversation::continue_renew_spawn`] once the scan lands.
fn handle_renew(app: &mut App, number: usize, prompt: String) -> Task<Message> {
    let (model, work_mode, workspace, need_tree, selected_model, selected_preamble) = {
        let Some(pos) = app.conversation.tab_pos(number) else {
            return Task::none();
        };
        let tab = &app.conversation.session_tabs[pos];
        let Some(model) = tab_model_config(app, tab) else {
            return Task::none();
        };
        (
            model,
            tab.session.dialogs.last().and_then(|d| d.mode),
            tab_workspace(app, tab),
            session_started_with_tree(tab),
            tab.selected_model.clone(),
            tab.selected_preamble.clone(),
        )
    };
    tracing::info!(parent_tab = number, model = %model.model_id, "renew tool: spawning successor session");
    let ws = workspace.clone();
    Task::perform(
        async move {
            if need_tree {
                tokio::task::spawn_blocking(move || crabot::workspace::build_files_tree(&ws))
                    .await
                    .ok()
            } else {
                None
            }
        },
        move |workspace_tree| {
            Message::Conversation(ConversationEvent::RenewSpawnReady(Box::new(
                SuccessorSpawn {
                    number,
                    selected_model,
                    selected_preamble,
                    model,
                    workspace,
                    workspace_tree,
                    kind: SpawnKind::Renew { mode: work_mode },
                    prompt,
                },
            )))
        },
    )
}

/// Handle a task-tool request: spawn a sub-agent session tab with a mode
/// preamble and difficulty-selected model, delivering its final report to the
/// parent on completion. The workspace scan runs off the UI thread; the spawn
/// itself happens in [`super::conversation::continue_task_spawn`].
fn handle_task_request(app: &mut App, number: usize, request: TaskRequest) -> Task<Message> {
    let Some(pos) = app.conversation.tab_pos(number) else {
        return Task::none();
    };
    let TaskRequest {
        call_id,
        title,
        prompt,
        mode,
        difficulty,
    } = request;
    tracing::info!(
        parent_tab = number,
        call_id = %call_id,
        mode = mode.as_deref().unwrap_or("default"),
        difficulty = difficulty.as_deref().unwrap_or("medium"),
        "task tool: spawning sub-agent"
    );
    // Resolve the parent tab's context (cheap — no filesystem access).
    let (parent_selected, parent_selected_preamble, parent_model, workspace, need_tree) = {
        let parent = &app.conversation.session_tabs[pos];
        (
            parent.selected_model.clone(),
            parent.selected_preamble.clone(),
            tab_model_config(app, parent),
            tab_workspace(app, parent),
            session_started_with_tree(parent),
        )
    };

    // Difficulty → configured subtask model (fallback: the parent's model).
    let difficulty = difficulty.as_deref().unwrap_or("medium");
    let configured = app.settings.task_models.get_config(difficulty);
    let (selected_model, model) = if configured.is_empty() {
        // Empty config means "inherit the parent session's model".
        match parent_model {
            Some(cfg) => (parent_selected, cfg),
            None => {
                return fail_task_spawn(
                    app,
                    number,
                    call_id.clone(),
                    "No model available for the subtask.".into(),
                );
            }
        }
    } else {
        (app.find_model_label(configured), configured.clone())
    };
    // Stale config would silently no-op `start_dialog` and strand the parent.
    if app.models.get_model_info(&model).is_none() {
        return fail_task_spawn(
            app,
            number,
            call_id,
            format!(
                "Subtask model is no longer resolvable: '{}' (provider '{}' / model '{}').",
                selected_model, model.provider_id, model.model_id
            ),
        );
    }

    // The sub-agent inherits the parent's workspace; the tree and mode
    // preamble are scanned off the UI thread.
    let ws = workspace.clone();
    Task::perform(
        async move {
            let tree = if need_tree {
                tokio::task::spawn_blocking(move || crabot::workspace::build_files_tree(&ws))
                    .await
                    .ok()
            } else {
                None
            };
            // Load the mode preamble; record the mode name only when a preamble file exists.
            let (preamble, preamble_name) = tokio::task::spawn_blocking(move || {
                let name = mode.as_deref().map(str::to_ascii_lowercase);
                let content = task_mode_preamble(name.as_deref());
                let name_used = name.filter(|_| content.is_some());
                (content, name_used)
            })
            .await
            .ok()
            .unwrap_or((None, None));
            (tree, preamble, preamble_name)
        },
        move |(workspace_tree, preamble, preamble_name)| {
            Message::Conversation(ConversationEvent::TaskSpawnReady(Box::new(
                SuccessorSpawn {
                    number,
                    selected_model,
                    selected_preamble: preamble_name.unwrap_or(parent_selected_preamble),
                    model,
                    workspace,
                    workspace_tree,
                    kind: SpawnKind::Task {
                        call_id,
                        title,
                        preamble,
                    },
                    prompt,
                },
            )))
        },
    )
}

/// Load the mode-specific sub-agent preamble (`~/.crabot/preamble/{mode}.md`).
/// Only the modes from the task schema are recognized; anything else
/// (including path-traversal attempts) falls back to the default prompt.
fn task_mode_preamble(mode: Option<&str>) -> Option<String> {
    let mode = mode?.to_ascii_lowercase();
    if !TASK_MODES.contains(&mode.as_str()) {
        return None;
    }
    let path = crabot::setup::default_workspace_path()
        .join("preamble")
        .join(format!("{mode}.md"));
    std::fs::read_to_string(path)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Extract the last non-empty assistant text from a finished session — the
/// sub-agent's final report returned to the parent as the task tool result.
fn final_assistant_text(session: &Session) -> Option<String> {
    session
        .history
        .iter()
        .rev()
        .filter(|m| m.role == ChatRole::Assistant)
        .find_map(|m| {
            let text = m.content.joined_texts().unwrap_or_default();
            (!text.trim().is_empty()).then_some(text)
        })
}

/// Pop the next pending ask from the queue and switch to that tab.
pub(super) fn process_pending_ask_queue(app: &mut App) -> Task<Message> {
    while let Some(number) = app.conversation.pending_ask_queue.pop_front() {
        // Skip closed tabs or asks already resolved (e.g. by timeout).
        if let Some(pos) = app.conversation.tab_pos(number)
            && app.conversation.session_tabs[pos]
                .session_state
                .ask_request
                .is_some()
        {
            if pos == app.conversation.viewing {
                // Already viewing — request is visible, nothing to do.
                break;
            }
            return switch_tab(app, number);
        }
    }
    Task::none()
}

/// Route a tagged stream event to the owning tab.
pub(super) fn session_event(app: &mut App, number: usize, event: SessionEvent) -> Task<Message> {
    // Renew/task requests are intercepted here — they spawn successor sessions.
    match &event {
        SessionEvent::RenewRequest(prompt) => return handle_renew(app, number, prompt.clone()),
        SessionEvent::TaskRequest(request) => {
            return handle_task_request(app, number, request.clone());
        }
        _ => {}
    }

    let Some(pos) = app.conversation.tab_pos(number) else {
        // Tab was closed while the stream was still running — drop the event.
        return Task::none();
    };

    let switch_task =
        if pos != app.conversation.viewing && matches!(event, SessionEvent::AskRequest(_)) {
            if app
                .conversation
                .viewing()
                .session_state
                .ask_request
                .is_some()
            {
                // Queue it — the user answers the current ask first.
                app.conversation.pending_ask_queue.push_back(number);
                Task::none()
            } else {
                // Auto-switch to a background tab that issues an ask.
                switch_tab(app, number)
            }
        } else {
            Task::none()
        };
    // `switch_tab` only changes `viewing`, never reorders tabs, so `pos` stays valid.
    let viewing = pos == app.conversation.viewing;

    // Compute cost and context window from the tab's session model BEFORE mutably borrowing the tab.
    let model_config = tab_model_config(app, &app.conversation.session_tabs[pos]);
    let cost = model_config
        .as_ref()
        .and_then(|cfg| app.models.get_model(cfg))
        .map(|m| m.cost.clone());
    let context_window = model_config.as_ref().map(|cfg| cfg.context_window);

    let finished = matches!(event, SessionEvent::Done);
    let asked = matches!(event, SessionEvent::AskRequest(_));
    let is_cancelled = matches!(event, SessionEvent::Cancelled);
    let task_error = match &event {
        SessionEvent::Error(err) => Some(err.clone()),
        _ => None,
    };

    // Remember whether this tab had an active ask so we can detect a clear.
    let had_ask = app.conversation.session_tabs[pos]
        .session_state
        .ask_request
        .is_some();

    let tab = &mut app.conversation.session_tabs[pos];
    let fill_ratio_threshold = app.settings.fill_ratio_threshold;
    let update_task = update(
        event,
        tab,
        cost,
        context_window,
        fill_ratio_threshold,
        viewing,
    );

    // If this tab's ask was just resolved (user answer, timeout, Done, …),
    // remove it from the queue and, when the viewing tab has no active ask,
    // process remaining pending asks.
    let ask_cleared = had_ask
        && app.conversation.session_tabs[pos]
            .session_state
            .ask_request
            .is_none();
    let mut queue_task = Task::none();
    if ask_cleared {
        app.conversation.pending_ask_queue.retain(|&n| n != number);
        if app
            .conversation
            .viewing()
            .session_state
            .ask_request
            .is_none()
        {
            queue_task = process_pending_ask_queue(app);
        }
    }

    // Auto-dispatch a prompt injected too late for the just-ended stream.
    let dispatch_task = if finished {
        dispatch_pending(app, pos)
    } else {
        Task::none()
    };

    // Task sub-agent that just terminated — deliver its final report to the parent.
    if let Some(parent) = app.conversation.session_tabs[pos].task_parent()
        && (finished || is_cancelled || task_error.is_some())
        && let Some(call_id) = app.conversation.session_tabs[pos].task_call_id.take()
    {
        let result = if let Some(err) = task_error {
            Err(format!("Subtask failed: {err}"))
        } else if is_cancelled {
            Err("Subtask was cancelled.".into())
        } else {
            Ok(
                final_assistant_text(&app.conversation.session_tabs[pos].session).unwrap_or_else(
                    || "Subtask completed without a final text response.".to_string(),
                ),
            )
        };
        tracing::info!(
            subtask_tab = number,
            parent_tab = parent,
            ok = result.is_ok(),
            "sub-agent finished, delivering report to parent"
        );
        deliver_task_report(app, parent, call_id, result);
    }

    // Drop stale ask flashes; Done/AskRequest otherwise raises attention.
    let attention_task = if !finished
        && !app.layout.window_focused
        && had_ask
        && app
            .conversation
            .session_tabs
            .iter()
            .all(|t| t.session_state.ask_request.is_none())
    {
        attention::clear()
    } else {
        attention::raise(app.layout.window_focused, finished, asked)
    };

    switch_task
        .chain(update_task.discard())
        .chain(dispatch_task)
        .chain(queue_task)
        .chain(attention_task)
}
