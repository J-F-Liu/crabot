//! User Interface page: language, appearance, font scale, and renderer backend.

use iced::{
    Alignment, Element, Length,
    widget::{column, container, row, text},
};

use super::{SettingsEvent, SettingsState, form_card_style, section_header, section_title};
use crate::views::styles::secondary_dropdown_style;
use crate::views::theme::color_muted;
use crate::widgets::dropdown::DropDown;
use crabot::i18n::Lang;
use crabot::settings::{Appearance, FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP};

/// Renders the User Interface page.
pub(super) fn user_interface_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let lang = state.language;
    column![
        section_header(lang.tr("User Interface")),
        language_card(state),
        appearance_card(state),
        font_scale_card(state),
        renderer_backend_card(state),
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

/// Muted hint line shown beneath a card's controls.
fn hint(label: &str) -> Element<'_, SettingsEvent> {
    text(label).size(11).color(color_muted()).into()
}

/// Dropdown entry: the label shown to the user plus the value it selects.
#[derive(Clone, Copy, PartialEq)]
struct Choice<'a, T> {
    label: &'a str,
    value: T,
}

impl<T> std::fmt::Display for Choice<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

/// The dropdown shared by every page-level choice.
fn picker<'a, T>(
    options: Vec<T>,
    selected: Option<T>,
    on_select: impl Fn(T) -> SettingsEvent + 'a,
) -> Element<'a, SettingsEvent>
where
    T: ToString + PartialEq + Clone + 'a,
{
    DropDown::new(options, selected, on_select)
        .width(Length::Fixed(220.0))
        .text_size(13)
        .style(secondary_dropdown_style)
        .into()
}

/// Language picker card.
fn language_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    let lang = state.language;
    card(
        lang.tr("Language"),
        picker(Lang::ALL.to_vec(), Some(lang), SettingsEvent::SetLanguage),
    )
}

/// Color-appearance picker card; "Follow system" tracks the OS.
fn appearance_card(state: &SettingsState) -> Element<'_, SettingsEvent> {
    let lang = state.language;
    let options: Vec<Choice<'static, Appearance>> = Appearance::ALL
        .into_iter()
        .map(|value| Choice {
            label: lang.tr(value.label()),
            value,
        })
        .collect();
    let selected = options
        .iter()
        .copied()
        .find(|o| o.value == state.appearance);
    card(
        lang.tr("Appearance"),
        column![
            picker(options, selected, |o| SettingsEvent::SetAppearance(o.value)),
            hint(lang.tr("Follow system tracks the OS light/dark preference.")),
        ]
        .spacing(6),
    )
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
    card(
        lang.tr("Font scale"),
        column![
            row![slider, percent].spacing(8).align_y(Alignment::Center),
            hint(lang.tr("Chat text size; Ctrl + and Ctrl - also zoom.")),
        ]
        .spacing(6),
    )
}

/// Renderer-backend presets as `(label, ICED_BACKEND value)`.
const BACKEND_PRESETS: [(&str, &str); 3] = [
    ("Auto", "auto"),
    ("Tiny Skia", "tiny_skia"),
    ("Wgpu", "wgpu"),
];

/// Renderer-backend picker card; takes effect after a restart.
fn renderer_backend_card<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let lang = state.language;
    let stored = state.iced_backend.trim();
    let options: Vec<Choice<'a, &'a str>> = BACKEND_PRESETS
        .iter()
        .map(|&(label, value)| Choice { label, value })
        .collect();
    // Preset match is case-insensitive; other stored values show verbatim.
    let selected = options
        .iter()
        .copied()
        .find(|o| o.value.eq_ignore_ascii_case(stored) || (o.value == "auto" && stored.is_empty()))
        .or_else(|| {
            (!stored.is_empty()).then_some(Choice {
                label: stored,
                value: stored,
            })
        });
    card(
        lang.tr("Renderer backend"),
        column![
            picker(options, selected, |o| SettingsEvent::SetIcedBackend(
                o.value.to_string()
            )),
            hint(lang.tr("Takes effect after a restart.")),
        ]
        .spacing(6),
    )
}
