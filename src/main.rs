mod adk;
mod chat;
mod model;
mod session;
mod settings;
mod system;
mod tool;
mod tools;
mod user;
mod workspace;

use futures::{SinkExt, future::FutureExt};
use iced::{
    Background, Border, Color, Element, Event, Fill, Font, Length, Point, Size, Subscription, Task,
    Theme,
    advanced::text::Highlight,
    alignment, event, font, mouse,
    widget::{
        self, Space, button, checkbox, column, container, markdown, mouse_area, row, rule,
        scrollable, text, text_editor, toggler,
    },
    window,
};
use iced_selection::Text as SelectableText;
use iced_selection::text::Style as SelectionStyle;
use indexmap::IndexMap;
use std::collections::HashSet;
use std::path::PathBuf;

use chat::{DisplayMessage, MessageContent, TextContent, ToolResult};

use genai::chat::ChatRole;
use model::{Model, ModelConfig, Provider, model_config_view};
use session::Session;
use system::{FilepathEntry, SystemPrompt};
use tool::dev_tools_view;
use tools::DevTool;
use user::{UserPrompt, WorkMode, user_prompt_view};

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .subscription(App::subscription)
        .theme(|state: &App| state.theme.clone())
        .window_size(Size::new(1200.0, 800.0))
        .title("Crabot")
        .antialiasing(true)
        .exit_on_close_request(false)
        .run()
}

// ── constants ─────────────────────────────────────────────────────

const MIN_W: f32 = 280.0;
const HANDLE: f32 = 6.0;
const MESSAGE_SCROLL: widget::Id = widget::Id::new("messages");

// ── theme colors ─────────────────────────────────────────────

const CRABOT_BG: Color = Color::from_rgb8(0xF0, 0xF0, 0xF0);
const CRABOT_PANEL: Color = Color::from_rgb8(0xF2, 0xF2, 0xF2);
const CRABOT_SURFACE: Color = Color::from_rgb8(0xE8, 0xE8, 0xE8);
const CRABOT_PRIMARY: Color = Color::from_rgb8(0x1A, 0x9A, 0x8C);
const CRABOT_PRIMARY_HOVER: Color = Color::from_rgb8(0x15, 0x8C, 0x7F);
const CRABOT_PRIMARY_PRESSED: Color = Color::from_rgb8(0x11, 0x7A, 0x70);
const CRABOT_TEXT: Color = Color::from_rgb8(0x33, 0x33, 0x33);
const CRABOT_TEXT_MUTED: Color = Color::from_rgb8(0x66, 0x66, 0x66);

fn crabot_palette() -> iced::theme::Palette {
    iced::theme::Palette {
        background: CRABOT_BG,
        text: CRABOT_TEXT,
        primary: CRABOT_PRIMARY,
        success: Color::from_rgb8(0x4C, 0xAF, 0x50),
        warning: Color::from_rgb8(0xFF, 0xA0, 0x00),
        danger: Color::from_rgb8(0xE8, 0x4E, 0x4E),
    }
}

