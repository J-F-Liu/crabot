use iced::{
    Border, Color, Element, Font, Length, Theme, font,
    widget::{button, checkbox, container, mouse_area, rule, toggler},
};
use iced_selection::text::Style as SelectionStyle;

use super::theme::*;
use crate::Message;

// ── pane styles ───────────────────────────────────────────────────

pub(crate) fn pane_side(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(CRABOT_PANEL.into()),
        ..container::Style::default()
    }
}

pub(crate) fn pane_center(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        ..container::Style::default()
    }
}

// ── label helper ──────────────────────────────────────────────────

pub(crate) fn label<'a>(text: &'a str, width: impl Into<Length>) -> Element<'a, Message> {
    container(iced::widget::text(text).size(14).font(Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .width(width)
    .into()
}

// ── divider ───────────────────────────────────────────────────────

pub(crate) fn divider() -> Element<'static, Message> {
    mouse_area(rule::vertical(HANDLE))
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
}

// ── button styles ───────────────────────────────────────────────

pub(crate) fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: Some(CRABOT_PRIMARY.into()),
        text_color: Color::WHITE,
        border: iced::Border::default().rounded(6),
        ..button::Style::default()
    };
    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style {
            background: Some(CRABOT_PRIMARY_HOVER.into()),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(CRABOT_PRIMARY_PRESSED.into()),
            ..base
        },
        button::Status::Disabled => button::Style {
            background: Some(CRABOT_PRIMARY.scale_alpha(0.5).into()),
            ..base
        },
    }
}

pub(crate) fn primary_toggler(_theme: &Theme, status: toggler::Status) -> toggler::Style {
    let base = toggler::Style {
        background: CRABOT_SURFACE.into(),
        background_border_width: 1.0,
        background_border_color: Color::from_rgb8(0xC0, 0xC0, 0xC0),
        foreground: Color::WHITE.into(),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(CRABOT_TEXT),
        border_radius: None,
        padding_ratio: 0.3,
    };
    match status {
        toggler::Status::Active { is_toggled }
        | toggler::Status::Hovered { is_toggled }
        | toggler::Status::Disabled { is_toggled } => {
            let mut style = base;
            if is_toggled {
                style.background = CRABOT_PRIMARY.into();
                style.background_border_color = CRABOT_PRIMARY;
            }
            if matches!(status, toggler::Status::Hovered { .. }) {
                style.background = if is_toggled {
                    CRABOT_PRIMARY_HOVER.into()
                } else {
                    Color::from_rgb8(0xD8, 0xD8, 0xD8).into()
                };
                style.background_border_color = if is_toggled {
                    CRABOT_PRIMARY_HOVER
                } else {
                    Color::from_rgb8(0xA8, 0xA8, 0xA8)
                };
            }
            style
        }
    }
}

pub(crate) fn primary_checkbox(_theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let base = checkbox::Style {
        background: Color::WHITE.into(),
        icon_color: Color::WHITE,
        border: iced::Border::default()
            .rounded(4)
            .width(1)
            .color(Color::from_rgb8(0xB0, 0xB0, 0xB0)),
        text_color: Some(CRABOT_TEXT),
    };
    match status {
        checkbox::Status::Active { is_checked }
        | checkbox::Status::Hovered { is_checked }
        | checkbox::Status::Disabled { is_checked } => {
            let mut style = base;
            if is_checked {
                style.background = CRABOT_PRIMARY.into();
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(CRABOT_PRIMARY);
                style.icon_color = Color::WHITE;
            }
            if matches!(status, checkbox::Status::Hovered { .. }) && is_checked {
                style.background = CRABOT_PRIMARY_HOVER.into();
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(CRABOT_PRIMARY_HOVER);
            }
            style
        }
    }
}

/// Subtle icon-button style — transparent background, dim text.
pub(crate) fn icon_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    let mut style = button::Style::default();
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            style.background = Some(p.secondary.weak.color.into());
        }
        _ => {}
    }
    style.text_color = CRABOT_TEXT_MUTED;
    style
}

// ── message bubble styles ─────────────────────────────────────────

pub(crate) fn user_bubble_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(CRABOT_USER_BG.into()),
        border: Border {
            color: CRABOT_USER_BG,
            width: 0.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn assistant_bubble_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(CRABOT_ASSISTANT_BG.into()),
        border: Border {
            color: CRABOT_ASSISTANT_BG,
            width: 0.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn tool_bubble_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(CRABOT_TOOL_BG.into()),
        border: Border {
            color: CRABOT_TOOL_BG,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// Small role badge (User / Assistant / Tool).
pub(crate) fn role_badge_style(role: &str) -> impl Fn(&Theme) -> container::Style + '_ {
    let (fg, bg) = match role {
        "User" => (Color::from_rgb8(0x4A, 0x90, 0xD9), Color::TRANSPARENT),
        "Assistant" => (Color::from_rgb8(0x1A, 0x9A, 0x8C), Color::TRANSPARENT),
        "Tool" => (Color::from_rgb8(0xD0, 0x8F, 0x33), Color::TRANSPARENT),
        _ => (CRABOT_SURFACE, Color::TRANSPARENT),
    };
    move |_theme: &Theme| container::Style {
        background: Some(bg.into()),
        text_color: Some(fg),
        ..container::Style::default()
    }
}

// ── pick_list styles ──────────────────────────────────────────────

/// Muted, non-interactive-looking style for pick_list when disabled.
pub(crate) fn disabled_pick_list_style(
    _theme: &Theme,
    _status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    iced::widget::pick_list::Style {
        text_color: CRABOT_TEXT_MUTED,
        placeholder_color: CRABOT_TEXT_MUTED,
        handle_color: CRABOT_TEXT_MUTED,
        background: iced::Background::Color(CRABOT_SURFACE),
        border: iced::Border::default(),
    }
}

// ── selectable text styles ────────────────────────────────────────

pub(crate) fn sel_default(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(color_text(theme)),
        selection: color_primary(theme),
    }
}

pub(crate) fn sel_primary(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(color_primary(theme)),
        selection: color_primary(theme),
    }
}

pub(crate) fn sel_secondary(theme: &Theme) -> SelectionStyle {
    SelectionStyle {
        color: Some(color_secondary(theme)),
        selection: color_secondary(theme),
    }
}
