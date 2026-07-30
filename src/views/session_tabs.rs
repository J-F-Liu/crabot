use iced::Task;
use iced::advanced::widget::operation::scrollable as scrollable_op;
use iced::widget::Id;
use iced::widget::scrollable::{AbsoluteOffset, Direction, Viewport};
use iced::{
    Alignment, Color, Element, Font, Length, Padding, font,
    widget::{button, container, row, scrollable, svg, text, tooltip},
};
use iced_runtime::task::widget as task_widget;

use super::icons::{CHEVRON_LEFT, CHEVRON_RIGHT, CLOSE};
use super::styles::{bordered_bar_style, session_tab_style, tab_close_button_style, tooltip_style};
use super::theme;
use crate::app::ConversationState;
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
pub(crate) const TAB_BAR_SCROLL: Id = Id::new("tab-bar");
/// Horizontal scroll step in pixels per arrow-click.
pub(crate) const TAB_SCROLL_STEP: f32 = 120.0;

/// Scroll the tab bar to `target_x` (absolute horizontal offset).
pub(crate) fn scroll_tab_bar_to(target_x: f32) -> Task<()> {
    task_widget(scrollable_op::scroll_to(
        TAB_BAR_SCROLL.clone(),
        AbsoluteOffset {
            x: Some(target_x),
            y: None,
        },
    ))
}

/// Build the session tab bar displayed at the top of the center pane.
pub(crate) fn session_tabs<'a>(
    conversation: &'a ConversationState,
    tab_bar_viewport: Option<Viewport>,
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

    // Determine arrow visibility.  Prefer the viewport when available;
    // otherwise fall back to a rough heuristic so the arrows are immediately
    // usable even before the first `on_scroll` event fires.
    let (show_left, show_right) = tab_bar_viewport.map_or_else(
        || {
            // Without a viewport we don't know the exact overflow, but if there
            // are many tabs the bar most likely overflows.  Show both arrows so
            // the user can scroll; the scrollable will naturally clamp scrolls.
            let many = conversation.session_tabs.len() > 4;
            (many, many)
        },
        |vp| {
            let overflow = vp.content_bounds().width > vp.bounds().width;
            if !overflow {
                return (false, false);
            }
            let offset_x = vp.absolute_offset().x;
            let max_x = (vp.content_bounds().width - vp.bounds().width).max(0.0);
            (offset_x > 1.0, offset_x < max_x - 1.0)
        },
    );

    let left_arrow = arrow_button(
        CHEVRON_LEFT,
        show_left,
        CenterPaneEvent::Conversation(ConversationEvent::TabBarScrollLeft),
    );
    let right_arrow = arrow_button(
        CHEVRON_RIGHT,
        show_right,
        CenterPaneEvent::Conversation(ConversationEvent::TabBarScrollRight),
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
                .id(TAB_BAR_SCROLL.clone())
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

/// A small triangular arrow button, invisible when `visible` is false.
fn arrow_button<'a>(
    icon_data: &'static [u8],
    visible: bool,
    on_press: CenterPaneEvent,
) -> Element<'a, CenterPaneEvent> {
    if !visible {
        return container(row![])
            .width(Length::Fixed(0.0))
            .height(Length::Fixed(TAB_HEIGHT))
            .into();
    }
    let icon = svg(svg::Handle::from_memory(icon_data))
        .width(14.0)
        .height(14.0)
        .style(|_theme, status| svg::Style {
            color: Some(match status {
                svg::Status::Hovered => theme::color_text_strong(),
                svg::Status::Idle => theme::color_muted(),
            }),
        });
    button(
        container(icon)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .on_press(on_press)
    .padding(0)
    .width(Length::Fixed(22.0))
    .height(Length::Fixed(TAB_HEIGHT))
    .style(super::styles::icon_button_style)
    .into()
}