fn default_theme() -> Theme {
    Theme::custom("Crabot Light", crabot_palette())
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

struct App {
    left_w: f32,
    right_w: f32,
    window_w: f32,
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
    rules_content: text_editor::Content,
    files_content: text_editor::Content,
    tools_content: text_editor::Content,
    dev_tools: IndexMap<DevTool, bool>,
    user_prompt: text_editor::Content,
    workmode: WorkMode,
    session: Session,
    /// Whether a streaming response is currently in progress.
    streaming: bool,
    /// Index in `session.messages` where the current stream's assistant
    /// placeholders begin; used by `handle_stream_done` to backfill captured
    /// content/reasoning into the right display messages.
    stream_start_index: usize,
    /// Indices of tool-result messages whose result body is expanded.
    expanded_tools: HashSet<usize>,
    /// Token usage from the most recent completed LLM response.
    last_usage: Option<genai::chat::Usage>,
    /// Last-sent user prompt text, displayed in the center-pane header.
    current_prompt: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    CursorMoved(Point),
    LeftPressed,
    LeftReleased,
    WindowResized(f32),
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
    EditUserPrompt(text_editor::Action),
    SelectWorkMode(WorkMode),
    NewSession,
    SendPrompt,
    ToggleToolExpand(usize),
    StreamContent(String),
    StreamReasoning(String),
    StreamToolResult(ToolResult),
    TokenUsage(Option<genai::chat::Usage>),
    StreamDone(Vec<genai::chat::ChatMessage>),
    StreamError(String, Vec<genai::chat::ChatMessage>),
    AppClosing,
    Noop,
}

// ── App impl ──────────────────────────────────────────────────────

impl App {
    fn boot() -> (Self, Task<Message>) {
        let providers = model::try_load_models_from_omp()
            .or_else(|_| model::try_load_models_from_pi())
            .unwrap_or_default();
        let saved = settings::Settings::load();

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

        let app = Self {
            left_w: saved.left_w,
            right_w: saved.right_w,
            window_w: saved.window_w,
            cursor: Point::ORIGIN,
            dragging: None,
            providers,
            selected_model: saved.selected_model,
            theme,
            system_prompt: saved.system_prompt.clone(),
            rules_expanded: saved.rules_expanded,
            tools_expanded: saved.tools_expanded,
            files_expanded: saved.files_expanded,
            selected_preamble: saved.selected_preamble,
            preamble_options: system::build_preamble_options(),
            workspace_options: system::build_workspace_options(&saved.recent_workspaces),
            rules_content: text_editor::Content::with_text(&saved.rules_text),
            files_content: text_editor::Content::with_text(&saved.files_text),
            tools_content: text_editor::Content::with_text(&saved.tools_text),
            dev_tools,
            user_prompt: text_editor::Content::new(),
            workmode: saved.workmode,
            session: Session::new(model_for_session, workspace_for_session),
            streaming: false,
            stream_start_index: 0,
            expanded_tools: HashSet::new(),
            last_usage: None,
            current_prompt: String::new(),
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
                        let max = (self.window_w - self.right_w - gutter - MIN_W).max(MIN_W);
                        self.left_w = (drag.left_start + delta).clamp(MIN_W, max);
                    }
                    Divider::Right => {
                        let max = (self.window_w - self.left_w - gutter - MIN_W).max(MIN_W);
                        self.right_w = (drag.right_start - delta).clamp(MIN_W, max);
                    }
                }
            }
            Message::LeftPressed => {
                let left_x = self.left_w;
                let right_x = self.window_w - self.right_w - HANDLE;

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
            Message::WindowResized(w) => {
                self.window_w = w;
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
                if name == "Workspace" {
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
                "Rules" => self.rules_expanded = !self.rules_expanded,
                "Tools" => self.tools_expanded = !self.tools_expanded,
                "Files" => self.files_expanded = !self.files_expanded,
                _ => {}
            },
            Message::EditTextField(name, value) => {
                if let Some(field) = self.system_prompt.get_mut(name) {
                    field.1 = value;
                }
            }
            Message::EditTextContent(name, action) => {
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
            Message::EditUserPrompt(action) => {
                self.user_prompt.perform(action);
            }
            Message::SelectWorkMode(mode) => {
                self.workmode = mode;
            }
            Message::NewSession => {
                let workspace = self.system_prompt.workspace.1.clone();
                self.session = Session::new(self.selected_model.clone(), workspace);
                self.current_prompt.clear();
            }
            Message::ToggleToolExpand(idx) => {
                if self.expanded_tools.contains(&idx) {
                    self.expanded_tools.remove(&idx);
                } else {
                    self.expanded_tools.insert(idx);
                }
            }
            Message::SendPrompt => {
                if self.streaming {
                    return Task::none();
                }
                let content = self.user_prompt.text();
                if content.trim().is_empty() {
                    return Task::none();
                }

                self.current_prompt = content.clone();

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

                self.user_prompt = text_editor::Content::new();
                self.stream_start_index = self.session.messages.len();
                self.session.push(DisplayMessage::user(user_prompt.clone()));
                self.streaming = true;

                // Update session model info
                self.session.model = self.selected_model.clone();
                self.session.workspace = workspace.clone();

                let config = adk::SendConfig {
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

                return Task::stream(iced::stream::channel(128, async move |sender| {
                    let mut callback = {
                        move |msg: Message| {
                            let mut sender = sender.clone();
                            async move { sender.send(msg).await.is_ok() }.boxed()
                        }
                    };
                    adk::send_stream(config, history, &mut callback).await;
                }));
            }
            Message::StreamContent(chunk) => {
                self.ensure_assistant_placeholder();
                if let Some(last) = self.session.messages.last_mut()
                    && let MessageContent::Text(tc) = &mut last.content
                {
                    tc.content.push_str(&chunk);
                }
                // Refresh the markdown cache for the last message.
                if let Some(last) = self.session.messages.last_mut() {
                    last.refresh_md_cache();
                }
                return scroll_to_end();
            }
            Message::StreamReasoning(chunk) => {
                self.ensure_assistant_placeholder();
                if let Some(last) = self.session.messages.last_mut()
                    && let MessageContent::Text(tc) = &mut last.content
                {
                    tc.reasoning
                        .get_or_insert_with(String::new)
                        .push_str(&chunk);
                }
                // Refresh the markdown cache for the last message.
                if let Some(last) = self.session.messages.last_mut() {
                    last.refresh_md_cache();
                }
                return scroll_to_end();
            }
            Message::StreamToolResult(tr) => {
                self.session.push(DisplayMessage::from_tool_result(tr));
                return scroll_to_end();
            }
            Message::TokenUsage(usage) => {
                self.last_usage = usage;
            }
            Message::StreamDone(genai_messages) => {
                self.handle_stream_done(genai_messages);
                return scroll_to_end();
            }
            Message::StreamError(err, genai_messages) => {
                self.handle_stream_error(err, genai_messages);
                return scroll_to_end();
            }
            Message::AppClosing => {
                self.save_settings();
                return iced::exit();
            }
            Message::Noop => {}
        }
        Task::none()
    }

    /// Ensure the last message is an assistant Text placeholder for streaming.
    /// If the last message is a Tool message (e.g., after a tool result was
    /// pushed in a subsequent iteration), create a new assistant placeholder
    /// so streamed text/reasoning lands in the right place.
    fn ensure_assistant_placeholder(&mut self) {
        let needs_placeholder = self.session.messages.last().is_none_or(|m| {
            !(m.role == ChatRole::Assistant && matches!(m.content, MessageContent::Text(_)))
        });
        if needs_placeholder {
            self.session
                .push(DisplayMessage::assistant(String::new(), None));
        }
    }

    /// Backfill streaming placeholders with captured content from genai,
    /// extend session history, and persist the session.
    fn handle_stream_done(&mut self, genai_messages: Vec<genai::chat::ChatMessage>) {
        self.streaming = false;

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

        for msg in self
            .session
            .messages
            .iter_mut()
            .skip(self.stream_start_index)
        {
            if msg.role != ChatRole::Assistant {
                continue;
            }
            if let MessageContent::Text(tc) = &mut msg.content
                && let Some(genai_asst) = genai_asst_iter.next()
            {
                if tc.content.is_empty() {
                    tc.content = genai_asst.content.joined_texts().unwrap_or_default();
                }
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
    fn handle_stream_error(&mut self, err: String, genai_messages: Vec<genai::chat::ChatMessage>) {
        self.streaming = false;

        // Preserve any messages generated before the error (user msg,
        // partial assistant turns, tool calls/responses) in the history
        // so subsequent requests still carry valid context.
        self.session.history.extend(genai_messages);

        let is_empty_placeholder = self.session.messages.last().is_some_and(|m| {
            m.role == ChatRole::Assistant
                && matches!(
                    &m.content,
                    MessageContent::Text(TextContent { content, reasoning })
                        if content.is_empty() && reasoning.is_none()
                )
        });

        if is_empty_placeholder {
            if let Some(last) = self.session.messages.last_mut() {
                *last = DisplayMessage::assistant(format!("Error: {err}"), None);
            }
        } else {
            self.session
                .push(DisplayMessage::assistant(format!("Error: {err}"), None));
        }
        let _ = self.session.save();
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

        paths.insert(0, path);
        paths.truncate(10);
        self.workspace_options = system::build_workspace_options(&paths);
    }

    /// Collect current app state into `Settings` and persist to disk.
    fn save_settings(&self) {
        let settings = settings::Settings {
            left_w: self.left_w,
            right_w: self.right_w,
            window_w: self.window_w,
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
            "Rules" => Some(&mut self.rules_content),
            "Tools" => Some(&mut self.tools_content),
            "Files" => Some(&mut self.files_content),
            _ => None,
        }
    }

    fn get_status(&self) -> &str {
        if self.streaming {
            "🔄 Thinking…"
        } else if self.session.messages.is_empty() {
            "Type a message to begin"
        } else {
            "✅ Ready"
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
            left_pane(self),
            divider(),
            center_pane(
                &self.current_prompt,
                &self.session.messages,
                &self.expanded_tools,
                self.get_status(),
                &self.theme,
            ),
            divider(),
            right_pane(
                Length::Fixed(self.right_w),
                pane_side,
                self.last_usage.as_ref(),
                self.selected_model().map(|(_, m)| m.context_window),
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
                Event::Window(window::Event::Resized(size)) => {
                    Some(Message::WindowResized(size.width))
                }
                _ => None,
            }),
            window::close_requests().map(|_id| Message::AppClosing),
        ])
    }
}

// ── free functions (widget constructors) ──────────────────────────

/// Return a widget-operation Task that snaps the message scroll to the end.
fn scroll_to_end() -> Task<Message> {
    iced_runtime::task::widget(iced::advanced::widget::operation::scrollable::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::END.into(),
    ))
}

fn divider() -> Element<'static, Message> {
    mouse_area(rule::vertical(HANDLE))
        .interaction(mouse::Interaction::ResizingHorizontally)
        .into()
}

// ── pane helpers ──────────────────────────────────────────────────

fn label<'a>(text: &'a str, width: impl Into<Length>) -> Element<'a, Message> {
    container(iced::widget::text(text).size(14).font(Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .width(width)
    .into()
}

fn left_pane(app: &App) -> Element<'_, Message> {
    let col = column![
        model_config_view(&app.providers, &app.selected_model),
        rule::horizontal(0),
        label("System Prompt", 140.0),
        system::preamble_field_view(
            &app.system_prompt.preamble,
            &app.preamble_options,
            &app.selected_preamble,
        ),
        system::rules_field_view(
            app.rules_expanded,
            &app.system_prompt.rules,
            &app.rules_content,
        ),
        system::tools_field_view(
            app.tools_expanded,
            &app.system_prompt.tools,
            &app.tools_content,
        ),
        system::workspace_field_view(&app.system_prompt.workspace, &app.workspace_options,),
        system::files_field_view(
            app.files_expanded,
            &app.system_prompt.files,
            &app.files_content,
        ),
        system::date_field_view(&app.system_prompt.date),
        session::session_view(),
        label("User Prompt", 140.0),
        user_prompt_view(&app.user_prompt, app.workmode),
        label("Dev Tools", 140.0),
        dev_tools_view(&app.dev_tools),
    ]
    .spacing(8);

    container(col.padding(15))
        .width(Length::Fixed(app.left_w))
        .height(Fill)
        .style(pane_side)
        .into()
}

