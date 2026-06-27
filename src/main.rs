// Hide the console window in release builds. Debug builds keep the console
// for `println!`/`eprintln!` output during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chat;
mod llm;
mod model;
mod session;
mod settings;
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
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use chat::{TextContent, ToolResult, Turn, TurnBody, replace_emoji};
use genai::chat::{ChatMessage, ChatRole};
use model::{Model, ModelConfig, Provider, TokenAmount};
use session::Session;
use system::{FilepathEntry, RULES, SystemPrompt, TOOLS, WORKSPACE, WORKSPACE_TREE};
use tools::DevTool;
use user::{UserPrompt, WorkMode};
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
    let saved = settings::Settings::load();
    let size = Size::new(
        saved.window_size.0.max(MIN_W),
        saved.window_size.1.max(200.0),
    );
    let position =
        iced::window::Position::Specific(Point::new(saved.window_pos.0, saved.window_pos.1));
    SAVED_SETTINGS.set(saved).ok();
    iced::application(App::boot, App::update, App::view)
        .subscription(App::subscription)
        .theme(|state: &App| state.theme.clone())
        .window_size(size)
        .position(position)
        .title(crabot_title!())
        .antialiasing(true)
        .exit_on_close_request(false)
        .run()
}

static SAVED_SETTINGS: OnceLock<settings::Settings> = OnceLock::new();

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
}

