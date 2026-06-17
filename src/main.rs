mod model;
mod system;
mod workspace;

use iced::{
    Alignment, Element, Event, Fill, Font, Length, Point, Size, Subscription, Task, Theme, event,
    font, mouse,
    widget::{column, container, mouse_area, pick_list, row, rule, text, text_editor, toggler},
    window,
};
use std::path::PathBuf;

use model::{Model, Provider};
use system::{FilepathEntry, SystemPrompt};

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
    selected_provider: Option<usize>,
    selected_model: Option<usize>,
    thinking_enabled: bool,
    thinking_level: Option<usize>,
    theme: Theme,
    system_prompt: SystemPrompt,
    rules_expanded: bool,
    tools_expanded: bool,
    files_expanded: bool,
    selected_preamble: String,
    preamble_options: Vec<FilepathEntry>,
    workspace_options: Vec<FilepathEntry>,
    rules_content: text_editor::Content,
    tools_content: text_editor::Content,
    files_content: text_editor::Content,
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
    ToggleSystemEnabled(&'static str, bool),
    ToggleSystemExpanded(&'static str),
    EditSystemField(&'static str, String),
    EditSystemContent(&'static str, text_editor::Action),
    SelectWorkspace(FilepathEntry),
    WorkspaceDialogResult(Option<PathBuf>),
    SelectPreamble(FilepathEntry),
    PreambleFileResult(Result<String, String>),
}

const MIN_W: f32 = 280.0;
const HANDLE: f32 = 6.0;

impl App {
    fn boot() -> (Self, Task<Message>) {
        let providers = model::try_load_models_from_omp()
            .or_else(|_| model::try_load_models_from_pi())
            .unwrap_or_default();
        let selected_provider = (!providers.is_empty()).then_some(0);
        let mut app = Self {
            left_w: 300.0,
            right_w: 400.0,
            window_w: 1200.0,
            cursor: Point::ORIGIN,
            dragging: None,
            providers,
            selected_provider,
            selected_model: None,
            thinking_enabled: false,
            thinking_level: None,
            theme: Theme::SolarizedLight,
            system_prompt: SystemPrompt {
                preamble: (true, String::new()),
                rules: (true, String::new()),
                tools: (true, String::new()),
                workspace: (true, String::new()),
                files: (true, String::new()),
                date: (true, String::new()),
            },
            rules_expanded: false,
            tools_expanded: false,
            files_expanded: false,
            selected_preamble: String::new(),
            preamble_options: system::build_preamble_options(),
            workspace_options: system::build_workspace_options(&[]),
            rules_content: text_editor::Content::new(),
            files_content: text_editor::Content::new(),
            tools_content: text_editor::Content::new(),
        };
        app.reselect_model();
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
                self.selected_provider = self.providers.iter().position(|p| p.id == id);
                self.reselect_model();
            }
            Message::SelectModel(id) => {
                if let Some(provider) = self.selected_provider() {
                    self.selected_model = provider.models.iter().position(|m| m.id == id);
                    self.reset_thinking();
                }
            }
            Message::ToggleThinking(enabled) => {
                if self.thinking_supported() {
                    self.thinking_enabled = enabled;
                }
            }
            Message::SelectThinkingLevel(level) => {
                if let Some(model) = self.selected_model() {
                    self.thinking_level = model.thinking_levels.iter().position(|l| *l == level);
                }
            }
            Message::ToggleSystemEnabled(name, enabled) => {
                if let Some(field) = self.system_prompt.get_mut(name) {
                    field.0 = enabled;
                }
            }
            Message::ToggleSystemExpanded(name) => match name {
                "Rules" => self.rules_expanded = !self.rules_expanded,
                "Tools" => self.tools_expanded = !self.tools_expanded,
                "Files" => self.files_expanded = !self.files_expanded,
                _ => {}
            },
            Message::EditSystemField(name, value) => {
                if let Some(field) = self.system_prompt.get_mut(name) {
                    field.1 = value;
                }
            }
            Message::EditSystemContent(name, action) => {
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
                        |maybe_path| {
                            Message::WorkspaceDialogResult(maybe_path)
                        },
                    );
                }
                self.set_workspace(entry.path);
            }
            Message::WorkspaceDialogResult(Some(path)) => {
                self.set_workspace(path);
            }
            Message::WorkspaceDialogResult(None) => {}
            Message::SelectPreamble(entry) => {
                self.selected_preamble = entry.display.clone();
                return Task::perform(
                    async move { std::fs::read_to_string(&entry.path).map_err(|e| e.to_string()) },
                    Message::PreambleFileResult,
                );
            }
            Message::PreambleFileResult(Ok(content)) => {
                self.system_prompt.preamble.1 = content;
            }
            Message::PreambleFileResult(Err(_)) => {}
        }
        Task::none()
    }

    /// Bump `path` to top of recents, persist it as current workspace,
    /// and rebuild the files tree.
    fn set_workspace(&mut self, path: PathBuf) {
        let mut paths: Vec<PathBuf> = self
            .workspace_options
            .iter()
            .filter(|e| !e.path.as_os_str().is_empty())
            .map(|e| e.path.clone())
            .collect();
        paths.retain(|p| p != &path);
        paths.insert(0, path.clone());
        paths.truncate(10);

        self.system_prompt.workspace.1 = path.to_string_lossy().to_string();
        self.system_prompt.files.1 = workspace::build_files_tree(&path);
        self.files_content = text_editor::Content::with_text(&self.system_prompt.files.1);
        self.workspace_options = system::build_workspace_options(&paths);
    }

    fn reselect_model(&mut self) {
        self.selected_model = self
            .selected_provider()
            .and_then(|p| (!p.models.is_empty()).then_some(0));
        self.reset_thinking();
    }

    fn reset_thinking(&mut self) {
        let (thinking, has_levels) = self
            .selected_model()
            .map(|m| (m.thinking, !m.thinking_levels.is_empty()))
            .unwrap_or((false, false));
        self.thinking_enabled = thinking;
        self.thinking_level = has_levels.then_some(0);
    }

    fn thinking_supported(&self) -> bool {
        self.selected_model().is_some_and(|m| m.thinking)
    }

    fn content_mut(&mut self, name: &str) -> Option<&mut text_editor::Content> {
        match name {
            "Rules" => Some(&mut self.rules_content),
            "Tools" => Some(&mut self.tools_content),
            "Files" => Some(&mut self.files_content),
            _ => None,
        }
    }

    fn selected_provider(&self) -> Option<&Provider> {
        self.selected_provider.and_then(|i| self.providers.get(i))
    }

    fn selected_model(&self) -> Option<&Model> {
        self.selected_provider()
            .and_then(|p| self.selected_model.and_then(|i| p.models.get(i)))
    }

    fn thinking_controls(&self) -> Element<'_, Message> {
        let supported = self.thinking_supported();
        let toggle: Element<_> = if supported {
            toggler(self.thinking_enabled)
                .on_toggle(Message::ToggleThinking)
                .into()
        } else {
            mouse_area(toggler(self.thinking_enabled))
                .interaction(mouse::Interaction::None)
                .into()
        };

        if supported {
            let levels: &[String] = self
                .selected_model()
                .map(|m| &*m.thinking_levels)
                .unwrap_or(&[]);
            let selected_level = self.thinking_level.and_then(|i| levels.get(i));
            row![
                label("Thinking", 60.0),
                toggle,
                text("Level").size(14),
                pick_list(levels, selected_level, Message::SelectThinkingLevel).width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            row![label("Thinking", 60.0), toggle]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let selected = self.selected_provider();
        let models: &[Model] = selected.map(|p| &*p.models).unwrap_or(&[]);
        let selected_model = self.selected_model();

        let mut left_col = column![
            row![
                label("Provider", 60.0),
                pick_list(&self.providers[..], selected, |p| Message::SelectProvider(
                    p.id
                ),)
                .width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                label("Model", 60.0),
                pick_list(models, selected_model, |m| Message::SelectModel(m.id)).width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            self.thinking_controls(),
            rule::horizontal(0),
        ]
        .spacing(8);

        left_col = left_col.push(label("System Prompt", 140.0));
        left_col = left_col.push(system::preamble_field_view(
            &self.system_prompt.preamble,
            &self.preamble_options,
            &self.selected_preamble,
        ));
        left_col = left_col.push(system::rules_field_view(
            self.rules_expanded,
            &self.system_prompt.rules,
            &self.rules_content,
        ));
        left_col = left_col.push(system::tools_field_view(
            self.tools_expanded,
            &self.system_prompt.tools,
            &self.tools_content,
        ));
        left_col = left_col.push(system::workspace_field_view(
            &self.system_prompt.workspace,
            &self.workspace_options,
        ));
        left_col = left_col.push(system::files_field_view(
            self.files_expanded,
            &self.system_prompt.files,
            &self.files_content,
        ));
        left_col = left_col.push(system::date_field_view(&self.system_prompt.date));

        let left_pane = container(left_col.padding(15))
            .width(Length::Fixed(self.left_w))
            .height(Fill)
            .style(pane_side);

        row![
            left_pane,
            divider(),
            pane("Center Pane", "◂ Drag ▸ to resize", Fill, pane_center),
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
