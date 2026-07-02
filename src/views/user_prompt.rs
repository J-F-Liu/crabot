use iced::{
    Alignment, Element, Length, padding,
    widget::{button, column, pick_list, row, text},
};

use crate::FocusedTarget;
use crate::Message;
use crate::user::WorkMode;
use crate::widgets::textarea::TextArea;

pub(crate) fn user_prompt_view<'a>(
    user_prompt: &'a TextArea,
    workmode: WorkMode,
) -> Element<'a, Message> {
    column![
        user_prompt
            .view(|msg| Message::EditTextArea(FocusedTarget::UserPrompt, msg))
            .height(120),
        row![
            pick_list(
                &[WorkMode::Plan, WorkMode::Code, WorkMode::Review][..],
                Some(workmode),
                Message::SelectWorkMode,
            )
            .width(120),
            iced::widget::Space::new().width(Length::Fill),
            button(text("Send").align_x(Alignment::Center))
                .width(80)
                .on_press(Message::SendPrompt)
                .style(crate::views::primary_button),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(4)
    .padding(padding::bottom(4))
    .into()
}