fn center_pane<'a>(
    current_prompt: &'a str,
    messages: &'a [DisplayMessage],
    expanded_tools: &'a HashSet<usize>,
    status: &'a str,
    theme: &'a Theme,
) -> Element<'a, Message> {
    container(column![
        session_header(current_prompt),
        scrollable(
            column(
                messages
                    .iter()
                    .enumerate()
                    .map(|(i, msg)| {
                        container({
                            let is_tool = matches!(&msg.content, MessageContent::Tool(_));
                            let expanded = is_tool && expanded_tools.contains(&i);
                            let indicator = if is_tool {
                                if expanded { "▼" } else { "▶" }
                            } else {
                                ""
                            };
                            let (header, is_edit_or_write, _) = match &msg.content {
                                MessageContent::Tool(ToolResult { name, result, .. }) => {
                                    let status_icon = match result {
                                        Ok(_) => " ✓",
                                        Err(_) => " ✗",
                                    };
                                    let hdr = format!(
                                        "{} {} — {}{}",
                                        indicator, msg.role, name, status_icon
                                    );
                                    let is_ew = name == "edit" || name == "write";
                                    (hdr, is_ew, name.as_str())
                                }
                                _ => (msg.role.to_string(), false, ""),
                            };
                            let header_text = text(header).size(13).color(CRABOT_TEXT);
                            let ts_text = SelectableText::new(&msg.timestamp)
                                .size(11)
                                .style(sel_secondary);
                            let mut col = if is_tool {
                                let header_row =
                                    row![header_text, Space::new().width(Length::Fill), ts_text,];
                                column![
                                    mouse_area(header_row)
                                        .on_press(Message::ToggleToolExpand(i))
                                        .interaction(mouse::Interaction::Pointer),
                                ]
                            } else {
                                column![row![
                                    header_text,
                                    Space::new().width(Length::Fill),
                                    ts_text,
                                ],]
                            };
                            match &msg.content {
                                MessageContent::Text(TextContent { content, reasoning }) => {
                                    if let Some(reasoning) = reasoning {
                                        col = col.push(
                                            SelectableText::new(reasoning)
                                                .size(13)
                                                .style(sel_secondary),
                                        );
                                    }
                                    if let Some(md) = &msg.content_md {
                                        let mut md_style = markdown::Style::from(theme.clone());
                                        md_style.inline_code_highlight = Highlight {
                                            background: Background::Color(Color::TRANSPARENT),
                                            border: Border::default(),
                                        };
                                        md_style.inline_code_padding = 0.into();
                                        md_style.inline_code_color = color_text(theme);
                                        col = col.push(
                                            markdown::view(
                                                md.items(),
                                                markdown::Settings::with_text_size(14, md_style),
                                            )
                                            .map(|_| Message::Noop),
                                        );
                                    } else {
                                        col = col.push(
                                            SelectableText::new(content)
                                                .size(14)
                                                .style(sel_default),
                                        );
                                    }
                                }
                                MessageContent::Tool(ToolResult { args, result, .. }) => {
                                    if is_edit_or_write {
                                        if expanded {
                                            col = col.extend(args_rows(args));
                                            col = col.push(result_text(result));
                                        } else if let Some(row) = path_arg_row(args) {
                                            col = col.push(row);
                                        }
                                    } else {
                                        col = col.extend(args_rows(args));
                                        if expanded {
                                            col = col.push(result_text(result));
                                        }
                                    }
                                }
                            }
                            col.spacing(4).width(Fill)
                        })
                        .width(Fill)
                        .padding(8)
                        .style(|_theme: &Theme| container::Style::default())
                        .into()
                    })
                    .collect::<Vec<_>>(),
            )
            .spacing(8)
            .padding(10),
        )
        .height(Fill)
        .id(MESSAGE_SCROLL.clone()),
        status_line(status),
    ])
    .width(Fill)
    .height(Fill)
    .style(pane_center)
    .into()
}

