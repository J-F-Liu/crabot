// Hide the console window in release builds. Debug builds keep the console
// for `println!`/`eprintln!` output during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chat;
mod fonts;
mod llm;
mod model;
mod session;
mod settings;
mod setup;
mod system;
mod tool;
mod tools;
mod user;
mod views;
mod widgets;
mod workspace;

use futures::{SinkExt, future::FutureExt};
use iced::widget::scrollable::Viewport;
use iced::widget::{row, text_editor};
use iced::{
    Element, Event, Point, Size, Subscription, Task, Theme, event, keyboard, mouse, window,
};
use indexmap::IndexMap;
use llm::StreamState;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chat::{TextContent, ToolCall, ToolResult, Turn, TurnBody, replace_emoji};
use genai::chat::{ChatMessage, ChatRole};
use model::{Model, ModelConfig, ModelList, TokenAmount};
use session::Session;
use system::{FilepathEntry, RULES, SystemPrompt, TOOLS, WORKSPACE, WORKSPACE_TREE};
use tools::DevTool;
use user::{UserPrompt, WorkMode};
use views::model_config::ProviderEntry;
use views::session_view::SessionEntry;
use views::theme::{HANDLE, MIN_W, default_theme};
use views::{center_pane, divider, left_pane, right_pane, scroll_to_end};
use widgets::textarea::{self, TextArea};

/// Compile-time title embedding the Cargo.toml version via crabtime.
#[crabtime::expression]
fn crabot_title() {
    let cargo_toml = format!("{}/Cargo.toml", crabtime::WORKSPACE_PATH);
    let content = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
    let version = content
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .unwrap_or("0.0");
    let title = format!("\"Crabot v{}\"", version);
    crabtime::output! {
        {{title}}
    }
}

pub fn main() -> iced::Result {
    setup::ensure_default_files();
    fonts::load_system_fonts();
    let saved = settings::Settings::load();
    let size = Size::new(
        saved.window_size.0.max(MIN_W),
        saved.window_size.1.max(200.0),
    );
    let position =
        iced::window::Position::Specific(Point::new(saved.window_pos.0, saved.window_pos.1));
    iced::application(move || App::boot(saved.clone()), App::update, App::view)
        .subscription(App::subscription)
        .theme(|state: &App| state.theme.clone())
        .window_size(size)
        .position(position)
        .title(crabot_title!())
        .antialiasing(true)
        .exit_on_close_request(false)
        .run()
}

// ── divider identity ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Divider {
    Left,
    Right,
}

// ── drag tracking ─────────────────────────────────────────────────

struct Drag {
    which: Divider,
    origin: f32,
    left_start: f32,
    right_start: f32,
}

// ── App ───────────────────────────────────────────────────────────

/// Which widget currently holds keyboard focus.
///
/// Stored as a single `Option` on `App` so that setting focus on one widget
/// implicitly clears focus on all others — no manual `set_focused(false)`
/// calls are needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedTarget {
    /// The user prompt text area.
    UserPrompt,
    /// A system-prompt text editor identified by its field name.
    EditText(&'static str),
    /// The session pick_list in the left pane.
    SessionPicker,
}

