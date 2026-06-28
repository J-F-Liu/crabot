use iced::{
    Alignment, Element, Fill, Font, font,
    widget::{button, column, container, pick_list, row, text},
};

use crate::Message;
use crate::llm::StreamState;
use crate::session::SessionEntry;

pub(crate) fn session_view<'a>(
    streaming: StreamState,
    session_options: &'a [SessionEntry],
    current_session_id: &'a str,
) -> Element<'a, Message> {
    let selected = session_options.iter().find(|e| e.id == current_session_id);

    let list = pick_list(
        session_options,
        selected,
        if streaming == StreamState::Idle {
            Message::LoadSession
        } else {
            |_| Message::Noop
        },
    )
    .width(Fill);
    let list = if streaming != StreamState::Idle {
        list.style(crate::views::disabled_pick_list_style)
    } else {
        list
    };

    column![
        row![
            text("Session").size(14).font(Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }),
            iced::widget::Space::new().width(Fill),
            button(text("New").align_x(Alignment::Center))
                .on_press_maybe(if streaming != StreamState::Idle {
                    None
                } else {
                    Some(Message::NewSession)
                })
                .style(crate::views::primary_button),
        ]
        .align_y(Alignment::Center)
        .spacing(8),
        container(list).clip(true),
    ]
    .spacing(4)
    .into()
}
