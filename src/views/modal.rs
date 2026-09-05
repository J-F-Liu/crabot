use std::path::Path;

use iced::{
    Border, Element, Font, Length, Theme, font,
    widget::{Space, button, column, container, mouse_area, row, rule, stack, text},
};

use crate::OverlayEvent;
use crabot::i18n::Lang;

use super::styles::{danger_button, primary_button, secondary_button};
use super::theme::{CRABOT_DIALOG_RADIUS, CRABOT_MODAL_SCRIM, CRABOT_PRIMARY, color_dialog_bg};

const TITLE: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

/// Shared modal scaffold: scrim backdrop (click to dismiss) + centered card.
fn modal<'a>(
    dismiss: OverlayEvent,
    title: &'a str,
    body: impl Into<Element<'a, OverlayEvent>>,
    buttons: impl Into<Element<'a, OverlayEvent>>,
) -> Element<'a, OverlayEvent> {
    let backdrop = mouse_area(
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(CRABOT_MODAL_SCRIM.into()),
                ..container::Style::default()
            }),
    )
    .on_press(dismiss);

    let card = container(
        column![
            container(text(title).size(18).font(TITLE).color(CRABOT_PRIMARY))
                .padding(iced::Padding::new(0.0).bottom(8.0)),
            rule::horizontal(1).style(|_: &Theme| rule::Style {
                color: CRABOT_PRIMARY,
                fill_mode: rule::FillMode::Full,
                radius: 0.0.into(),
                snap: false,
            }),
            body.into(),
            buttons.into(),
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
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    ]
    .into()
}

pub fn workspace_modal(default_path: &Path, lang: Lang) -> Element<'_, OverlayEvent> {
    modal(
        OverlayEvent::EmptyWorkspaceConfirm(None),
        lang.tr("Empty Workspace"),
        text(
            lang.tr("Workspace path is empty.\n\nContinue with the default workspace?\n{}")
                .replace("{}", &default_path.display().to_string()),
        )
        .size(14),
        row![
            button(text(lang.tr("Yes"))).style(primary_button).on_press(
                OverlayEvent::EmptyWorkspaceConfirm(Some(default_path.to_path_buf()))
            ),
            button(text(lang.tr("No")))
                .style(secondary_button)
                .on_press(OverlayEvent::EmptyWorkspaceConfirm(None)),
        ]
        .spacing(20)
        .padding(10),
    )
}

/// Confirmation dialog for Revert All — destructive, requires explicit confirmation.
pub fn revert_all_modal(lang: Lang) -> Element<'static, OverlayEvent> {
    modal(
        OverlayEvent::RevertAllConfirm(false),
        lang.tr("Revert All Files"),
        text(lang.tr("Revert all files modified by this session?\n\n\
             Modified files are restored to their original content and files \
             created by the session are deleted. This cannot be undone."))
        .size(14),
        row![
            button(text(lang.tr("Revert All")))
                .style(danger_button)
                .on_press(OverlayEvent::RevertAllConfirm(true)),
            button(text(lang.tr("Cancel")))
                .style(secondary_button)
                .on_press(OverlayEvent::RevertAllConfirm(false)),
        ]
        .spacing(20)
        .padding(10),
    )
}
