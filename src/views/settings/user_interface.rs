//! User Interface page: language picker and chat font scale.

use iced::{
    Alignment, Element, Length,
    widget::{column, container, row, text},
};

use super::{SettingsEvent, SettingsState, form_card_style, section_header, section_title};
use crate::views::styles::secondary_dropdown_style;
use crate::views::theme::color_muted;
use crate::widgets::dropdown::DropDown;
use crabot::i18n::Lang;
use crabot::settings::{FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP};

/// Renders the User Interface page with language and font scale settings.
pub(super) fn user_interface_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let lang = state.language;
    column![
        section_header(lang.tr("User Interface")),
        language_card(state),
        font_scale_card(state),
    ]
    .spacing(8)
    .into()
}

/// Titled settings card: muted bold title over the given body.
fn card<'a>(
    title: &'static str,
    body: impl Into<Element<'a, SettingsEvent>>,
) -> Element<'a, SettingsEvent> {
    container(column![section_title(title), body.into()].spacing(6))
        .padding([6, 12])
        .style(form_card_style)
        .width(Length::Fill)
        .into()
}

/// Language picker card.
fn language_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    let lang = state.language;
    let picker = DropDown::new(Lang::ALL, Some(lang), SettingsEvent::SetLanguage)
        .width(Length::Fixed(220.0))
        .text_size(13)
        .style(secondary_dropdown_style);
    card(lang.tr("Language"), picker)
}

/// Chat font-scale slider card.
fn font_scale_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    let lang = state.language;
    let scale = state.font_scale;
    let slider = iced::widget::slider(
        FONT_SCALE_MIN..=FONT_SCALE_MAX,
        scale,
        SettingsEvent::SetFontScale,
    )
    .step(FONT_SCALE_STEP)
    .width(Length::Fixed(220.0));
    let percent = text(format!("{}%", (scale * 100.0).round() as i32))
        .size(13)
        .width(Length::Fixed(44.0));
    let hint = text(lang.tr("Chat text size; Ctrl + and Ctrl - also zoom."))
        .size(11)
        .color(color_muted());
    card(
        lang.tr("Font scale"),
        column![
            row![slider, percent].spacing(8).align_y(Alignment::Center),
            hint
        ]
        .spacing(6),
    )
}
