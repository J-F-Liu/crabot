use std::path::Path;

use iced::{
    Border, Element, Font, Length, Theme, font,
    widget::{Space, button, column, container, mouse_area, row, rule, stack, text},
};

use crate::OverlayEvent;
use crate::app::StopSubtaskConfirm;
use crate::app::session_state::USER_CANCELLED_TASK;
use crabot::i18n::Lang;

use super::session_tabs::localize_tab_label;
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
        yes_no_buttons(
            OverlayEvent::EmptyWorkspaceConfirm(Some(default_path.to_path_buf())),
            OverlayEvent::EmptyWorkspaceConfirm(None),
            lang,
        ),
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

/// Ask whether a stopped sub-agent reports its cancellation to the parent.
pub fn stop_subtask_modal(
    confirm: &StopSubtaskConfirm,
    lang: Lang,
) -> Element<'static, OverlayEvent> {
    let parent = localize_tab_label(&confirm.parent_label, lang);
    let question = lang
        .tr("Return \"{}\" to {}?")
        .replacen("{}", USER_CANCELLED_TASK, 1)
        .replacen("{}", &parent, 1);
    let explanation = lang.tr(
        "No: keep this session open and send a new prompt or resend; the result is returned to the parent when the session finishes. Closing this tab or opening another session in it reports cancellation to the parent immediately.",
    );
    modal(
        OverlayEvent::StopSubtaskConfirm(false),
        lang.tr("Stop Subtask"),
        text(format!("{question}\n\n{explanation}")).size(14),
        yes_no_buttons(
            OverlayEvent::StopSubtaskConfirm(true),
            OverlayEvent::StopSubtaskConfirm(false),
            lang,
        ),
    )
}

/// Primary "Yes" / secondary "No" buttons for confirmation modals.
fn yes_no_buttons(
    yes: OverlayEvent,
    no: OverlayEvent,
    lang: Lang,
) -> Element<'static, OverlayEvent> {
    row![
        button(text(lang.tr("Yes")))
            .style(primary_button)
            .on_press(yes),
        button(text(lang.tr("No")))
            .style(secondary_button)
            .on_press(no),
    ]
    .spacing(20)
    .padding(10)
    .into()
}
