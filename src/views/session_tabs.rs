use iced::widget::scrollable::Direction;
use iced::{
    Alignment, Color, Element, Font, Length, Padding, font,
    widget::{button, container, row, scrollable, svg, text, tooltip},
};

use super::icons::CLOSE;
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

    container(
        scrollable(bar_content)
            .direction(Direction::Horizontal(
                iced::widget::scrollable::Scrollbar::new()
                    .width(0)
                    .scroller_width(0),
            ))
            .height(Length::Shrink),
    )
    .width(Length::Fill)
    .height(Length::Fixed(BAR_HEIGHT))
    .padding(Padding {
        top: BAR_VPAD,
        right: BAR_VPAD,
        bottom: 0.0,
        left: 8.0,
    })
    .style(bordered_bar_style)
    .into()
}