/// Label-value row with the value right-aligned via a fill spacer.
fn token_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    row![
        text(label).size(16),
        Space::new().width(Length::Fill),
        text(value).size(16),
    ]
    .into()
}

fn right_pane<'a>(
    width: impl Into<Length>,
    style: fn(&Theme) -> container::Style,
    usage: Option<&genai::chat::Usage>,
    context_window: Option<u64>,
) -> Element<'a, Message> {
    let mut col = column![].spacing(8);

    if let (Some(u), Some(cw)) = (usage, context_window) {
        let prompt_tokens = u.prompt_tokens.unwrap_or(0);
        let cached_tokens = u
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0);
        let total_tokens = u.total_tokens.unwrap_or(0);
        let pct = ((prompt_tokens as u64) * 100).checked_div(cw).unwrap_or(0);
        col = col
            .push(rule::horizontal(1))
            .push(text("Token Usage").size(14).font(Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }))
            .push(token_row("Prompt tokens:", format!("{prompt_tokens}")))
            .push(token_row("Cached tokens:", format!("{cached_tokens}")))
            .push(token_row("Total tokens:", format!("{total_tokens}")))
            .push(token_row("Context window:", format!("{cw}")))
            .push(token_row("Window used:", format!("{pct}%")));
    }

    container(col.padding(20))
        .width(width)
        .height(Fill)
        .style(style)
        .into()
}

