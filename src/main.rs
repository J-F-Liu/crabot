mod model;

use iced::{
    Alignment, Element, Event, Fill, Font, Length, Point, Size, Subscription, Task, Theme, event,
    font, mouse, window,
    widget::{column, container, mouse_area, pick_list, row, rule, text, toggler},
};
use model::Provider;

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

// ── application state ─────────────────────────────────────────────

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
}
#[derive(Debug, Clone)]
enum Message {
    CursorMoved(Point),
    LeftPressed,
    LeftReleased,
    WindowResized(f32),
    SelectProvider(String),
    SelectModel(String),
    ToggleThinking(bool),
    SelectThinkingLevel(String),
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
                if let Some(index) = self.selected_provider {
                    self.selected_model =
                        self.providers[index].models.iter().position(|m| m.id == id);
                    self.reset_thinking();
                }
            }
            Message::ToggleThinking(enabled) => {
                if self.thinking_supported() {
                    self.thinking_enabled = enabled;
                }
            }
            Message::SelectThinkingLevel(level) => {
                if let (Some(pi), Some(mi)) = (self.selected_provider, self.selected_model) {
                    self.thinking_level = self.providers[pi].models[mi]
                        .thinking_levels
                        .iter()
                        .position(|l| *l == level);
                }
            }
        }
        Task::none()
    }

    fn reselect_model(&mut self) {
        self.selected_model = self
            .selected_provider
            .and_then(|i| (!self.providers[i].models.is_empty()).then_some(0));
        self.reset_thinking();
    }

    fn reset_thinking(&mut self) {
        let model = self
            .selected_provider
            .and_then(|pi| self.providers[pi].models.get(self.selected_model?));
        self.thinking_enabled = model.is_some_and(|m| m.thinking);
        self.thinking_level = model.and_then(|m| (!m.thinking_levels.is_empty()).then_some(0));
    }

    fn thinking_supported(&self) -> bool {
        self.selected_provider
            .and_then(|pi| {
                self.selected_model
                    .and_then(|mi| self.providers[pi].models.get(mi))
            })
            .is_some_and(|m| m.thinking)
    }

    fn view(&self) -> Element<'_, Message> {
        let selected: Option<&Provider> =
            self.selected_provider.and_then(|i| self.providers.get(i));

        let models: &[model::Model] = match self.selected_provider {
            Some(i) => &self.providers[i].models[..],
            None => &[],
        };
        let selected_model: Option<&model::Model> = self.selected_model.and_then(|i| models.get(i));

        let levels: &[String] = match (self.selected_provider, self.selected_model) {
            (Some(pi), Some(mi)) => &self.providers[pi].models[mi].thinking_levels[..],
            _ => &[],
        };
        let selected_level: Option<&String> = self.thinking_level.and_then(|i| levels.get(i));

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

        let thinking_row: Element<_> = if supported {
            row![
                label("Thinking"),
                toggle,
                text("Level").size(14),
                pick_list(levels, selected_level, Message::SelectThinkingLevel).width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            row![label("Thinking"), toggle]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
        };

        let left_pane = container(
            column![
                row![
                    label("Provider"),
                    pick_list(&self.providers[..], selected, |p| Message::SelectProvider(
                        p.id
                    ),)
                    .width(Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    label("Model"),
                    pick_list(models, selected_model, |m| Message::SelectModel(m.id)).width(Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                thinking_row,
                rule::horizontal(0),
            ]
            .spacing(8)
            .padding(15),
        )
        .width(Length::Fixed(self.left_w))
        .height(Fill)
        .style(pane_side);

        row![
            left_pane,
            divider(),
            pane(
                "Center Pane",
                "◂ Drag ▸ to resize",
                Fill,
                pane_center
            ),
            divider(),
            pane("Right Pane", "◂ Drag to resize", Length::Fixed(self.right_w), pane_side),
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
            Event::Window(window::Event::Resized(size)) => {
                Some(Message::WindowResized(size.width))
            }
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

fn label(text: &str) -> Element<'_, Message> {
    container(iced::widget::text(text).size(14).font(Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .width(Length::Fixed(80.0))
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
