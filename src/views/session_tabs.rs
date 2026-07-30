use iced::widget::scrollable::{AbsoluteOffset, Direction};
use iced::widget::{Id, operation};
use iced::{
    Alignment, Color, Element, Font, Length, Padding, Task, font, mouse,
    widget::{button, container, mouse_area, row, scrollable, svg, text, tooltip},
};

use super::icons::{CHEVRON_LEFT, CHEVRON_RIGHT, CLOSE};
use super::styles::{bordered_bar_style, session_tab_style, tab_close_button_style, tooltip_style};
use super::theme;
use crate::app::{ConversationState, conversation::TabBarDirection};
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
pub(crate) fn session_tabs<'a>(
    conversation: &'a ConversationState,
) -> Element<'a, CenterPaneEvent> {
    let viewing_number = conversation.viewing_tab_number();

    let tabs: Vec<Element<'_, CenterPaneEvent>> = conversation
        .session_tabs
        .iter()
        .map(|tab| {
            let active = tab.number == viewing_number;
            let running = tab.running();
            let is_running_bg = running && !active;

            let mut row_content = row![].spacing(6).align_y(Alignment::Center);

            // Running dot (only for the running tab), horizontally centered in a fixed
            // cell so the label stays put when the dot appears or disappears.
            if running {
                row_content = row_content.push(
                    container(text("●").size(9.0).color(theme::CRABOT_PRIMARY))
                        .width(Length::Fixed(10.0))
                        .center_x(Length::Fixed(10.0)),
                );
            }

            // Tab label.
            let label = format!("Session {}", tab.number);
            let label_text = text(label).size(12.0).font(Font {
                weight: if active {
                    font::Weight::Bold
                } else {
                    font::Weight::Normal
                },
                ..Font::DEFAULT
            });

            // Tooltip with session info.
            let tip = if tab.session.title.is_empty() {
                tab.session.id.clone()
            } else {
                tab.session.title.clone()
            };
            let label_with_tip = tooltip(
                container(label_text).padding(0),
                text(tip).size(11).color(Color::WHITE),
                tooltip::Position::Bottom,
            )
            .gap(4)
            .style(tooltip_style);

            row_content = row_content.push(label_with_tip);

            // Close button — disabled while this tab is running.
            let close_icon = svg(svg::Handle::from_memory(CLOSE))
                .width(CLOSE_SIZE)
                .height(CLOSE_SIZE)
                .style(|_theme, status| svg::Style {
                    color: Some(match status {
                        svg::Status::Hovered => theme::color_text_strong(),
                        svg::Status::Idle => theme::color_muted(),
                    }),
                });
            let close_glyph = container(close_icon)
                .width(Length::Fixed(CLOSE_SIZE))
                .height(Length::Fixed(CLOSE_SIZE))
                .center_x(Length::Fixed(CLOSE_SIZE))
                .center_y(Length::Fixed(CLOSE_SIZE));
            let close_btn = button(close_glyph)
                .on_press_maybe(if running {
                    None
                } else {
                    Some(CenterPaneEvent::Conversation(ConversationEvent::CloseTab(
                        tab.number,
                    )))
                })
                .padding(0)
                .style(tab_close_button_style);

            row_content = row_content.push(close_btn);

            let tab_widget = button(container(row_content).center_y(Length::Fill))
                .on_press(CenterPaneEvent::Conversation(ConversationEvent::SwitchTab(
                    tab.number,
                )))
                .padding(Padding::from([4, 8]).top(6))
                .height(Length::Fixed(TAB_HEIGHT))
                .style(session_tab_style(active, is_running_bg));

            tab_widget.into()
        })
        .collect();

    let bar_content = row(tabs).spacing(4).align_y(Alignment::Center);

    // Arrow visibility: the TabBarScrollState is updated eagerly on arrow clicks
    // and also from `on_scroll` events, so it is always consistent.
    let scroll = conversation.tab_bar_scroll;
    let overflow = scroll.has_overflow();
    let show_left = overflow && scroll.can_scroll_left();
    let show_right = overflow && scroll.can_scroll_right();
    let held_left = conversation.tab_bar_held_direction == Some(TabBarDirection::Left);
    let held_right = conversation.tab_bar_held_direction == Some(TabBarDirection::Right);
    let hovered_left = conversation.tab_bar_hovered_direction == Some(TabBarDirection::Left);
    let hovered_right = conversation.tab_bar_hovered_direction == Some(TabBarDirection::Right);

    let left_arrow = arrow_button(
        CHEVRON_LEFT,
        show_left,
        held_left,
        hovered_left,
        TabBarDirection::Left,
    );
    let right_arrow = arrow_button(
        CHEVRON_RIGHT,
        show_right,
        held_right,
        hovered_right,
        TabBarDirection::Right,
    );

    container(
        row![
            left_arrow,
            scrollable(bar_content)
                .direction(Direction::Horizontal(
                    iced::widget::scrollable::Scrollbar::new()
                        .width(0)
                        .scroller_width(0),
                ))
                .width(Length::Fill)
                .height(Length::Shrink)
                .id(TAB_BAR_ID.clone())
                .on_scroll(CenterPaneEvent::TabBarScrolled),
            right_arrow,
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(BAR_HEIGHT))
    .padding(Padding {
        top: BAR_VPAD,
        right: 0.0,
        bottom: 0.0,
        left: 4.0,
    })
    .style(bordered_bar_style)
    .into()
}

/// A small chevron arrow button. When `visible` is false, collapses to zero width
/// so it doesn't affect the row layout.
///
/// Uses a [`mouse_area`] so `on_press` fires on mouse-down, enabling press-and-hold
/// auto-repeat.  Hover feedback is driven by enter/exit events rather than a
/// [`button`](iced::widget::button) widget so that the area stays interactive even
/// after the cursor is dragged outside.
fn arrow_button<'a>(
    icon_data: &'static [u8],
    visible: bool,
    held: bool,
    hovered: bool,
    direction: TabBarDirection,
) -> Element<'a, CenterPaneEvent> {
    if !visible {
        return container(row![])
            .width(Length::Fixed(0.0))
            .height(Length::Fixed(TAB_HEIGHT))
            .into();
    }

    let on_press = CenterPaneEvent::Conversation(match direction {
        TabBarDirection::Left => ConversationEvent::TabBarScrollLeftHold,
        TabBarDirection::Right => ConversationEvent::TabBarScrollRightHold,
    });
    let on_enter = CenterPaneEvent::Conversation(ConversationEvent::TabBarArrowEnter(direction));
    let on_exit = CenterPaneEvent::Conversation(ConversationEvent::TabBarArrowExit);

    let icon = svg(svg::Handle::from_memory(icon_data))
        .width(14.0)
        .height(14.0)
        .style(|_theme, status| svg::Style {
            color: Some(match status {
                svg::Status::Hovered => theme::color_text_strong(),
                svg::Status::Idle => theme::color_muted(),
            }),
        });

    let area = mouse_area(
        container(icon)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .on_press(on_press)
    .on_enter(on_enter)
    .on_exit(on_exit)
    .interaction(mouse::Interaction::Pointer);

    container(area)
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(TAB_HEIGHT))
        .align_y(Alignment::Center)
        .style(move |theme: &iced::Theme| container::Style {
            background: if held || hovered {
                let p = theme.extended_palette();
                Some(p.secondary.weak.color.into())
            } else {
                None
            },
            ..container::Style::default()
        })
        .into()
}
