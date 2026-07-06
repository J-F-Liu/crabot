use iced::{
    Border, Color, Element, Font, Length, Theme, font,
    widget::{button, column, container, mouse_area, row, rule, stack, text},
};

use crate::Message;

use super::theme::CRABOT_PRIMARY;

pub fn workspace_modal() -> Element<'static, Message> {
    let default_path = home::home_dir().unwrap_or_default().join(".crabot");

    let backdrop = mouse_area(
        container(text(""))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.0, 0.0, 0.0, 0.5,
                ))),
                ..container::Style::default()
            }),
    )
    .on_press(Message::Noop);

    let dialog = container(
        column![
            container(
                text("Empty Workspace")
                    .size(18)
                    .font(Font {
                        weight: font::Weight::Bold,
                        ..Font::DEFAULT
                    })
                    .color(CRABOT_PRIMARY),
            )
            .padding(iced::Padding::new(0.0).bottom(8.0)),
            rule::horizontal(1).style(|_: &Theme| rule::Style {
                color: CRABOT_PRIMARY,
                fill_mode: rule::FillMode::Full,
                radius: 0.0.into(),
                snap: false,
            }),
            text(format!(
                "Workspace path is empty.\n\nContinue with the default workspace?\n{}",
                default_path.display()
            ))
            .size(14),
            row![
                button(text("Yes")).on_press(Message::EmptyWorkspaceConfirm(Some(default_path))),
                button(text("No")).on_press(Message::EmptyWorkspaceConfirm(None)),
            ]
            .spacing(10)
            .padding(10),
        ]
        .spacing(10)
        .padding(20)
        .align_x(iced::Alignment::Center),
    )
    .style(|_: &Theme| container::Style {
        background: Some(Color::WHITE.into()),
        border: Border::default().rounded(8),
        ..container::Style::default()
    })
    .max_width(400);

    stack![
        backdrop,
        container(dialog)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    ]
    .into()
}
