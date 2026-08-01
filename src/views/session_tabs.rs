use iced::widget::scrollable::{AbsoluteOffset, Direction, Scrollbar};
use iced::widget::{Id, operation};
use iced::{
    Alignment, Color, Element, Font, Length, Padding, Task, font, mouse,
    widget::{Space, button, container, mouse_area, row, scrollable, svg, text, tooltip},
};

use super::icons::{CHEVRON_LEFT, CHEVRON_RIGHT, CLOSE};
use super::styles::{bordered_bar_style, session_tab_style, tab_close_button_style, tooltip_style};
use super::theme;
use crate::app::{ConversationState, SessionEndStatus, SessionTab, conversation::TabBarDirection};
use crate::{CenterPaneEvent, ConversationEvent};

/// Height of the tab bar, in logical pixels.
const BAR_HEIGHT: f32 = 30.0;
/// Vertical padding above the tabs inside the bar (tabs extend to the bar's bottom edge).
const BAR_VPAD: f32 = 3.0;
/// Height of a single tab — fills the bar down to its bottom border.
const TAB_HEIGHT: f32 = BAR_HEIGHT - BAR_VPAD;
/// Edge length of the square close button inside a tab.
const CLOSE_SIZE: f32 = 16.0;
/// Widget id for the tab bar scrollable — used to programmatically scroll it.
const TAB_BAR_ID: Id = Id::new("tab-bar");
/// Horizontal scroll step in pixels per arrow-click.
pub(crate) const TAB_SCROLL_STEP: f32 = 120.0;

/// Return a task that scrolls the tab bar to the given absolute horizontal offset.
pub(crate) fn scroll_tab_bar_to(target_x: f32) -> Task<()> {
    operation::scroll_to(
        TAB_BAR_ID.clone(),
        AbsoluteOffset {
            x: Some(target_x),
            y: None,
        },
    )
}

/// Build the session tab bar displayed at the top of the center pane.
pub(crate) fn session_tabs(conversation: &ConversationState) -> Element<'_, CenterPaneEvent> {
    let viewing_number = conversation.viewing_tab_number();
    let tabs = conversation
        .session_tabs
        .iter()
        .map(|tab| tab_button(tab, tab.number == viewing_number));

    let bar = scrollable(row(tabs).spacing(4).align_y(Alignment::Center))
        .direction(Direction::Horizontal(
            Scrollbar::new().width(0).scroller_width(0),
        ))
        .width(Length::Fill)
        .height(Length::Shrink)
        .id(TAB_BAR_ID.clone())
        .on_scroll(CenterPaneEvent::TabBarScrolled);

    container(
        row![
            arrow_button(CHEVRON_LEFT, TabBarDirection::Left, conversation),
            bar,
            arrow_button(CHEVRON_RIGHT, TabBarDirection::Right, conversation),
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(BAR_HEIGHT))
    .padding(Padding::new(0.0).top(BAR_VPAD).left(4.0))
    .style(bordered_bar_style)
    .into()
}

/// Build a single session tab: status dot, label with tooltip, close button.
fn tab_button<'a>(tab: &'a SessionTab, active: bool) -> Element<'a, CenterPaneEvent> {
    let number = tab.number;
    let running = tab.running();

    // Status indicator: ○ while streaming (raised slightly), colored ● after a terminal event.
    let status = if running {
        Some(("○", theme::CRABOT_PRIMARY, 2.0))
    } else {
        tab.end_status.map(|s| ("●", end_status_color(s), 0.0))
    };

    let mut content = row![].spacing(4).align_y(Alignment::Center);
    if let Some((glyph, color, raise)) = status {
        content = content.push(
            container(text(glyph).size(9.0).color(color)).padding(Padding::new(0.0).bottom(raise)),
        );
    }

    let label = text(format!("Session {}", number)).size(12.0).font(Font {
        weight: if active {
            font::Weight::Bold
        } else {
            font::Weight::Normal
        },
        ..Font::DEFAULT
    });

    // Tooltip with session info — title, falling back to the session id.
    let tip = if tab.session.title.is_empty() {
        &tab.session.id
    } else {
        &tab.session.title
    };
    content = content.push(
        tooltip(
            label,
            text(tip).size(11).color(Color::WHITE),
            tooltip::Position::Bottom,
        )
        .gap(4)
        .style(tooltip_style),
    );

    // Close button — disabled while this tab is running.
    let close = button(tinted_icon(CLOSE, CLOSE_SIZE))
        .on_press_maybe(
            (!running).then(|| CenterPaneEvent::Conversation(ConversationEvent::CloseTab(number))),
        )
        .padding(0)
        .style(tab_close_button_style);
    content = content.push(close);

    button(container(content).center_y(Length::Fill))
        .on_press(CenterPaneEvent::Conversation(ConversationEvent::SwitchTab(
            number,
        )))
        .padding(Padding::from([4, 8]).top(6))
        .height(Length::Fixed(TAB_HEIGHT))
        .style(session_tab_style(active, running && !active))
        .into()
}

