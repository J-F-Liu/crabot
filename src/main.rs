mod adk;
mod chat;
mod model;
mod session;
mod system;
mod tool;
mod tools;
mod user;
mod workspace;

use futures::{SinkExt, future::FutureExt};
use iced::{
    Element, Event, Fill, Font, Length, Point, Size, Subscription, Task, Theme, event, font, mouse,
    widget::{self, column, container, mouse_area, row, rule, scrollable, text, text_editor},
    window,
};
use iced_selection::Text as SelectableText;
use iced_selection::text::Style as SelectionStyle;
use indexmap::IndexMap;
use std::path::PathBuf;

use chat::{DisplayMessage, MessageContent};

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
    StreamContent(String),
    StreamReasoning(String),
    StreamToolResult {
        name: String,
        call_id: String,
        args: serde_json::Value,
        result: String,
    },
    StreamDone(Vec<genai::chat::ChatMessage>),
    StreamError(String, Vec<genai::chat::ChatMessage>),
}

/// Return a widget-operation Task that snaps the message scroll to the end.
fn scroll_to_end() -> Task<Message> {
    iced_runtime::task::widget(iced::advanced::widget::operation::scrollable::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::END.into(),
    ))
}

const MIN_W: f32 = 280.0;
const HANDLE: f32 = 6.0;
const MESSAGE_SCROLL: widget::Id = widget::Id::new("messages");