struct App {
    left_w: f32,
    right_w: f32,
    window_size: Size,
    window_pos: Point,
    cursor: Point,
    dragging: Option<Drag>,
    providers: Vec<Provider>,
    selected_model: Option<ModelConfig>,
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
    /// Current phase of the LLM interaction lifecycle.
    streaming: StreamState,
    /// Index (flat turn count) in the session where the current stream's
    /// placeholders begin; used by `handle_stream_done` to backfill captured
    /// content/reasoning into the right display messages.
    stream_start_index: usize,
    /// Indices of tool-result messages whose result body is expanded.
    expanded_tools: HashSet<usize>,
    /// Token usage from the most recent completed LLM response.
    last_usage: genai::chat::Usage,
    /// Last-sent user prompt text, displayed in the center-pane header.
    current_prompt: String,
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
    /// Files modified during this session (insertion order, deduplicated).
    /// Mirrors `session.modified_files` for convenient UI access.
    modified_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    CursorMoved(Point),
    LeftPressed,
    LeftReleased,
    WindowResized(Size),
    WindowMoved(Point),
    SelectProvider(String),
    SelectModel(String),
    ToggleThinking(bool),
    SelectThinkingLevel(String),
    ToggleEnabled(&'static str, bool),
    ToggleExpanded(&'static str),
    EditTextField(&'static str, String),
    EditTextContent(&'static str, text_editor::Action),
    SelectWorkspace(FilepathEntry),
    WorkspaceDialogResult(Option<PathBuf>),
    SelectPreamble(FilepathEntry),
    PreambleFileResult(Result<String, String>),
    ToggleDevTool(String, bool),
    /// An edit action targeting a specific [`TextArea`]. The [`FocusedTarget`]
    /// identifies which text area (rules or user prompt) should receive the
    /// action — a click sets focus, subsequent edits are gated on that focus.
    EditTextArea(FocusedTarget, textarea::Message),
    /// Global undo/redo shortcut (Ctrl+Z / Ctrl+Y). Routed to whichever
    /// [`TextArea`] currently holds keyboard focus.
    UndoRedo(textarea::Message),
    SelectWorkMode(WorkMode),
    NewSession,
    SendPrompt,
    ToggleToolExpand(usize),
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
    CopySession,
    ResendLastPrompt,
    Restart,
    /// Fires when the message scrollable's viewport changes.
    MessageViewScrolled(Viewport),
    /// Toggle a single message between Markdown and selectable plain-text.
    /// `Some(i)` toggles message `i`; `None` clears all (ESC).
    ToggleSelectableMode(Option<usize>),
    /// Track whether the Shift key is currently held.
    ShiftHeld(bool),
}

// ── App impl ──────────────────────────────────────────────────────

impl App {
    fn boot() -> (Self, Task<Message>) {
        let providers = model::try_load_models_from_omp()
            .or_else(|_| model::try_load_models_from_pi())
            .unwrap_or_default();
        let saved = SAVED_SETTINGS
            .get()
            .expect("settings must be loaded in main before boot");

        let dev_tools: IndexMap<DevTool, bool> = saved
            .dev_tools
            .iter()
            .filter_map(|(name, enabled)| DevTool::from_name(name).map(|t| (t, *enabled)))
            .collect();
        // Ensure any newly-added tools default to enabled.
        let dev_tools: IndexMap<DevTool, bool> = DevTool::ALL
            .iter()
            .map(|&t| (t, dev_tools.get(&t).copied().unwrap_or(true)))
            .collect();

        let theme = default_theme();

        let model_for_session = saved.selected_model.clone();
        let workspace_for_session = saved.system_prompt.workspace.1.clone();

        let mut system_prompt = saved.system_prompt.clone();
        // Load preamble content from the .md file, not from saved settings.
        let preamble_options = system::build_preamble_options();
        let preamble_content = preamble_options
            .iter()
            .find(|e| e.display == saved.selected_preamble)
            .map(|e| std::fs::read_to_string(&e.path).unwrap_or_else(|e| e.to_string()))
            .unwrap_or_default();
        system_prompt.preamble.1 = preamble_content;
        system_prompt.date.1 = chrono::Local::now().format("%Y-%m-%d").to_string();

        let show_restart = !workspace_for_session.as_os_str().is_empty()
            && env::current_exe()
                .ok()
                .is_some_and(|exe| exe.starts_with(&workspace_for_session));

        let app = Self {
            left_w: saved.left_w,
            right_w: saved.right_w,
            window_size: Size::new(saved.window_size.0, saved.window_size.1),
            window_pos: Point::new(saved.window_pos.0, saved.window_pos.1),
            cursor: Point::ORIGIN,
            dragging: None,
            providers,
            preamble_options,
            system_prompt,
            theme,
            selected_model: saved.selected_model.clone(),
            rules_expanded: saved.rules_expanded,
            tools_expanded: saved.tools_expanded,
            files_expanded: saved.files_expanded,
            selected_preamble: saved.selected_preamble.clone(),
            workspace_options: system::build_workspace_options(&saved.recent_workspaces),
            rules_content: TextArea::with_text(&saved.rules_text),
            files_content: text_editor::Content::with_text(&saved.files_text),
            tools_content: text_editor::Content::with_text(&saved.tools_text),
            dev_tools,
            user_prompt: TextArea::new(),
            workmode: saved.workmode,
            session: Session::new(model_for_session, workspace_for_session),
            streaming: StreamState::Idle,
            stream_start_index: 0,
            expanded_tools: HashSet::new(),
            last_usage: genai::chat::Usage::default(),
            current_prompt: "New session".into(),
            show_restart,
            cancel_token: Arc::new(AtomicBool::new(false)),
            auto_scroll: Arc::new(AtomicBool::new(true)),
            selectable_msgs: HashSet::new(),
            shift_held: false,
            focused: None,
            modified_files: Vec::new(),
        };
        (app, Task::none())
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
                        let max =
                            (self.window_size.width - self.right_w - gutter - MIN_W).max(MIN_W);
                        self.left_w = (drag.left_start + delta).clamp(MIN_W, max);
                    }
                    Divider::Right => {
                        let max =
                            (self.window_size.width - self.left_w - gutter - MIN_W).max(MIN_W);
                        self.right_w = (drag.right_start - delta).clamp(MIN_W, max);
                    }
                }
            }
            Message::LeftPressed => {
                let left_x = self.left_w;
                let right_x = self.window_size.width - self.right_w - HANDLE;

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
                        left_start: self.left_w,
                        right_start: self.right_w,
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
            Message::SelectProvider(id) => {
                self.selected_model = self.providers.iter().find(|p| p.id == id).and_then(|p| {
                    p.models.first().map(|m| ModelConfig {
                        provider_id: p.id.clone(),
                        model_id: m.id.clone(),
                        thinking: m.thinking,
                        thinking_level: m.thinking_levels.first().cloned().unwrap_or_default(),
                    })
                });
            }
            Message::SelectModel(id) => {
                if let Some(ref mut cfg) = self.selected_model {
                    cfg.model_id = id.clone();
                    cfg.thinking = false;
                    cfg.thinking_level = String::new();
                    if let Some(p) = self.providers.iter().find(|p| p.id == cfg.provider_id)
                        && let Some(m) = p.models.iter().find(|m| m.id == id)
                    {
                        cfg.thinking = m.thinking;
                        cfg.thinking_level = m.thinking_levels.first().cloned().unwrap_or_default();
                    }
                }
            }
            Message::ToggleThinking(enabled) => {
                let supported = self.selected_model().is_some_and(|(_, m)| m.thinking);
                if supported && let Some(ref mut cfg) = self.selected_model {
                    cfg.thinking = enabled;
                }
            }
            Message::SelectThinkingLevel(level) => {
                if let Some(ref mut cfg) = self.selected_model {
                    cfg.thinking_level = level;
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
            }
            Message::WorkspaceDialogResult(Some(path)) => {
                self.set_workspace(path);
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
                let workspace = self.system_prompt.workspace.1.clone();
                self.session = Session::new(self.selected_model.clone(), workspace);
                self.current_prompt = "New session".into();
                self.last_usage = genai::chat::Usage::default();
                self.modified_files.clear();
                self.expanded_tools.clear();
                self.selectable_msgs.clear();
            }
            Message::ToggleToolExpand(idx) => {
                if self.expanded_tools.contains(&idx) {
                    self.expanded_tools.remove(&idx);
                } else {
                    self.expanded_tools.insert(idx);
                }
            }
            Message::SendPrompt => {
                if self.streaming != StreamState::Idle {
                    return Task::none();
                }
                let content = self.user_prompt.text();
                if content.trim().is_empty() {
                    return Task::none();
                }
                let title = Session::derive_title(&content);

                self.current_prompt = content.clone();
                self.auto_scroll.store(true, Ordering::Relaxed);

                let user_prompt = UserPrompt::new(self.workmode, content).get_prompt();

                let Some((provider, model)) = self.selected_model() else {
                    return Task::none();
                };
                let (api_type, api_key, base_url, model_id) = (
                    provider.api_type.clone(),
                    provider.api_key.clone(),
                    provider.base_url.clone(),
                    model.id.clone(),
                );

                let system_prompt = self.system_prompt.get_prompt();
                let tools = DevTool::build_tools(&self.dev_tools);
                let workspace = self.system_prompt.workspace.1.clone();
                let history = self.session.history.clone();

                self.user_prompt.clear();
                self.stream_start_index = self.session.total_turns();
                self.streaming = StreamState::LlmLoading;

                self.cancel_token.store(false, Ordering::Relaxed);
                let cancel_token = self.cancel_token.clone();

                // Update session state with the selected model and workspace before sending the prompt.
                self.session.model = self.selected_model.clone();
                self.session.workspace = workspace.clone();
                self.session.add_dialog(title);
                self.session.push_turn(Turn::user(user_prompt.clone()));

                let config = llm::SendConfig {
                    base_url,
                    api_type,
                    api_key,
                    model_id,
                    workspace,
                    system_prompt,
                    user_prompt,
                    tools,
                    thinking: self.selected_model.as_ref().is_some_and(|m| m.thinking),
                    thinking_level: self
                        .selected_model
                        .as_ref()
                        .map(|m| m.thinking_level.clone())
                        .unwrap_or_default(),
                };

                return Task::batch([
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
                ]);
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
                if tr.result.is_ok()
                    && (tr.name == "write" || tr.name == "edit")
                    && let Some(path_str) = tr.args.get("path").and_then(|v| v.as_str())
                    && !self.modified_files.iter().any(|p| p == path_str)
                {
                    self.modified_files.push(path_str.to_string());
                }
                self.session.push_turn(Turn::from_tool_result(tr));
                return self.maybe_scroll_to_end();
            }
            Message::TokenUsage(usage) => {
                let u = usage.unwrap_or_default();
                let tokens = TokenAmount::from_genai(&u);
                let cost = self.selected_model.as_ref().and_then(|cfg| {
                    self.providers
                        .iter()
                        .find(|p| p.id == cfg.provider_id)
                        .and_then(|p| p.models.iter().find(|m| m.id == cfg.model_id))
                        .map(|m| &m.cost)
                });
                self.session.accumulate_usage(&tokens, cost);
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
            Message::CopySession => {
                return iced::clipboard::write(self.current_prompt.clone());
            }
            Message::ResendLastPrompt => {
                if self.current_prompt != "New session" {
                    self.user_prompt = TextArea::with_text(&self.current_prompt);
                    self.focused = None;
                    return Task::done(Message::SendPrompt);
                }
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
                // While streaming, track whether the user has scrolled away
                // from the bottom (to pause auto-scroll) or back to it (to
                // resume).
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
            .skip(1) // skip user message
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
            left_w: self.left_w,
            right_w: self.right_w,
            window_size: (self.window_size.width, self.window_size.height),
            window_pos: (self.window_pos.x, self.window_pos.y),
            selected_model: self.selected_model.clone(),
            system_prompt: self.system_prompt.clone(),
            rules_expanded: self.rules_expanded,
            tools_expanded: self.tools_expanded,
            files_expanded: self.files_expanded,
            selected_preamble: self.selected_preamble.clone(),
            recent_workspaces: self
                .workspace_options
                .iter()
                .filter(|e| !e.path.as_os_str().is_empty())
                .map(|e| e.path.clone())
                .collect(),
            rules_text: self.rules_content.text(),
            tools_text: self.tools_content.text(),
            files_text: self.files_content.text(),
            dev_tools: self
                .dev_tools
                .iter()
                .map(|(t, &enabled)| (t.name().to_string(), enabled))
                .collect(),
            workmode: self.workmode,
        };
        settings.save();
    }

    fn content_mut(&mut self, name: &str) -> Option<&mut text_editor::Content> {
        match name {
            TOOLS => Some(&mut self.tools_content),
            WORKSPACE_TREE => Some(&mut self.files_content),
            _ => None,
        }
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

    fn selected_model(&self) -> Option<(&Provider, &Model)> {
        let cfg = self.selected_model.as_ref()?;
        let provider = self.providers.iter().find(|p| p.id == cfg.provider_id)?;
        let model = provider.models.iter().find(|m| m.id == cfg.model_id)?;
        Some((provider, model))
    }

    fn view(&self) -> Element<'_, Message> {
        row![
            left_pane(
                self.left_w,
                &self.providers,
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
            ),
            divider(),
            center_pane(
                &self.current_prompt,
                self.session.dialogs_ref(),
                &self.expanded_tools,
                self.get_status(),
                &self.theme,
                self.streaming,
                &self.selectable_msgs,
            ),
            divider(),
            right_pane(
                self.right_w,
                self.selected_model().map(|(_, m)| m.context_window),
                &self.last_usage,
                &self.session.usage,
                self.session.cost,
                &self.modified_files,
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
