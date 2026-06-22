use iced::{
    Alignment, Element, Length,
    widget::{button, column, pick_list, row, text, text_editor},
};
use std::fmt;

use crate::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkMode {
    Plan,
    Code,
    Review,
}

impl fmt::Display for WorkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkMode::Plan => write!(f, "plan"),
            WorkMode::Code => write!(f, "code"),
            WorkMode::Review => write!(f, "review"),
        }
    }
}

pub struct UserPrompt {
    pub mode: WorkMode,
    pub content: String,
}

impl UserPrompt {
    pub fn new(mode: WorkMode, content: String) -> Self {
        Self { mode, content }
    }

    pub fn get_prompt(&self) -> String {
        let mut prompt = String::new();
        prompt.push_str(&format!("<work-mode>{}</work-mode>\n", self.mode));
        prompt.push_str(&format!("{}\n", &self.content));
        prompt
    }
}

pub fn user_prompt_view<'a>(
    user_prompt: &'a text_editor::Content,
    workmode: WorkMode,
) -> Element<'a, Message> {
    column![
        text_editor(user_prompt)
            .height(120)
            .on_action(Message::EditUserPrompt),
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
                .style(crate::primary_button),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(4)
    .into()
}