// ── pane styles ───────────────────────────────────────────────────

fn pane_side(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(CRABOT_PANEL.into()),
        ..container::Style::default()
    }
}

fn pane_center(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        ..container::Style::default()
    }
}

// ── session header ──────────────────────────────────────────────────

/// Header bar at the top of the center pane displaying the current
/// user-prompt text (if any).
fn session_header<'a>(prompt: &'a str) -> Element<'a, Message> {
    let display_text = if prompt.is_empty() {
        "New session"
    } else {
        prompt
    };
    container(
        SelectableText::new(display_text)
            .size(14)
            .style(|theme: &Theme| {
                let p = theme.extended_palette();
                SelectionStyle {
                    color: Some(CRABOT_TEXT),
                    selection: p.primary.base.color,
                }
            }),
    )
    .width(Fill)
    .padding([8, 12])
    .style(|_theme: &Theme| container::Style {
        background: Some(CRABOT_SURFACE.into()),
        ..container::Style::default()
    })
    .into()
}

// ── status line ───────────────────────────────────────────────────

fn status_line<'a>(status_text: &'a str) -> Element<'a, Message> {
    container(text(status_text).size(12).color(CRABOT_TEXT_MUTED))
        .width(Fill)
        .align_x(alignment::Horizontal::Center)
        .padding([4, 10])
        .style(|_theme: &Theme| container::Style {
            background: Some(CRABOT_SURFACE.into()),
            ..container::Style::default()
        })
        .into()
}

