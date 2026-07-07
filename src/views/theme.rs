use iced::widget::scrollable::{Direction, Scrollbar};
use iced::{Color, Theme};

// ── layout constants ──────────────────────────────────────────────

pub(crate) const MIN_W: f32 = 240.0;
pub(crate) const HANDLE: f32 = 4.0;

// ── theme colors ─────────────────────────────────────────────

pub(crate) const CRABOT_BG: Color = Color::from_rgb8(0xF0, 0xF0, 0xF0);
pub(crate) const CRABOT_PANEL: Color = Color::from_rgb8(0xF2, 0xF2, 0xF2);
pub(crate) const CRABOT_SURFACE: Color = Color::from_rgb8(0xE8, 0xE8, 0xE8);
pub(crate) const CRABOT_PRIMARY: Color = Color::from_rgb8(0x1A, 0x9A, 0x8C);
pub(crate) const CRABOT_PRIMARY_HOVER: Color = Color::from_rgb8(0x15, 0x8C, 0x7F);
pub(crate) const CRABOT_PRIMARY_PRESSED: Color = Color::from_rgb8(0x11, 0x7A, 0x70);
pub(crate) const CRABOT_TEXT: Color = Color::from_rgb8(0x33, 0x33, 0x33);
pub(crate) const CRABOT_TEXT_MUTED: Color = Color::from_rgb8(0x66, 0x66, 0x66);
pub(crate) const CRABOT_BORDER: Color = Color::from_rgb8(0xE0, 0xE0, 0xE0);
pub(crate) const CRABOT_USER_BG: Color = Color::from_rgb8(0xEF, 0xF5, 0xFD);
pub(crate) const CRABOT_ASSISTANT_BG: Color = Color::from_rgb8(0xF3, 0xF7, 0xF6);
pub(crate) const CRABOT_TOOL_BG: Color = Color::from_rgb8(0xFB, 0xFB, 0xF8);
pub(crate) const CRABOT_TOOL_ACCENT: Color = Color::from_rgb8(0xD9, 0xA5, 0x58);
pub(crate) const CRABOT_TOOL_CONTENT_BG: Color = Color::from_rgb8(0xFF, 0xF8, 0xF2);
pub(crate) const CRABOT_TOOL_CONTENT_BORDER: Color = Color::from_rgb8(0xF4, 0xF0, 0xEC);
pub(crate) const CRABOT_SUCCESS: Color = Color::from_rgb8(0x2E, 0xB6, 0x7F);
pub(crate) const CRABOT_DANGER: Color = Color::from_rgb8(0xE5, 0x4D, 0x4D);

// ── dialog / modal constants ──────────────────────────────────────

pub(crate) const CRABOT_DIALOG_BG: Color = Color::WHITE;
pub(crate) const CRABOT_DIALOG_RADIUS: f32 = 10.0;
/// Semi-transparent scrim drawn behind in-app modal dialogs.
pub(crate) const CRABOT_MODAL_SCRIM: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.5);

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

/// Thin vertical scrollbar direction for all scrollable widgets.
pub(crate) fn thin_vertical() -> Direction {
    Direction::Vertical(Scrollbar::new().width(4).scroller_width(4))
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
