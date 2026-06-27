use iced::{Color, Theme};

// ── layout constants ──────────────────────────────────────────────

pub(crate) const MIN_W: f32 = 280.0;
pub(crate) const HANDLE: f32 = 6.0;

// ── theme colors ─────────────────────────────────────────────

pub(crate) const CRABOT_BG: Color = Color::from_rgb8(0xF0, 0xF0, 0xF0);
pub(crate) const CRABOT_PANEL: Color = Color::from_rgb8(0xF2, 0xF2, 0xF2);
pub(crate) const CRABOT_SURFACE: Color = Color::from_rgb8(0xE8, 0xE8, 0xE8);
pub(crate) const CRABOT_PRIMARY: Color = Color::from_rgb8(0x1A, 0x9A, 0x8C);
pub(crate) const CRABOT_PRIMARY_HOVER: Color = Color::from_rgb8(0x15, 0x8C, 0x7F);
pub(crate) const CRABOT_PRIMARY_PRESSED: Color = Color::from_rgb8(0x11, 0x7A, 0x70);
pub(crate) const CRABOT_TEXT: Color = Color::from_rgb8(0x33, 0x33, 0x33);
pub(crate) const CRABOT_TEXT_MUTED: Color = Color::from_rgb8(0x66, 0x66, 0x66);

pub(crate) fn crabot_palette() -> iced::theme::Palette {
    iced::theme::Palette {
        background: CRABOT_BG,
        text: CRABOT_TEXT,
        primary: CRABOT_PRIMARY,
        success: Color::from_rgb8(0x4C, 0xAF, 0x50),
        warning: Color::from_rgb8(0xFF, 0xA0, 0x00),
        danger: Color::from_rgb8(0xE8, 0x4E, 0x4E),
    }
}

pub(crate) fn default_theme() -> Theme {
    Theme::custom("Crabot Light", crabot_palette())
}

// ── palette accessors ─────────────────────────────────────────────

pub(crate) fn color_text(theme: &Theme) -> iced::Color {
    theme.palette().text
}
pub(crate) fn color_primary(theme: &Theme) -> iced::Color {
    theme.palette().primary
}
pub(crate) fn color_secondary(theme: &Theme) -> iced::Color {
    theme.extended_palette().secondary.base.color
}