// ── button styles ───────────────────────────────────────────────

pub fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: Some(CRABOT_PRIMARY.into()),
        text_color: Color::WHITE,
        border: Border::default().rounded(6),
        ..button::Style::default()
    };
    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style {
            background: Some(CRABOT_PRIMARY_HOVER.into()),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(CRABOT_PRIMARY_PRESSED.into()),
            ..base
        },
        button::Status::Disabled => button::Style {
            background: Some(CRABOT_PRIMARY.scale_alpha(0.5).into()),
            ..base
        },
    }
}

pub fn primary_toggler(_theme: &Theme, status: toggler::Status) -> toggler::Style {
    let base = toggler::Style {
        background: CRABOT_SURFACE.into(),
        background_border_width: 1.0,
        background_border_color: Color::from_rgb8(0xC0, 0xC0, 0xC0),
        foreground: Color::WHITE.into(),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(CRABOT_TEXT),
        border_radius: None,
        padding_ratio: 0.3,
    };
    match status {
        toggler::Status::Active { is_toggled }
        | toggler::Status::Hovered { is_toggled }
        | toggler::Status::Disabled { is_toggled } => {
            let mut style = base;
            if is_toggled {
                style.background = CRABOT_PRIMARY.into();
                style.background_border_color = CRABOT_PRIMARY;
            }
            if matches!(status, toggler::Status::Hovered { .. }) {
                style.background = if is_toggled {
                    CRABOT_PRIMARY_HOVER.into()
                } else {
                    Color::from_rgb8(0xD8, 0xD8, 0xD8).into()
                };
                style.background_border_color = if is_toggled {
                    CRABOT_PRIMARY_HOVER
                } else {
                    Color::from_rgb8(0xA8, 0xA8, 0xA8)
                };
            }
            style
        }
    }
}