/// A small chevron arrow button for scrolling the tab bar. When the bar cannot
/// scroll further in `direction`, collapses to zero width so it doesn't affect
/// the row layout.
///
/// Uses a [`mouse_area`] so `on_press` fires on mouse-down, enabling press-and-hold
/// auto-repeat.  Hover feedback is driven by enter/exit events rather than a
/// [`button`] widget so that the area stays interactive even after the cursor is
/// dragged outside.
fn arrow_button<'a>(
    icon_data: &'static [u8],
    direction: TabBarDirection,
    conversation: &'a ConversationState,
) -> Element<'a, CenterPaneEvent> {
    let scroll = &conversation.tab_bar_scroll;
    let can_scroll = match direction {
        TabBarDirection::Left => scroll.can_scroll_left(),
        TabBarDirection::Right => scroll.can_scroll_right(),
    };
    // The TabBarScrollState is updated eagerly on arrow clicks and also from
    // `on_scroll` events, so it is always consistent.
    if !scroll.has_overflow() || !can_scroll {
        return Space::new()
            .width(0.0)
            .height(Length::Fixed(TAB_HEIGHT))
            .into();
    }

    let held = conversation.tab_bar_held_direction == Some(direction);
    let hovered = conversation.tab_bar_hovered_direction == Some(direction);
    let event = |e: ConversationEvent| CenterPaneEvent::Conversation(e);

    let area = mouse_area(
        container(tinted_icon(icon_data, 14.0))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .on_press(event(match direction {
        TabBarDirection::Left => ConversationEvent::TabBarScrollLeftHold,
        TabBarDirection::Right => ConversationEvent::TabBarScrollRightHold,
    }))
    .on_enter(event(ConversationEvent::TabBarArrowEnter(direction)))
    .on_exit(event(ConversationEvent::TabBarArrowExit))
    .interaction(mouse::Interaction::Pointer);

    container(area)
        .width(22.0)
        .height(Length::Fixed(TAB_HEIGHT))
        .align_y(Alignment::Center)
        .style(move |theme: &iced::Theme| container::Style {
            background: (held || hovered)
                .then(|| theme.extended_palette().secondary.weak.color.into()),
            ..container::Style::default()
        })
        .into()
}

/// An inline SVG icon tinted muted, brightening on hover.
fn tinted_icon<'a>(data: &'static [u8], size: f32) -> svg::Svg<'a> {
    svg(svg::Handle::from_memory(data))
        .width(size)
        .height(size)
        .style(|_theme, status| svg::Style {
            color: Some(match status {
                svg::Status::Hovered => theme::color_text_strong(),
                svg::Status::Idle => theme::color_muted(),
            }),
        })
}

/// Map a session-end status to a dot color for the tab indicator.
fn end_status_color(status: SessionEndStatus) -> Color {
    match status {
        SessionEndStatus::Done => theme::CRABOT_SUCCESS,
        SessionEndStatus::Error => theme::CRABOT_DANGER,
        SessionEndStatus::Cancelled => theme::CRABOT_YELLOW,
    }
}
