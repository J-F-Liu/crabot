use iced::{
    Alignment, Element, Font, Length, font,
    widget::{button, row, text},
};

use crate::Message;
use crate::llm::StreamState;

pub(crate) fn session_view<'a>(streaming: StreamState) -> Element<'a, Message> {
    row![
        text("Session").size(14).font(Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        }),
        iced::widget::Space::new().width(Length::Fill),
        button(text("New").align_x(Alignment::Center))
            .on_press_maybe(if streaming != StreamState::Idle {
                None
            } else {
                Some(Message::NewSession)
            })
            .style(crate::views::primary_button),
    ]
    .align_y(Alignment::Center)
    .spacing(8)
    .into()
}