pub fn primary_checkbox(_theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let base = checkbox::Style {
        background: Color::WHITE.into(),
        icon_color: Color::WHITE,
        border: Border::default()
            .rounded(4)
            .width(1)
            .color(Color::from_rgb8(0xB0, 0xB0, 0xB0)),
        text_color: Some(CRABOT_TEXT),
    };
    match status {
        checkbox::Status::Active { is_checked }
        | checkbox::Status::Hovered { is_checked }
        | checkbox::Status::Disabled { is_checked } => {
            let mut style = base;
            if is_checked {
                style.background = CRABOT_PRIMARY.into();
                style.border = Border::default().rounded(4).width(1).color(CRABOT_PRIMARY);
                style.icon_color = Color::WHITE;
            }
            if matches!(status, checkbox::Status::Hovered { .. }) && is_checked {
                style.background = CRABOT_PRIMARY_HOVER.into();
                style.border = Border::default()
                    .rounded(4)
                    .width(1)
                    .color(CRABOT_PRIMARY_HOVER);
            }
            style
        }
    }
}

fn color_text(theme: &Theme) -> iced::Color {
    theme.palette().text
}
fn color_primary(theme: &Theme) -> iced::Color {
    theme.palette().primary
}
fn color_secondary(theme: &Theme) -> iced::Color {
    theme.extended_palette().secondary.base.color
}

// ── selectable text styles ────────────────────────────────────────

fn sel_default(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(color_text(theme)),
        selection: color_primary(theme),
    }
}

fn sel_primary(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(color_primary(theme)),
        selection: color_primary(theme),
    }
}

fn sel_secondary(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(color_secondary(theme)),
        selection: color_secondary(theme),
    }
}

// ── tool message rendering helpers ────────────────────────────────

/// Single tool-argument key-value row.
fn arg_row<'a>(key: &'a str, value: String) -> Element<'a, Message> {
    row![
        SelectableText::new(key).size(12).style(sel_primary),
        Space::new().width(8),
        SelectableText::new(value).size(12).style(sel_secondary),
    ]
    .spacing(0)
    .into()
}

/// All tool-argument rows.
fn args_rows(args: &serde_json::Value) -> Vec<Element<'_, Message>> {
    let Some(map) = args.as_object() else {
        return Vec::new();
    };
    map.iter()
        .map(|(k, v)| {
            let val = v
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string());
            arg_row(k, val)
        })
        .collect()
}

/// Only the "path" argument row, when present.
fn path_arg_row(args: &serde_json::Value) -> Option<Element<'_, Message>> {
    let path = args.as_object()?.get("path")?.as_str()?;
    Some(arg_row("path", path.to_string()))
}

/// Tool result text (success or error).
fn result_text(result: &Result<String, String>) -> Element<'_, Message> {
    let display = result.clone().unwrap_or_else(|e| e);
    let style = if result.is_ok() {
        sel_default
    } else {
        sel_secondary
    };
    SelectableText::new(display).size(14).style(style).into()
}