struct App {
    left_pane_width: f32,
    right_pane_width: f32,
    window_size: Size,
    window_pos: Point,
    cursor: Point,
    dragging: Option<Drag>,
    provided_models: ModelList,
    provider_entries: Vec<ProviderEntry>,
    selected_model: String,
    theme: Theme,
    system_prompt: SystemPrompt,
    rules_expanded: bool,
    tools_expanded: bool,
    files_expanded: bool,
    selected_preamble: String,
    preamble_options: Vec<FilepathEntry>,
    workspace_options: Vec<FilepathEntry>,
    // user-editable Content need to persist between view calls to maintain editor state
    rules_content: TextArea,
    files_content: text_editor::Content,
    tools_content: text_editor::Content,
    dev_tools: IndexMap<DevTool, bool>,
    user_prompt: TextArea,
    workmode: WorkMode,
    session: Session,
    /// Available saved-sessions for the dropdown list in the left pane.
    session_options: Vec<SessionEntry>,
    /// Current phase of the LLM interaction lifecycle.
    streaming: StreamState,
    /// Index (flat turn count) in the session where the current stream's
    /// placeholders begin; used by `handle_stream_done` to backfill captured
    /// content/reasoning into the right display messages.
    stream_start_index: usize,
    /// Indices of turns whose collapsible body (tool result / reasoning) is expanded.
    expanded_turns: HashSet<usize>,
    /// Indices of dialogs that are expanded.
    expanded_dialogs: HashSet<usize>,
    /// Token usage from the most recent completed LLM response.
    last_usage: genai::chat::Usage,
    /// Last-sent user prompt text, displayed in the center-pane header.
    center_pane_title: String,
    /// Whether to show the Restart button (current_exe within workspace).
    show_restart: bool,
    /// Cancellation token to stop an in-progress stream early.
    cancel_token: Arc<AtomicBool>,
    /// Whether to auto-scroll the message view to the bottom during streaming.
    /// Set `true` when a new prompt is sent; `on_scroll` toggles it based on
    /// whether the user has manually scrolled away from or to the bottom.
    auto_scroll: Arc<AtomicBool>,
    /// Indices of messages displayed as selectable plain text (double-click
    /// a single message to toggle; ESC clears all).
    selectable_msgs: HashSet<usize>,
    /// Whether the Shift key is currently held. Used to distinguish Enter
    /// (send prompt) from Shift+Enter (insert newline) in the text editor.
    shift_held: bool,
    /// Which widget currently holds keyboard focus; `None` when no editable
    /// widget is focused. Setting this implicitly clears focus on all others.
    focused: Option<FocusedTarget>,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    CursorMoved(Point),
    LeftPressed,
    LeftReleased,
    WindowResized(Size),
    WindowMoved(Point),
    ToggleEnabled(&'static str, bool),
    ToggleExpanded(&'static str),
    EditTextField(&'static str, String),
    EditTextContent(&'static str, text_editor::Action),
    SelectWorkspace(FilepathEntry),
    WorkspaceDialogResult(Option<PathBuf>),
    SelectPreamble(FilepathEntry),
    PreambleFileResult(Result<String, String>),
    ToggleDevTool(String, bool),
    /// An edit action targeting a specific [`TextArea`].
    /// The [`FocusedTarget`] identifies which text area should receive the action.
    EditTextArea(FocusedTarget, textarea::Message),
    /// Global undo/redo shortcut (Ctrl+Z / Ctrl+Y). Routed to whichever
    /// [`TextArea`] currently holds keyboard focus.
    UndoRedo(textarea::Message),
    SelectWorkMode(WorkMode),
    NewSession,
    LoadSession(SessionEntry),
    SessionListLoaded(Vec<SessionEntry>),
    SendPrompt,
    ToggleTurnExpand(usize),
    ToggleDialogExpand(usize),
    StreamToolCall(ToolCall),
    StreamContent(String),
    StreamReasoning(String),
    StreamToolResult(ToolResult),
    TokenUsage(Option<genai::chat::Usage>),
    StreamDone(Vec<ChatMessage>),
    StreamError(String, Vec<ChatMessage>),
    StreamCancelled(Vec<ChatMessage>),
    StreamStateChange(StreamState),
    StopStream,
    AppClosing,
    Noop,
    CopySessionTitle,
    ResendLastPrompt,
    Restart,
    /// Fires when the message scrollable's viewport changes.
    MessageViewScrolled(Viewport),
    /// Toggle a single message between Markdown and selectable plain-text.
    /// `Some(i)` toggles message `i`; `None` clears all (ESC).
    ToggleSelectableMode(Option<usize>),
    /// Track whether the Shift key is currently held.
    ShiftHeld(bool),
    /// Model configuration event (provider/model selection, thinking).
    ModelConfigEvent(views::model_config::Event),
    /// Session pick_list gained focus (e.g. via dropdown open).
    SessionPickerFocused,
    /// Arrow-key navigation for the session pick_list.
    /// `true` = up (previous), `false` = down (next).
    NavigateSession(bool),
}

// ── App impl ──────────────────────────────────────────────────────

impl App {
    fn boot(saved: settings::Settings) -> (Self, Task<Message>) {
        let provided_models = model::load_models();
        let provider_entries: Vec<ProviderEntry> = provided_models
            .providers
            .iter()
            .map(|(id, p)| ProviderEntry {
                id: id.clone(),
                name: p.name.clone(),
            })
            .collect();
        let selected_model = provided_models.ensure_valid_name(&saved.selected_model);

        let dev_tools: IndexMap<DevTool, bool> = DevTool::ALL
            .iter()
            .map(|&t| {
                (
                    t,
                    saved.builtin_tools.get(t.name()).copied().unwrap_or(true),
                )
            })
            .collect();

        let preamble_options = system::build_preamble_options();
        let preamble_content = preamble_options
            .iter()
            .find(|e| e.display == saved.selected_preamble)
            .map(|e| std::fs::read_to_string(&e.path).unwrap_or_else(|e| e.to_string()))
            .unwrap_or_default();

        let workspace_path = saved.workspace;
        let files_tree = workspace::build_files_tree(&workspace_path);
        let tools_summary = tool::tools_summary(&dev_tools);
        let rules_content = TextArea::with_text(&saved.rules_text);
        let files_content = text_editor::Content::with_text(&files_tree);
        let tools_content = text_editor::Content::with_text(&tools_summary);

        let system_prompt = SystemPrompt {
            preamble: (saved.preamble_enabled, preamble_content),
            rules: (saved.rules_enabled, saved.rules_text),
            tools: (saved.tools_enabled, tools_summary),
            workspace: (saved.workspace_enabled, workspace_path.clone()),
            files: (saved.files_enabled, files_tree),
            date: (
                saved.date_enabled,
                chrono::Local::now().format("%Y-%m-%d").to_string(),
            ),
        };

        let show_restart = !workspace_path.as_os_str().is_empty()
            && env::current_exe()
                .ok()
                .is_some_and(|exe| exe.starts_with(&workspace_path));

        let mut app = Self {
            left_pane_width: saved.left_pane_width,
            right_pane_width: saved.right_pane_width,
            window_size: Size::new(saved.window_size.0, saved.window_size.1),
            window_pos: Point::new(saved.window_pos.0, saved.window_pos.1),
            cursor: Point::ORIGIN,
            dragging: None,
            provided_models,
            provider_entries,
            preamble_options,
            system_prompt,
            theme: default_theme(),
            selected_model,
            selected_preamble: saved.selected_preamble,
            workspace_options: system::build_workspace_options(&saved.recent_workspaces),
            rules_expanded: false,
            tools_expanded: false,
            files_expanded: false,
            rules_content,
            files_content,
            tools_content,
            dev_tools,
            user_prompt: TextArea::new(),
            workmode: WorkMode::Code,
            session: Session::new(),
            session_options: Vec::new(),
            streaming: StreamState::Idle,
            stream_start_index: 0,
            expanded_turns: HashSet::new(),
            expanded_dialogs: HashSet::new(),
            last_usage: genai::chat::Usage::default(),
            center_pane_title: "New session".into(),
            show_restart,
            cancel_token: Arc::new(AtomicBool::new(false)),
            auto_scroll: Arc::new(AtomicBool::new(true)),
            selectable_msgs: HashSet::new(),
            shift_held: false,
            focused: None,
        };
        let session_task = app.refresh_session_list();
        (app, session_task)
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::CursorMoved(pos) => {
                self.cursor = pos;
                let Some(drag) = &self.dragging else {
                    return Task::none();
                };
                let delta = pos.x - drag.origin;
                let gutter = 2.0 * HANDLE;
                match drag.which {
                    Divider::Left => {
                        let max = (self.window_size.width - self.right_pane_width - gutter - MIN_W)
                            .max(MIN_W);
                        self.left_pane_width = (drag.left_start + delta).clamp(MIN_W, max);
                    }
                    Divider::Right => {
                        let max = (self.window_size.width - self.left_pane_width - gutter - MIN_W)
                            .max(MIN_W);
                        self.right_pane_width = (drag.right_start - delta).clamp(MIN_W, max);
                    }
                }
            }
            Message::LeftPressed => {
                let left_x = self.left_pane_width;
                let right_x = self.window_size.width - self.right_pane_width - HANDLE;

                let which = if self.cursor.x >= left_x && self.cursor.x <= left_x + HANDLE {
                    Some(Divider::Left)
                } else if self.cursor.x >= right_x && self.cursor.x <= right_x + HANDLE {
                    Some(Divider::Right)
                } else {
                    None
                };

                if let Some(which) = which {
                    self.dragging = Some(Drag {
                        which,
                        origin: self.cursor.x,
                        left_start: self.left_pane_width,
                        right_start: self.right_pane_width,
                    });
                }
            }
            Message::LeftReleased => {
                self.dragging = None;
            }
            Message::WindowResized(size) => {
                // Ignore zero-size events (e.g. window minimized on Windows).
                if size.width > 0.0 && size.height > 0.0 {
                    self.window_size = size;
                }
            }
            Message::WindowMoved(pos) => {
                self.window_pos = pos;
            }
            Message::ModelConfigEvent(event) => {
                if views::model_config::update(
                    &event,
                    &mut self.provided_models,
                    &self.selected_model,
                ) {
                    self.provided_models.save();
                }
            }
            Message::ToggleEnabled(name, enabled) => {
                if name == WORKSPACE {
                    self.system_prompt.workspace.0 = enabled;
                } else if let Some(field) = self.system_prompt.get_mut(name) {
                    field.0 = enabled;
                }
            }
            Message::ToggleDevTool(tool_name, enabled) => {
                if let Some(tool) = DevTool::ALL.iter().find(|t| t.name() == tool_name) {
                    self.dev_tools.insert(*tool, enabled);
                    let summary = tool::tools_summary(&self.dev_tools);
                    self.system_prompt.tools.1 = summary.clone();
                    self.tools_content = text_editor::Content::with_text(&summary);
                }
            }
            Message::ToggleExpanded(name) => match name {
                RULES => self.rules_expanded = !self.rules_expanded,
                TOOLS => self.tools_expanded = !self.tools_expanded,
                WORKSPACE_TREE => self.files_expanded = !self.files_expanded,
                _ => {}
            },
            Message::EditTextField(name, value) => {
                if let Some(field) = self.system_prompt.get_mut(name) {
                    field.1 = value;
                }
            }
            Message::EditTextContent(name, action) => {
                // A click on this text editor claims focus (implicitly clearing
                // focus on the prompt editor and all others).
                if matches!(action, text_editor::Action::Click(_)) {
                    self.focused = Some(FocusedTarget::EditText(name));
                }
                let text = if let Some(content) = self.content_mut(name) {
                    content.perform(action);
                    content.text()
                } else {
                    return Task::none();
                };
                if let Some(field) = self.system_prompt.get_mut(name) {
                    field.1 = text;
                }
            }
            Message::EditTextArea(target, msg) => {
                // A click claims keyboard focus for this text area.
                if msg.is_click() {
                    self.focused = Some(target);
                } else if self.focused != Some(target) {
                    return Task::none();
                }
                match target {
                    FocusedTarget::UserPrompt => {
                        // Enter without Shift sends the prompt; Shift+Enter inserts
                        // a newline.
                        if msg.is_enter() && !self.shift_held {
                            return Task::done(Message::SendPrompt);
                        }
                        self.user_prompt.update(msg, self.shift_held);
                    }
                    FocusedTarget::EditText(RULES) => {
                        self.rules_content.update(msg, self.shift_held);
                        self.system_prompt.rules.1 = self.rules_content.text();
                    }
                    _ => {}
                }
            }
            Message::UndoRedo(msg) => {
                // Global Ctrl+Z/Y — route to whichever TextArea holds focus.
                match self.focused {
                    Some(FocusedTarget::UserPrompt) => {
                        self.user_prompt.update(msg, self.shift_held);
                    }
                    Some(FocusedTarget::EditText(RULES)) => {
                        self.rules_content.update(msg, self.shift_held);
                        self.system_prompt.rules.1 = self.rules_content.text();
                    }
                    _ => {}
                }
            }
            Message::SelectWorkspace(entry) => {
                if entry.path.as_os_str().is_empty() {
                    return Task::perform(
                        async { rfd::FileDialog::new().pick_folder() },
                        Message::WorkspaceDialogResult,
                    );
                }
                self.set_workspace(entry.path);
                return self.refresh_session_list();
            }
            Message::WorkspaceDialogResult(Some(path)) => {
                self.set_workspace(path);
                return self.refresh_session_list();
            }
            Message::WorkspaceDialogResult(None) => {}
            Message::SelectPreamble(entry) => {
                let FilepathEntry { display, path } = entry;
                self.selected_preamble = display;
                return Task::perform(
                    async move { std::fs::read_to_string(&path).map_err(|e| e.to_string()) },
                    Message::PreambleFileResult,
                );
            }
            Message::PreambleFileResult(Ok(content)) => {
                self.system_prompt.preamble.1 = content;
            }
            Message::PreambleFileResult(Err(_)) => {}
            Message::SelectWorkMode(mode) => {
                self.workmode = mode;
            }
            Message::NewSession => {
                self.session = Session::new();
                self.center_pane_title = "New session".into();
                self.last_usage = genai::chat::Usage::default();
                self.expanded_turns.clear();
                self.expanded_dialogs.clear();
                self.selectable_msgs.clear();
                // Refresh workspace tree so the system prompt reflects current files.
                self.system_prompt.files.1 =
                    workspace::build_files_tree(&self.system_prompt.workspace.1);
                self.files_content = text_editor::Content::with_text(&self.system_prompt.files.1);
                return self.refresh_session_list();
            }
            Message::ToggleTurnExpand(idx) => {
                if self.expanded_turns.contains(&idx) {
                    self.expanded_turns.remove(&idx);
                } else {
                    self.expanded_turns.insert(idx);
                }
            }
            Message::ToggleDialogExpand(idx) => {
                if self.expanded_dialogs.contains(&idx) {
                    self.expanded_dialogs.remove(&idx);
                } else {
                    self.expanded_dialogs.insert(idx);
                }
            }
            Message::LoadSession(entry) => {
                if self.streaming != StreamState::Idle {
                    return Task::none();
                }
                match Session::load(&entry.path) {
                    Ok(session) => {
                        self.session = session;
                    }
                    Err(e) => {
                        self.session = Session::new();
                        self.session.id = entry.id;
                        eprintln!("Failed to load session: {e}");
                    }
                }
                self.last_usage = genai::chat::Usage {
                    prompt_tokens: Some(self.session.size),
                    ..Default::default()
                };
                self.center_pane_title = self.session.title.clone();
                self.expanded_turns.clear();
                self.expanded_dialogs.clear();
                self.selectable_msgs.clear();
            }
            Message::SessionListLoaded(entries) => {
                self.session_options = entries;
            }
            Message::SessionPickerFocused => {
                self.focused = Some(FocusedTarget::SessionPicker);
            }
            Message::NavigateSession(up) => {
                if self.focused != Some(FocusedTarget::SessionPicker)
                    || self.streaming != StreamState::Idle
                    || self.session_options.is_empty()
                {
                    return Task::none();
                }
                let current_idx = self
                    .session_options
                    .iter()
                    .position(|e| e.id == self.session.id);
                let new_entry = match current_idx {
                    Some(idx) => {
                        let new_idx = if up {
                            idx.checked_sub(1)
                                .unwrap_or(self.session_options.len().saturating_sub(1))
                        } else {
                            let next = idx + 1;
                            if next < self.session_options.len() {
                                next
                            } else {
                                0
                            }
                        };
                        Some(self.session_options[new_idx].clone())
                    }
                    None => {
                        // Current session not in list; select the first (or last) entry.
                        if up {
                            self.session_options.last().cloned()
                        } else {
                            self.session_options.first().cloned()
                        }
                    }
                };
                if let Some(entry) = new_entry {
                    return Task::done(Message::LoadSession(entry));
                }
            }
            Message::SendPrompt => {
                let content = self.user_prompt.text();
                if self.streaming != StreamState::Idle || content.trim().is_empty() {
                    return Task::none();
                }
                let Some(model) = self
                    .provided_models
                    .get_config(&self.selected_model)
                    .cloned()
                else {
                    return Task::none();
                };

                let title = Session::derive_title(&content);
                self.center_pane_title = content.clone();

                let user_prompt = UserPrompt::new(self.workmode, content).get_prompt();
                self.user_prompt.clear();
                // Auto-collapse all previous dialogs; keep the new one expanded.
                let new_dialog_idx = self.session.dialogs.len();
                self.expanded_dialogs.clear();
                self.expanded_dialogs.insert(new_dialog_idx);
                self.session.add_dialog(title);
                self.session.push_turn(Turn::user(user_prompt.clone()));

                return self.start_dialog(&model, Some(user_prompt));
            }
            Message::ResendLastPrompt => {
                if self.streaming != StreamState::Idle || self.center_pane_title == "New session" {
                    return Task::none();
                }
                let Some(model) = self
                    .provided_models
                    .get_config(&self.selected_model)
                    .cloned()
                else {
                    return Task::none();
                };
                // Collapse all other dialogs; keep the last one expanded during resend.
                self.expanded_dialogs.clear();
                if let Some(idx) = self.session.dialogs.len().checked_sub(1) {
                    self.expanded_dialogs.insert(idx);
                }
                return self.start_dialog(&model, None);
            }
            Message::StreamToolCall(tc) => {
                self.session.push_turn(Turn::from_tool_call(tc));
                return self.maybe_scroll_to_end();
            }
            Message::StreamContent(chunk) => {
                self.ensure_assistant_placeholder();
                if let Some(last) = self.session.last_turn_mut()
                    && let TurnBody::Text(tc) = &mut last.body
                {
                    tc.content.push_str(&chunk);
                }
                // Refresh the markdown cache for the last message.
                if let Some(last) = self.session.last_turn_mut() {
                    last.refresh_md_cache();
                }
                return self.maybe_scroll_to_end();
            }
            Message::StreamReasoning(chunk) => {
                self.ensure_assistant_placeholder();
                if let Some(last) = self.session.last_turn_mut()
                    && let TurnBody::Text(tc) = &mut last.body
                {
                    tc.reasoning
                        .get_or_insert_with(String::new)
                        .push_str(&chunk);
                }
                // Refresh the markdown cache for the last message.
                if let Some(last) = self.session.last_turn_mut() {
                    last.refresh_md_cache();
                }
                return self.maybe_scroll_to_end();
            }
            Message::StreamToolResult(tr) => {
                // Track files modified by write / edit tools.
                if let Some(path_str) = tr.get_modified_file()
                    && !self.session.modified_files.iter().any(|p| p == path_str)
                {
                    self.session.modified_files.push(path_str.to_string());
                }
                // Replace the pending Temp placeholder with the completed Tool turn.
                if let Some(last) = self.session.last_turn_mut() {
                    last.body = TurnBody::Tool(tr);
                } else {
                    self.session.push_turn(Turn::from_tool_result(tr));
                }
                return self.maybe_scroll_to_end();
            }
            Message::TokenUsage(usage) => {
                let u = usage.unwrap_or_default();
                let tokens = TokenAmount::from_genai(&u);
                let cost = self.get_current_model().map(|m| m.cost);
                self.session.accumulate_usage(&tokens, cost);
                self.session.size = u.prompt_tokens.unwrap_or(0);
                self.last_usage = u;
            }
            Message::StreamDone(genai_messages) => {
                self.handle_stream_done(genai_messages);
                return self.maybe_scroll_to_end();
            }
            Message::StreamError(err, genai_messages) => {
                self.handle_stream_error(err, genai_messages);
                return self.maybe_scroll_to_end();
            }
            Message::CopySessionTitle => {
                return iced::clipboard::write(self.center_pane_title.clone());
            }
            Message::Restart => {
                self.save_settings();
                // Spawn release build in background before exiting.
                let _ = Command::new("cargo").args(["run", "--release"]).spawn();
                return iced::exit();
            }
            Message::StopStream => {
                self.cancel_token.store(true, Ordering::Relaxed);
            }
            Message::MessageViewScrolled(viewport) => {
                // While streaming, track whether the user has scrolled away from the bottom
                // (to pause auto-scroll) or back to it (to resume).
                if self.streaming != StreamState::Idle {
                    let y = viewport.relative_offset().y;
                    let at_bottom = if y.is_nan() { true } else { y >= 0.99 };
                    self.auto_scroll.store(at_bottom, Ordering::Relaxed);
                }
            }
            Message::StreamCancelled(genai_messages) => {
                self.streaming = StreamState::Idle;
                // Preserve partial assistant/tool messages in history so
                // subsequent requests still carry valid context.
                self.session.history.extend(genai_messages);
                let _ = self.session.save();
            }
            Message::StreamStateChange(state) => {
                self.streaming = state;
            }
            Message::AppClosing => {
                self.save_settings();
                return iced::exit();
            }
            Message::Noop => {}
            Message::ShiftHeld(held) => {
                self.shift_held = held;
            }
            Message::ToggleSelectableMode(msg_index) => match msg_index {
                Some(i) => {
                    if self.selectable_msgs.contains(&i) {
                        self.selectable_msgs.remove(&i);
                    } else {
                        self.selectable_msgs.insert(i);
                    }
                }
                None => self.selectable_msgs.clear(),
            },
        }
        Task::none()
    }

    /// Ensure the last message is an assistant Text placeholder for streaming.
    /// If the last message is a Tool message (e.g., after a tool result was
    /// pushed in a subsequent iteration), create a new assistant placeholder
    /// so streamed text/reasoning lands in the right place.
    fn ensure_assistant_placeholder(&mut self) {
        let needs_placeholder = self.session.last_turn().is_none_or(|m| {
            !(m.role == ChatRole::Assistant && matches!(m.body, TurnBody::Text(_)))
        });
        if needs_placeholder {
            self.session.push_turn(Turn::assistant(String::new(), None));
        }
    }

    /// Backfill streaming placeholders with captured content from genai,
    /// extend session history, and persist the session.
    fn handle_stream_done(&mut self, genai_messages: Vec<ChatMessage>) {
        self.streaming = StreamState::Idle;

        // Some providers omit ReasoningChunk events and only expose
        // reasoning via captured_reasoning_content at stream end.
        let mut genai_asst_iter = genai_messages
            .iter()
            .filter(|m| m.role == genai::chat::ChatRole::Assistant)
            .filter(|m| {
                !m.content.joined_texts().unwrap_or_default().is_empty()
                    || m.content.first_reasoning_content().is_some()
            });

        for msg in self.session.turns_from_mut(self.stream_start_index) {
            if msg.role != ChatRole::Assistant {
                continue;
            }
            if let TurnBody::Text(tc) = &mut msg.body
                && let Some(genai_asst) = genai_asst_iter.next()
            {
                tc.content = replace_emoji(&genai_asst.content.joined_texts().unwrap_or_default());
                if tc.reasoning.is_none() {
                    tc.reasoning = genai_asst
                        .content
                        .first_reasoning_content()
                        .map(|s| s.to_string());
                }
                msg.refresh_md_cache();
            }
        }

        self.session.history.extend(genai_messages);
        let _ = self.session.save();
    }

    /// Replace the last-message empty assistant placeholder with this error,
    /// or push a new error message if no placeholder exists.
    fn handle_stream_error(&mut self, err: String, genai_messages: Vec<ChatMessage>) {
        self.streaming = StreamState::Idle;

        // Preserve any messages generated before the error (user msg,
        // partial assistant turns, tool calls/responses) in the history
        // so subsequent requests still carry valid context.
        self.session.history.extend(genai_messages);

        let is_empty_placeholder = self.session.last_turn().is_some_and(|m| {
            m.role == ChatRole::Assistant
                && matches!(
                    &m.body,
                    TurnBody::Text(TextContent { content, reasoning })
                        if content.is_empty() && reasoning.is_none()
                )
        });

        if is_empty_placeholder {
            if let Some(last) = self.session.last_turn_mut() {
                *last = Turn::assistant(format!("Error: {err}"), None);
            }
        } else {
            self.session
                .push_turn(Turn::assistant(format!("Error: {err}"), None));
        }
        let _ = self.session.save();
    }

    /// Snap the message scroll to the end, but only if auto-scroll is
    /// currently enabled (i.e. the user hasn't scrolled away).
    fn maybe_scroll_to_end(&self) -> Task<Message> {
        if self.auto_scroll.load(Ordering::Relaxed) {
            scroll_to_end()
        } else {
            Task::none()
        }
    }

    fn start_dialog(
        &mut self,
        model_config: &ModelConfig,
        user_prompt: Option<String>,
    ) -> Task<Message> {
        // Update session state with the selected model and workspace.
        let Some(model) = self.provided_models.get_model_info(model_config) else {
            return Task::none();
        };
        self.session.model = Some(model_config.clone());
        self.session.workspace = self.system_prompt.workspace.1.clone();
        self.stream_start_index = self.session.total_turns();
        self.auto_scroll.store(true, Ordering::Relaxed);

        let config = llm::SendConfig {
            model,
            workspace: self.system_prompt.workspace.1.clone(),
            system_prompt: self.system_prompt.get_prompt(),
            user_prompt,
            tools: DevTool::build_tools(&self.dev_tools),
        };

        let history = self.session.history.clone();

        self.streaming = StreamState::LlmLoading;
        self.cancel_token.store(false, Ordering::Relaxed);
        let cancel_token = self.cancel_token.clone();

        Task::batch([
            scroll_to_end(),
            Task::stream(iced::stream::channel(128, async move |sender| {
                let cancel = cancel_token.clone();
                let mut callback = {
                    move |msg: Message| {
                        let cancel = cancel.clone();
                        let mut sender = sender.clone();
                        async move {
                            let ok = sender.send(msg).await.is_ok();
                            if cancel.load(Ordering::Relaxed) {
                                false
                            } else {
                                ok
                            }
                        }
                        .boxed()
                    }
                };
                llm::send_stream(config, history, &mut callback).await;
            })),
        ])
    }

    /// Bump `path` to top of recents, persist it as current workspace,
    /// and rebuild the files tree.
    fn set_workspace(&mut self, path: PathBuf) {
        let mut paths: Vec<PathBuf> = std::mem::take(&mut self.workspace_options)
            .into_iter()
            .filter(|e| !e.path.as_os_str().is_empty())
            .map(|e| e.path)
            .collect();
        paths.retain(|p| p != &path);

        self.system_prompt.workspace.1 = path.clone();
        self.system_prompt.files.1 = workspace::build_files_tree(&path);
        self.files_content = text_editor::Content::with_text(&self.system_prompt.files.1);
        self.show_restart = env::current_exe()
            .ok()
            .is_some_and(|exe| exe.starts_with(&path));

        paths.insert(0, path);
        paths.truncate(10);
        self.workspace_options = system::build_workspace_options(&paths);
    }

    /// Collect current app state into `Settings` and persist to disk.
    fn save_settings(&self) {
        let settings = settings::Settings {
            left_pane_width: self.left_pane_width,
            right_pane_width: self.right_pane_width,
            window_size: (self.window_size.width, self.window_size.height),
            window_pos: (self.window_pos.x.max(0.0), self.window_pos.y.max(0.0)),
            selected_model: self.selected_model.clone(),
            selected_preamble: self.selected_preamble.clone(),
            preamble_enabled: self.system_prompt.preamble.0,
            rules_enabled: self.system_prompt.rules.0,
            tools_enabled: self.system_prompt.tools.0,
            workspace_enabled: self.system_prompt.workspace.0,
            files_enabled: self.system_prompt.files.0,
            date_enabled: self.system_prompt.date.0,
            workspace: self.system_prompt.workspace.1.clone(),
            recent_workspaces: self
                .workspace_options
                .iter()
                .filter(|e| !e.path.as_os_str().is_empty())
                .map(|e| e.path.clone())
                .collect(),
            rules_text: self.rules_content.text(),
            builtin_tools: self
                .dev_tools
                .iter()
                .map(|(t, &enabled)| (t.name().to_string(), enabled))
                .collect(),
        };
        settings.save();
    }

    /// Refresh the session list dropdown entries from disk.
    fn refresh_session_list(&mut self) -> Task<Message> {
        let workspace = self.system_prompt.workspace.1.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::views::session_view::list_entries(&workspace)
                })
                .await
                .unwrap_or(Ok(Vec::new()))
            },
            |result| match result {
                Ok(entries) => Message::SessionListLoaded(entries),
                Err(_) => Message::SessionListLoaded(Vec::new()),
            },
        )
    }

    fn content_mut(&mut self, name: &str) -> Option<&mut text_editor::Content> {
        match name {
            TOOLS => Some(&mut self.tools_content),
            WORKSPACE_TREE => Some(&mut self.files_content),
            _ => None,
        }
    }

    fn get_current_model(&self) -> Option<&Model> {
        self.session
            .model
            .as_ref()
            .or_else(|| self.provided_models.get_config(&self.selected_model))
            .and_then(|cfg| self.provided_models.get_model(cfg))
    }

    fn get_status(&self) -> &str {
        match self.streaming {
            StreamState::LlmLoading => "🔗 Loading LLM…",
            StreamState::LlmThinking => "💭 LLM thinking…",
            StreamState::ToolExecuting => "🔧 Tool executing…",
            StreamState::Idle => {
                if self.session.is_empty() {
                    "Send user prompt to start dialog with LLM"
                } else {
                    "✅ Ready"
                }
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        row![
            left_pane(
                self.left_pane_width,
                &self.provided_models,
                &self.provider_entries,
                &self.selected_model,
                &self.system_prompt,
                self.rules_expanded,
                self.tools_expanded,
                self.files_expanded,
                &self.selected_preamble,
                &self.preamble_options,
                &self.workspace_options,
                &self.rules_content,
                &self.files_content,
                &self.tools_content,
                &self.dev_tools,
                &self.user_prompt,
                self.workmode,
                self.streaming,
                &self.session_options,
                &self.session.id,
            ),
            divider(),
            center_pane(
                &self.center_pane_title,
                self.session.dialogs.as_slice(),
                &self.expanded_turns,
                &self.expanded_dialogs,
                self.get_status(),
                &self.theme,
                self.streaming,
                &self.selectable_msgs,
            ),
            divider(),
            right_pane(
                self.right_pane_width,
                self.get_current_model().map(|model| model.context_window),
                &self.last_usage,
                &self.session.usage,
                self.session.cost,
                &self.session.modified_files,
                self.show_restart,
            ),
        ]
        .spacing(0)
        .into()
    }

    fn subscription(_state: &Self) -> Subscription<Message> {
        Subscription::batch([
            event::listen_with(|event, _status, _window| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::CursorMoved(position))
                }
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    Some(Message::LeftPressed)
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::LeftReleased)
                }
                Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
                Event::Window(window::Event::Moved(pos)) => Some(Message::WindowMoved(pos)),
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
                    ..
                }) => Some(Message::NavigateSession(true)),
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
                    ..
                }) => Some(Message::NavigateSession(false)),
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::ToggleSelectableMode(None)),
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if modifiers.command() =>
                {
                    match &key {
                        keyboard::Key::Character(s) if s.as_str() == "z" => {
                            Some(Message::UndoRedo(textarea::Message::Undo))
                        }
                        keyboard::Key::Character(s) if s.as_str() == "y" => {
                            Some(Message::UndoRedo(textarea::Message::Redo))
                        }
                        _ => None,
                    }
                }
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Shift),
                    ..
                }) => Some(Message::ShiftHeld(true)),
                Event::Keyboard(keyboard::Event::KeyReleased {
                    key: keyboard::Key::Named(keyboard::key::Named::Shift),
                    ..
                }) => Some(Message::ShiftHeld(false)),
                _ => None,
            }),
            window::close_requests().map(|_id| Message::AppClosing),
        ])
    }
}
