//! Inline monochrome SVG icons (Lucide), tinted via `svg::Style::color`.

use std::time::Duration;

use iced::{
    Color, Element,
    widget::{button, svg, text, tooltip},
};

use crate::Message;

use super::styles::{icon_button_style, tooltip_style};
use super::theme::{CRABOT_TEXT, CRABOT_TEXT_MUTED};

/// Lucide "copy" icon.
pub(crate) const COPY: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>"##;

/// Lucide "refresh-cw" icon.
pub(crate) const RESEND: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/></svg>"##;

/// Lucide "settings" gear icon.
pub(crate) const SETTINGS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>"##;

/// Lucide "x" close icon.
pub(crate) const CLOSE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"##;

/// A small SVG icon button with a tooltip shown below on hover.
#[must_use]
pub(crate) fn icon_action(
    icon: &'static [u8],
    tip: &'static str,
    on_press: Message,
) -> Element<'static, Message> {
    let icon = svg(svg::Handle::from_memory(icon))
        .width(14)
        .height(14)
        .style(|_theme, status| svg::Style {
            color: Some(match status {
                svg::Status::Hovered => CRABOT_TEXT,
                svg::Status::Idle => CRABOT_TEXT_MUTED,
            }),
        });

    tooltip(
        button(icon)
            .on_press(on_press)
            .padding(6)
            .style(icon_button_style),
        text(tip).size(11).color(Color::WHITE),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .delay(Duration::from_millis(400))
    .style(tooltip_style)
    .into()
}
