use iced::{
    Alignment, Element, Length, padding,
    widget::{button, checkbox, column, row, text},
};
use iced_aw::{
    style::{status::Status, tab_bar::Style as TabBarStyle},
    widget::tab_bar::{TabBar, TabLabel},
};

use crate::FocusedTarget;
use crate::Message;
use crate::widgets::textarea::TextArea;
use crabot::user::WorkMode;

pub(crate) fn user_prompt_view<'a>(
    user_prompt: &'a TextArea,
    workmode: WorkMode,
    workmode_enabled: bool,
) -> Element<'a, Message> {
    let mut tab_bar_builder = TabBar::new(Message::SelectWorkMode);
    for mode in WorkMode::all() {
        tab_bar_builder = tab_bar_builder.push(*mode, TabLabel::Text(mode.name.to_string()));
    }
    let tab_bar: Element<'_, Message> = tab_bar_builder
        .set_active_tab(&workmode)
        .tab_width(Length::Shrink)
        .width(Length::Shrink)
        .text_size(13.0)
        .padding([0, 8])
        .style(|theme: &iced::Theme, status| TabBarStyle {
            tab_label_background: match status {
                Status::Active => iced::Background::Color(theme.palette().primary),
                Status::Hovered => {
                    iced::Background::Color(theme.extended_palette().primary.weak.color)
                }
                _ => iced::Background::Color(theme.extended_palette().background.weak.color),
            },
            text_color: match status {
                Status::Active => iced::Color::WHITE,
                _ => theme.palette().text,
            },
            ..Default::default()
        })
        .into();
    column![
        row![
            checkbox(workmode_enabled)
                .label("Work mode")
                .on_toggle(Message::ToggleWorkMode)
                .style(crate::views::primary_checkbox),
            tab_bar,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        user_prompt
            .view(|msg| Message::EditTextArea(FocusedTarget::UserPrompt, msg))
            .height(120),
        row![
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