impl App {
    fn boot() -> (Self, Task<Message>) {
        let providers = model::try_load_models_from_omp()
            .or_else(|_| model::try_load_models_from_pi())
            .unwrap_or_default();
        let dev_tools: IndexMap<DevTool, bool> = DevTool::ALL.iter().map(|&t| (t, true)).collect();
        let tools_summary = tool::tools_summary(&dev_tools);
        let app = Self {
            left_w: 300.0,
            right_w: 400.0,
            window_w: 1200.0,
            cursor: Point::ORIGIN,
            dragging: None,
            providers,
            selected_model: None,
            theme: Theme::SolarizedLight,
            system_prompt: SystemPrompt {
                preamble: (true, String::new()),
                rules: (true, String::new()),
                tools: (true, tools_summary.clone()),
                workspace: (true, PathBuf::new()),
                files: (true, String::new()),
                date: (true, chrono::Local::now().format("%Y-%m-%d").to_string()),
            },
            rules_expanded: false,
            tools_expanded: false,
            files_expanded: false,
            selected_preamble: String::new(),
            preamble_options: system::build_preamble_options(),
            workspace_options: system::build_workspace_options(&[]),
            rules_content: text_editor::Content::new(),
            files_content: text_editor::Content::new(),
            tools_content: text_editor::Content::with_text(&tools_summary),
            dev_tools,
            user_prompt: text_editor::Content::new(),
            workmode: WorkMode::Code,
            session: Session::new(None, PathBuf::new()),
            streaming: false,
            stream_start_index: 0,
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
            }
            Message::SendPrompt => {
                if self.streaming {
                    return Task::none();
                }
                let content = self.user_prompt.text();
                if content.trim().is_empty() {
                    return Task::none();
                }

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
                    && let MessageContent::Text { content, .. } = &mut last.content
                {
                    content.push_str(&chunk);
                }
                return scroll_to_end();
            }
            Message::StreamReasoning(chunk) => {
                self.ensure_assistant_placeholder();
                if let Some(last) = self.session.messages.last_mut()
                    && let MessageContent::Text { reasoning, .. } = &mut last.content
                {
                    reasoning.get_or_insert_with(String::new).push_str(&chunk);
                }
                return scroll_to_end();
            }
            Message::StreamToolResult {
                name,
                call_id,
                args,
                result,
            } => {
                self.session
                    .push(DisplayMessage::tool(name, &args, Some(call_id), result));
                return scroll_to_end();
            }
            Message::StreamDone(genai_messages) => {
                self.handle_stream_done(genai_messages);
                return scroll_to_end();
            }
            Message::StreamError(err, genai_messages) => {
                self.handle_stream_error(err, genai_messages);
                return scroll_to_end();
            }
        }
        Task::none()
    }

    /// Ensure the last message is an assistant Text placeholder for streaming.
    /// If the last message is a Tool message (e.g., after a tool result was
    /// pushed in a subsequent iteration), create a new assistant placeholder
    /// so streamed text/reasoning lands in the right place.
    fn ensure_assistant_placeholder(&mut self) {
        let needs_placeholder = self.session.messages.last().is_none_or(|m| {
            !(m.role == ChatRole::Assistant && matches!(m.content, MessageContent::Text { .. }))
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
            if let MessageContent::Text { content, reasoning } = &mut msg.content
                && let Some(genai_asst) = genai_asst_iter.next()
            {
                if content.is_empty() {
                    *content = genai_asst.content.joined_texts().unwrap_or_default();
                }
                if reasoning.is_none() {
                    *reasoning = genai_asst
                        .content
                        .first_reasoning_content()
                        .map(|s| s.to_string());
                }
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
                    MessageContent::Text { content, reasoning }
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

    fn content_mut(&mut self, name: &str) -> Option<&mut text_editor::Content> {
        match name {
            "Rules" => Some(&mut self.rules_content),
            "Tools" => Some(&mut self.tools_content),
            "Files" => Some(&mut self.files_content),
            _ => None,
        }
    }

    fn selected_model(&self) -> Option<(&Provider, &Model)> {
        let cfg = self.selected_model.as_ref()?;
        let provider = self.providers.iter().find(|p| p.id == cfg.provider_id)?;
        let model = provider.models.iter().find(|m| m.id == cfg.model_id)?;
        Some((provider, model))
    }

    fn view(&self) -> Element<'_, Message> {
        let left_col = column![
            model_config_view(&self.providers, &self.selected_model),
            rule::horizontal(0),
            label("System Prompt", 140.0),
            system::preamble_field_view(
                &self.system_prompt.preamble,
                &self.preamble_options,
                &self.selected_preamble,
            ),
            system::rules_field_view(
                self.rules_expanded,
                &self.system_prompt.rules,
                &self.rules_content,
            ),
            system::tools_field_view(
                self.tools_expanded,
                &self.system_prompt.tools,
                &self.tools_content,
            ),
            system::workspace_field_view(&self.system_prompt.workspace, &self.workspace_options,),
            system::files_field_view(
                self.files_expanded,
                &self.system_prompt.files,
                &self.files_content,
            ),
            system::date_field_view(&self.system_prompt.date),
            session::session_view(),
            label("User Prompt", 140.0),
            user_prompt_view(&self.user_prompt, self.workmode),
            label("Dev Tools", 140.0),
            dev_tools_view(&self.dev_tools),
        ]
        .spacing(8);

        let left_pane = container(left_col.padding(15))
            .width(Length::Fixed(self.left_w))
            .height(Fill)
            .style(pane_side);

        row![
            left_pane,
            divider(),
            container(
                scrollable(
                    column(
                        self.session
                            .messages
                            .iter()
                            .map(|msg| {
                                let role_color: fn(&Theme) -> SelectionStyle = match msg.role {
                                    ChatRole::User => sel_primary,
                                    ChatRole::Tool => sel_primary,
                                    _ => sel_secondary,
                                };
                                container({
                                    let header = match &msg.content {
                                        MessageContent::Tool { name, .. } => {
                                            format!("{} — {}", msg.role, name)
                                        }
                                        _ => msg.role.to_string(),
                                    };
                                    let mut col = column![row![
                                        SelectableText::new(header).size(13).style(role_color),
                                        iced::widget::Space::new().width(Length::Fill),
                                        SelectableText::new(&msg.timestamp)
                                            .size(11)
                                            .style(sel_secondary),
                                    ],];
                                    match &msg.content {
                                        MessageContent::Text { content, reasoning } => {
                                            if let Some(reasoning) = reasoning {
                                                col = col.push(
                                                    SelectableText::new(reasoning)
                                                        .size(13)
                                                        .style(sel_secondary),
                                                );
                                            }
                                            col = col.push(
                                                SelectableText::new(content)
                                                    .size(14)
                                                    .style(sel_default),
                                            );
                                        }
                                        MessageContent::Tool { args, result, .. } => {
                                            col = col.push(
                                                SelectableText::new(args)
                                                    .size(13)
                                                    .style(sel_secondary),
                                            );
                                            col = col.push(
                                                SelectableText::new(result)
                                                    .size(14)
                                                    .style(sel_default),
                                            );
                                        }
                                    }
                                    col.spacing(4).width(Fill)
                                })
                                .width(Fill)
                                .padding(8)
                                .style(|theme: &Theme| {
                                    let p = theme.extended_palette();
                                    container::Style {
                                        background: Some(p.background.base.color.into()),
                                        ..Default::default()
                                    }
                                })
                                .into()
                            })
                            .collect::<Vec<_>>(),
                    )
                    .spacing(8)
                    .padding(10),
                )
                .height(Fill)
                .id(MESSAGE_SCROLL.clone()),
            )
            .width(Fill)
            .height(Fill)
            .style(pane_center),
            divider(),
            pane(
                "Right Pane",
                "◂ Drag to resize",
                Length::Fixed(self.right_w),
                pane_side
            ),
        ]
        .spacing(0)
        .into()
    }

    fn subscription(_state: &Self) -> Subscription<Message> {
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
            Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size.width)),
            _ => None,
        })
    }
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

fn pane<'a>(
    title: &'a str,
    hint: &'a str,
    width: impl Into<Length>,
    style: fn(&Theme) -> container::Style,
) -> Element<'a, Message> {
    container(
        column![text(title).size(26), text(hint).size(14)]
            .spacing(8)
            .padding(20),
    )
    .width(width)
    .height(Fill)
    .style(style)
    .into()
}

// ── pane styles ───────────────────────────────────────────────────

fn pane_side(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.weak.color.into()),
        ..container::Style::default()
    }
}

fn pane_center(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.palette().background.into()),
        ..container::Style::default()
    }
}

// ── selectable text styles ────────────────────────────────────────

fn sel_default(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(theme.palette().text),
        selection: theme.palette().primary,
    }
}

fn sel_primary(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(theme.palette().primary),
        selection: theme.palette().primary,
    }
}

fn sel_secondary(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(theme.extended_palette().secondary.base.color),
        selection: theme.extended_palette().secondary.base.color,
    }
}
