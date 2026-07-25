use std::path::Path;

use iced::{
    Border, Element, Font, Length, Theme, font,
    widget::{Space, button, column, container, mouse_area, row, rule, stack, text},
};

use crate::OverlayEvent;

use super::styles::{primary_button, secondary_button};
use super::theme::{CRABOT_DIALOG_RADIUS, CRABOT_MODAL_SCRIM, CRABOT_PRIMARY, color_dialog_bg};

pub fn workspace_modal(default_path: &Path) -> Element<'_, OverlayEvent> {
    let backdrop = mouse_area(
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(CRABOT_MODAL_SCRIM.into()),
                ..container::Style::default()
            }),
    )
    .on_press(OverlayEvent::EmptyWorkspaceConfirm(None));

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
                button(text("Yes")).style(primary_button).on_press(
                    OverlayEvent::EmptyWorkspaceConfirm(Some(default_path.to_path_buf()))
                ),
                button(text("No"))
                    .style(secondary_button)
                    .on_press(OverlayEvent::EmptyWorkspaceConfirm(None)),
            ]
            .spacing(20)
            .padding(10),
        ]
        .spacing(10)
        .padding(20)
        .align_x(iced::Alignment::Center),
    )
    .style(|_: &Theme| container::Style {
        background: Some(color_dialog_bg().into()),
        border: Border::default().rounded(CRABOT_DIALOG_RADIUS),
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
