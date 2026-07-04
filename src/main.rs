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
mod user;
mod views;
mod widgets;
mod workspace;

use crabot::{HashSetExt, tools};
use futures::{SinkExt, future::FutureExt};
use iced::widget::scrollable::Viewport;
use iced::widget::{row, text_editor};
use iced::{
    Element, Event, Point, Size, Subscription, Task, Theme, event, keyboard, mouse, window,
};
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
use system::{FilepathEntry, SystemPrompt, TOOLS, WORKSPACE, WORKSPACE_TREE};

use user::{UserPrompt, WorkMode};
use views::model_config::ProviderEntry;
use views::session_view::SessionEntry;
use views::theme::{HANDLE, MIN_W, default_theme};
use views::{DividerState, center_pane, divider, left_pane, right_pane, scroll_to_end};
use widgets::textarea::{self, TextArea};

fn crabot_title() -> &'static str {
    concat!("Crabot v", env!("CARGO_PKG_VERSION"))
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
        .title(crabot_title())
        .antialiasing(true)
        .exit_on_close_request(false)
        .run()
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
    left_divider: DividerState,
    right_divider: DividerState,
    provided_models: ModelList,
    provider_entries: Vec<ProviderEntry>,
    selected_model: String,
    theme: Theme,
    system_prompt: SystemPrompt,
    tools_expanded: bool,
    files_expanded: bool,
    selected_preamble: String,
    preamble_options: Vec<FilepathEntry>,
    selected_rules: String,
    rules_options: Vec<FilepathEntry>,
    workspace_options: Vec<FilepathEntry>,
    /// Whether an AGENTS.md file exists in the current workspace.
    agents_md_exists: bool,
    // user-editable Content need to persist between view calls to maintain editor state
    files_content: text_editor::Content,
    tools_content: text_editor::Content,
    enabled_tools: HashSet<String>,
    custom_tool_names: Vec<String>,
    builtin_tool_names: Vec<String>,
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
    /// Font scale factor for center pane dialog blocks.
    font_scale: f32,
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
    SelectRules(FilepathEntry),
    RulesFileResult(Result<String, String>),
    ToggleAgentTool(String, bool),
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
    /// Result of the "empty workspace" confirmation dialog shown before sending.
    EmptyWorkspaceConfirm(bool),
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
    /// Fires when the center pane scrollable's viewport changes.
    SessionViewScrolled(Viewport),
    /// Toggle a single message between Markdown and selectable plain-text.
    /// `Some(i)` toggles message `i`; `None` clears all (ESC).
    ToggleSelectableMode(Option<usize>),
    /// Track whether the Shift key is currently held.
    ShiftHeld(bool),
    /// Zoom the center pane font scale by a delta.
    Zoom(f32),
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

        let custom_tool_list = tools::ToolList::load();
        tools::register_custom_tools(custom_tool_list.build_tools());

        let enabled_tools: HashSet<String> = tools::builtin_tool_names()
            .into_iter()
            .chain(custom_tool_list.names())
            .filter(|name| saved.agent_tools.get(name).copied().unwrap_or(true))
            .collect();

        let (preamble_options, preamble_content) =
            views::load_prompt_options("preamble", &saved.selected_preamble);

        let (rules_options, rules_content) =
            views::load_prompt_options("rules", &saved.selected_rules);

        let workspace_path = saved.workspace;
        let files_tree = workspace::build_files_tree(&workspace_path);
        let (agents_md_exists, agents_md_content) = load_agents_md(&workspace_path);
        let tools_summary = system::tools_summary(&tools::enabled_tools(&enabled_tools));
        let files_content = text_editor::Content::with_text(&files_tree);
        let tools_content = text_editor::Content::with_text(&tools_summary);

        let show_restart = !workspace_path.as_os_str().is_empty()
            && env::current_exe()
                .ok()
                .is_some_and(|exe| exe.starts_with(&workspace_path));

        let system_prompt = SystemPrompt {
            preamble: (saved.preamble_enabled, preamble_content),
            rules: (saved.rules_enabled, rules_content),
            tools: (saved.tools_enabled, tools_summary),
            workspace: (saved.workspace_enabled, workspace_path),
            files: (saved.files_enabled, files_tree),
            agents_md: (saved.agents_md_enabled, agents_md_content),
            date: (
                saved.date_enabled,
                chrono::Local::now().format("%Y-%m-%d").to_string(),
            ),
        };

        let mut app = Self {
            left_pane_width: saved.left_pane_width,
            right_pane_width: saved.right_pane_width,
            window_size: Size::new(saved.window_size.0, saved.window_size.1),
            window_pos: Point::new(saved.window_pos.0, saved.window_pos.1),
            cursor: Point::ORIGIN,
            left_divider: DividerState::default(),
            right_divider: DividerState::default(),
            provided_models,
            provider_entries,
            preamble_options,
            rules_options,
            system_prompt,
            theme: default_theme(),
            selected_model,
            selected_preamble: saved.selected_preamble,
            selected_rules: saved.selected_rules,
            workspace_options: views::build_workspace_options(&saved.recent_workspaces),
            agents_md_exists,
            tools_expanded: false,
            files_expanded: false,
            files_content,
            tools_content,
            enabled_tools,
            custom_tool_names: custom_tool_list.names(),
            builtin_tool_names: tools::builtin_tool_names(),
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
            font_scale: saved.font_scale,
            focused: None,
        };
        let session_task = app.refresh_session_list();
        (app, session_task)
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::CursorMoved(pos) => {
                self.cursor = pos;
                self.left_divider.hovered =
                    pos.x >= self.left_pane_width && pos.x <= self.left_pane_width + HANDLE;
                let right_x = self.window_size.width - self.right_pane_width - HANDLE;
                self.right_divider.hovered = pos.x >= right_x && pos.x <= right_x + HANDLE;

                if self.left_divider.dragging {
                    let delta = pos.x - self.left_divider.origin;
                    let gutter = 2.0 * HANDLE;
                    let max = (self.window_size.width - self.right_pane_width - gutter - MIN_W)
                        .max(MIN_W);
                    self.left_pane_width = (self.left_divider.start + delta).clamp(MIN_W, max);
                } else if self.right_divider.dragging {
                    let delta = pos.x - self.right_divider.origin;
                    let gutter = 2.0 * HANDLE;
                    let max =
                        (self.window_size.width - self.left_pane_width - gutter - MIN_W).max(MIN_W);
                    let new = (self.right_divider.start - delta).max(0.0);
                    // Shrink below MIN_W → hide, expand from hidden → snap to MIN_W.
                    self.right_pane_width = if self.right_divider.start == 0.0 {
                        if new > 10.0 {
                            new.clamp(MIN_W, max)
                        } else {
                            0.0
                        }
                    } else if new < MIN_W - 10.0 {
                        0.0
                    } else {
                        new.min(max)
                    };
                }
            }
            Message::LeftPressed => {
                let left_x = self.left_pane_width;
                let right_x = self.window_size.width - self.right_pane_width - HANDLE;

                if self.cursor.x >= left_x && self.cursor.x <= left_x + HANDLE {
                    self.left_divider.dragging = true;
                    self.left_divider.origin = self.cursor.x;
                    self.left_divider.start = self.left_pane_width;
                } else if self.cursor.x >= right_x && self.cursor.x <= right_x + HANDLE {
                    // When the right pane is hidden, a single click on the divider shows it
                    if self.right_pane_width == 0.0 {
                        self.right_pane_width = MIN_W;
                    } else {
                        self.right_divider.dragging = true;
                        self.right_divider.origin = self.cursor.x;
                        self.right_divider.start = self.right_pane_width;
                    }
                }
            }
            Message::LeftReleased => {
                self.left_divider.dragging = false;
                self.right_divider.dragging = false;
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
                    event,
                    &mut self.provided_models,
                    &mut self.selected_model,
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
            Message::ToggleAgentTool(tool_name, enabled) => {
                if tools::find_tool(&tool_name).is_some() {
                    self.enabled_tools.set(tool_name, enabled);
                    let summary = system::tools_summary(&tools::enabled_tools(&self.enabled_tools));
                    self.system_prompt.tools.1 = summary.clone();
                    self.tools_content = text_editor::Content::with_text(&summary);
                }
            }
            Message::ToggleExpanded(name) => match name {
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
                if target == FocusedTarget::UserPrompt {
                    // Enter sends the prompt; Shift+Enter inserts a newline.
                    if msg.is_enter() && !self.shift_held {
                        return Task::done(Message::SendPrompt);
                    }
                    self.user_prompt.update(msg, self.shift_held);
                }
            }
            Message::UndoRedo(msg) => {
                // Global Ctrl+Z/Y — route to whichever TextArea holds focus.
                if let Some(FocusedTarget::UserPrompt) = self.focused {
                    self.user_prompt.update(msg, self.shift_held);
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
                return Self::select_prompt_file(
                    entry,
                    &mut self.selected_preamble,
                    Message::PreambleFileResult,
                );
            }
            Message::PreambleFileResult(result) => {
                if let Ok(content) = result {
                    self.system_prompt.preamble.1 = content;
                }
            }
            Message::SelectRules(entry) => {
                return Self::select_prompt_file(
                    entry,
                    &mut self.selected_rules,
                    Message::RulesFileResult,
                );
            }
            Message::RulesFileResult(result) => {
                if let Ok(content) = result {
                    self.system_prompt.rules.1 = content;
                }
            }
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
                let (exists, agents_md_content) = load_agents_md(&self.system_prompt.workspace.1);
                self.agents_md_exists = exists;
                self.system_prompt.agents_md = (self.agents_md_exists, agents_md_content);
                return self.refresh_session_list();
            }
            Message::ToggleTurnExpand(idx) => {
                let present = self.expanded_turns.contains(&idx);
                self.expanded_turns.set(idx, !present);
            }
            Message::ToggleDialogExpand(idx) => {
                let present = self.expanded_dialogs.contains(&idx);
                self.expanded_dialogs.set(idx, !present);
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

                // If no workspace is set, ask the user whether to continue with
                // the default `~/.crabot` workspace before sending the prompt.
                if self.system_prompt.workspace.1.as_os_str().is_empty() {
                    return window::oldest()
                        .and_then(|id| {
                            window::run(id, |window| {
                                let default_path =
                                    home::home_dir().unwrap_or_default().join(".crabot");
                                rfd::MessageDialog::new()
                                    .set_title("Empty Workspace")
                                    .set_level(rfd::MessageLevel::Warning)
                                    .set_buttons(rfd::MessageButtons::YesNo)
                                    .set_description(&format!(
                                        "Workspace path is empty. Continue with the default \
                                         workspace?\n\n{}",
                                        default_path.display()
                                    ))
                                    .set_parent(window)
                                    .show()
                                    == rfd::MessageDialogResult::Yes
                            })
                        })
                        .map(Message::EmptyWorkspaceConfirm);
                }

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
            Message::EmptyWorkspaceConfirm(confirmed) => {
                if !confirmed {
                    return Task::none();
                }
                let default_path = home::home_dir().unwrap_or_default().join(".crabot");
                self.set_workspace(default_path);
                // Re-enter the send-prompt flow now that a workspace is set.
                return Task::done(Message::SendPrompt);
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
            Message::SessionViewScrolled(viewport) => {
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
                // When the LLM begins thinking, push a new assistant turn placeholder.
                if state == StreamState::LlmThinking {
                    self.session.push_turn(Turn::assistant(String::new(), None));
                }
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
            Message::Zoom(delta) => {
                self.font_scale = (self.font_scale + delta).clamp(0.5, 2.0);
            }
            Message::ToggleSelectableMode(msg_index) => match msg_index {
                Some(i) => {
                    let present = self.selectable_msgs.contains(&i);
                    self.selectable_msgs.set(i, !present);
                }
                None => self.selectable_msgs.clear(),
            },
        }
        Task::none()
    }

    /// Backfill streaming placeholders with captured content from genai,
    /// extend session history, and persist the session.
    fn handle_stream_done(&mut self, genai_messages: Vec<ChatMessage>) {
        self.streaming = StreamState::Idle;

        let mut genai_asst_iter = genai_messages
            .iter()
            .filter(|m| m.role == genai::chat::ChatRole::Assistant)
            .filter_map(|m| {
                let text = m.content.joined_texts().unwrap_or_default();
                let reasoning = m.content.first_reasoning_content().map(|s| s.to_string());
                if !text.is_empty() || reasoning.is_some() {
                    Some((text, reasoning))
                } else {
                    None
                }
            });

        for turn in self.session.turns_from_mut(self.stream_start_index) {
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
                turn.refresh_md_cache();
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

        let error_msg = format!("Error: {err}");

        // Replace an empty assistant placeholder from streaming, or push a new error turn.
        if let Some(turn) = self.session.last_turn_mut()
            && turn.role == ChatRole::Assistant
            && matches!(
                &turn.body,
                TurnBody::Text(TextContent { content, reasoning: None })
                    if content.is_empty()
            )
        {
            turn.body = TurnBody::Text(TextContent {
                content: error_msg,
                reasoning: None,
            });
        } else {
            self.session.push_turn(Turn::assistant(error_msg, None));
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
        self.session.save().ok();
        self.stream_start_index = self.session.total_turns();
        self.auto_scroll.store(true, Ordering::Relaxed);

        let config = llm::SendConfig {
            model,
            workspace: self.system_prompt.workspace.1.clone(),
            system_prompt: self.system_prompt.get_prompt(),
            user_prompt,
            tools: tools::enabled_tools(&self.enabled_tools),
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
        let (exists, agents_md_content) = load_agents_md(&path);
        self.agents_md_exists = exists;
        self.system_prompt.agents_md = (self.agents_md_exists, agents_md_content);
        self.show_restart = env::current_exe()
            .ok()
            .is_some_and(|exe| exe.starts_with(&path));

        paths.insert(0, path);
        paths.truncate(10);
        self.workspace_options = views::build_workspace_options(&paths);
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
            selected_rules: self.selected_rules.clone(),
            preamble_enabled: self.system_prompt.preamble.0,
            rules_enabled: self.system_prompt.rules.0,
            tools_enabled: self.system_prompt.tools.0,
            workspace_enabled: self.system_prompt.workspace.0,
            agents_md_enabled: self.system_prompt.agents_md.0,
            files_enabled: self.system_prompt.files.0,
            date_enabled: self.system_prompt.date.0,
            workspace: self.system_prompt.workspace.1.clone(),
            recent_workspaces: self
                .workspace_options
                .iter()
                .filter(|e| !e.path.as_os_str().is_empty())
                .map(|e| e.path.clone())
                .collect(),
            agent_tools: tools::builtin_tool_names()
                .into_iter()
                .chain(self.custom_tool_names.iter().cloned())
                .map(|name| {
                    let enabled = self.enabled_tools.contains(&name);
                    (name, enabled)
                })
                .collect(),
            font_scale: self.font_scale,
        };
        settings.save();
    }

    /// Read a prompt file (preamble or rules) from disk and return a task
    /// that produces the appropriate `FileResult` message.
    fn select_prompt_file(
        entry: FilepathEntry,
        selected: &mut String,
        on_load: fn(Result<String, String>) -> Message,
    ) -> Task<Message> {
        let FilepathEntry { display, path } = entry;
        *selected = display;
        Task::perform(
            async move { std::fs::read_to_string(&path).map_err(|e| e.to_string()) },
            on_load,
        )
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
                self.agents_md_exists,
                self.tools_expanded,
                self.files_expanded,
                &self.selected_preamble,
                &self.preamble_options,
                &self.selected_rules,
                &self.rules_options,
                &self.workspace_options,
                &self.files_content,
                &self.tools_content,
                &self.enabled_tools,
                &self.builtin_tool_names,
                &self.custom_tool_names,
                &self.user_prompt,
                self.workmode,
                self.streaming,
                &self.session_options,
                &self.session.id,
            ),
            divider(&self.left_divider),
            center_pane(
                &self.center_pane_title,
                self.session.dialogs.as_slice(),
                &self.expanded_turns,
                &self.expanded_dialogs,
                self.get_status(),
                &self.theme,
                self.streaming,
                &self.selectable_msgs,
                self.font_scale,
            ),
            divider(&self.right_divider),
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
                    match key.as_ref() {
                        keyboard::Key::Character("z") => {
                            Some(Message::UndoRedo(textarea::Message::Undo))
                        }
                        keyboard::Key::Character("y") => {
                            Some(Message::UndoRedo(textarea::Message::Redo))
                        }
                        keyboard::Key::Character("=") => Some(Message::Zoom(0.05)),
                        keyboard::Key::Character("-") => Some(Message::Zoom(-0.05)),
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

/// Read `AGENTS.md` from the workspace root, returning (exists, content).
fn load_agents_md(workspace: &std::path::Path) -> (bool, String) {
    if !workspace.as_os_str().is_empty() {
        let path = workspace.join("AGENTS.md");
        if path.is_file() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            return (true, content);
        }
    }
    (false, String::new())
}
