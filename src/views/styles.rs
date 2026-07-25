use iced::{
    Border, Color, Element, Font, Length, Shadow, Theme, Vector, font,
    widget::{button, checkbox, container, mouse_area, rule, toggler},
};
use iced_selection::text::Style as SelectionStyle;

use super::theme::*;

/// Card background with a thin border, used for status bars and the search bar.
pub(crate) fn bordered_bar_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_card().into()),
        border: Border {
            color: color_border(),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

// ── pane styles ───────────────────────────────────────────────────

pub(crate) fn pane_side(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_panel().into()),
        ..container::Style::default()
    }
}

pub(crate) fn pane_center(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_card().into()),
        ..container::Style::default()
    }
}

// ── label helper ──────────────────────────────────────────────────

pub(crate) fn label<'a, M: Clone + 'static>(
    text: &'a str,
    width: impl Into<Length>,
) -> Element<'a, M> {
    container(iced::widget::text(text).size(14).font(Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .width(width)
    .into()
}

// ── divider ───────────────────────────────────────────────────────

/// Per-divider hover + drag state.
#[derive(Default, Debug)]
pub(crate) struct DividerState {
    pub(crate) hovered: bool,
    pub(crate) dragging: bool,
    pub(crate) origin: f32,
    pub(crate) start: f32,
}

pub(crate) fn divider<M: Clone + 'static>(state: &DividerState) -> Element<'static, M> {
    let color = if state.dragging {
        CRABOT_PRIMARY
    } else if state.hovered {
        color_muted()
    } else {
        color_border()
    };
    mouse_area(rule::vertical(HANDLE).style(move |_theme| rule::Style {
        color,
        fill_mode: rule::FillMode::Full,
        radius: 0.0.into(),
        snap: false,
    }))
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

/// Neutral / secondary button style — surface background with a border.
pub(crate) fn secondary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let (hover_bg, pressed_bg) = if is_dark() {
        (
            Color::from_rgb8(0x33, 0x39, 0x44),
            Color::from_rgb8(0x3B, 0x42, 0x4E),
        )
    } else {
        (
            Color::from_rgb8(0xD8, 0xD8, 0xD8),
            Color::from_rgb8(0xC8, 0xC8, 0xC8),
        )
    };
    let base = button::Style {
        background: Some(color_surface().into()),
        text_color: color_text_strong(),
        border: iced::Border::default()
            .rounded(6)
            .width(1)
            .color(color_border()),
        ..button::Style::default()
    };
    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style {
            background: Some(hover_bg.into()),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(pressed_bg.into()),
            ..base
        },
        button::Status::Disabled => button::Style {
            background: Some(color_surface().scale_alpha(0.5).into()),
            ..base
        },
    }
}

pub(crate) fn primary_toggler(_theme: &Theme, status: toggler::Status) -> toggler::Style {
    let base = toggler::Style {
        background: color_surface().into(),
        background_border_width: 1.0,
        background_border_color: if is_dark() {
            Color::from_rgb8(0x45, 0x4C, 0x59)
        } else {
            Color::from_rgb8(0xC0, 0xC0, 0xC0)
        },
        foreground: Color::WHITE.into(),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(color_text_strong()),
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
                } else if is_dark() {
                    Color::from_rgb8(0x33, 0x39, 0x44).into()
                } else {
                    Color::from_rgb8(0xD8, 0xD8, 0xD8).into()
                };
                style.background_border_color = if is_toggled {
                    CRABOT_PRIMARY_HOVER
                } else if is_dark() {
                    Color::from_rgb8(0x50, 0x58, 0x66)
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
        background: color_card().into(),
        icon_color: Color::WHITE,
        border: iced::Border::default()
            .rounded(4)
            .width(1)
            .color(if is_dark() {
                Color::from_rgb8(0x4A, 0x51, 0x5E)
            } else {
                Color::from_rgb8(0xB0, 0xB0, 0xB0)
            }),
        text_color: Some(color_text_strong()),
    };
    match status {
        checkbox::Status::Disabled { is_checked } => {
            let mut style = base;
            if is_checked {
                style.background = Color::from_rgb8(0xA0, 0xA0, 0xA0).into();
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(Color::from_rgb8(0xA0, 0xA0, 0xA0));
                style.icon_color = Color::from_rgb8(0xE0, 0xE0, 0xE0);
            } else {
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(Color::from_rgb8(0xD0, 0xD0, 0xD0));
            }
            style.text_color = Some(color_muted());
            style
        }
        checkbox::Status::Active { is_checked } | checkbox::Status::Hovered { is_checked } => {
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
    style.text_color = color_muted();
    style
}

// ── message bubble styles ─────────────────────────────────────────

pub(crate) fn user_bubble_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_user_bg().into()),
        border: Border {
            color: color_user_bg(),
            width: 0.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn assistant_bubble_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_assistant_bg().into()),
        border: Border {
            color: color_assistant_bg(),
            width: 0.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

pub(crate) fn tool_bubble_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_tool_bg().into()),
        border: Border {
            color: color_tool_bg(),
            width: 0.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// Subtle inset container for reasoning text inside assistant bubbles.
pub(crate) fn reasoning_box_style(_theme: &Theme) -> container::Style {
    let (bg, border) = if is_dark() {
        (
            Color::from_rgba(1.0, 1.0, 1.0, 0.05),
            Color::from_rgba(1.0, 1.0, 1.0, 0.09),
        )
    } else {
        (
            Color::from_rgba(0.0, 0.0, 0.0, 0.035),
            Color::from_rgba(0.0, 0.0, 0.0, 0.06),
        )
    };
    container::Style {
        background: Some(bg.into()),
        border: Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
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
        _ => (color_surface(), Color::TRANSPARENT),
    };
    move |_theme: &Theme| container::Style {
        background: Some(bg.into()),
        text_color: Some(fg),
        ..container::Style::default()
    }
}

// ── dropdown styles ───────────────────────────────────────────────

/// Muted, non-interactive-looking style for DropDown when disabled.
pub(crate) fn disabled_dropdown_style(_theme: &Theme) -> crate::widgets::dropdown::Style {
    crate::widgets::dropdown::Style {
        text_color: color_muted(),
        placeholder_color: color_muted(),
        handle_color: color_muted(),
        background: iced::Background::Color(color_surface()),
        border: iced::Border::default(),
    }
}

/// Floating tooltip box — dark rounded background with a subtle shadow.
pub(crate) fn tooltip_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.2, 0.2, 0.2, 0.95).into()),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.25),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
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
