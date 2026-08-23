use std::sync::atomic::{AtomicBool, Ordering};

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
/// Faint gray background for the session header bar.
pub(crate) const CRABOT_HEADER_BG: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
pub(crate) const CRABOT_SUCCESS: Color = Color::from_rgb8(0x2E, 0xB6, 0x7F);
pub(crate) const CRABOT_DANGER: Color = Color::from_rgb8(0xE5, 0x4D, 0x4D);
pub(crate) const CRABOT_DANGER_HOVER: Color = Color::from_rgb8(0xC9, 0x3A, 0x3A);
pub(crate) const CRABOT_DANGER_PRESSED: Color = Color::from_rgb8(0xAF, 0x2E, 0x2E);
pub(crate) const CRABOT_YELLOW: Color = Color::from_rgb8(0xF0, 0xCC, 0x00);

// ── dark theme colors ─────────────────────────────────────────

pub(crate) const DARK_BG: Color = Color::from_rgb8(0x14, 0x16, 0x1A);
pub(crate) const DARK_PANEL: Color = Color::from_rgb8(0x1B, 0x1E, 0x24);
pub(crate) const DARK_CARD: Color = Color::from_rgb8(0x20, 0x24, 0x2B);
pub(crate) const DARK_SURFACE: Color = Color::from_rgb8(0x2A, 0x2F, 0x38);
pub(crate) const DARK_TEXT: Color = Color::from_rgb8(0xE2, 0xE5, 0xEA);
pub(crate) const DARK_TEXT_MUTED: Color = Color::from_rgb8(0x9B, 0xA1, 0xAB);
pub(crate) const DARK_BORDER: Color = Color::from_rgb8(0x34, 0x39, 0x45);
pub(crate) const DARK_USER_BG: Color = Color::from_rgb8(0x1E, 0x29, 0x38);
pub(crate) const DARK_ASSISTANT_BG: Color = Color::from_rgb8(0x21, 0x25, 0x2C);
pub(crate) const DARK_TOOL_BG: Color = Color::from_rgb8(0x28, 0x25, 0x20);
pub(crate) const DARK_TOOL_CONTENT_BG: Color = Color::from_rgb8(0x2B, 0x27, 0x21);
pub(crate) const DARK_TOOL_CONTENT_BORDER: Color = Color::from_rgb8(0x3D, 0x38, 0x2E);
pub(crate) const DARK_DIALOG_BG: Color = Color::from_rgb8(0x23, 0x27, 0x30);
/// Faint gray background for the session header bar (dark mode).
pub(crate) const DARK_HEADER_BG: Color = Color::from_rgb8(0x24, 0x28, 0x2F);

/// Diff-row background for deletions (light pink).
const DIFF_BG_DEL_LIGHT: Color = Color::from_rgb8(0xFF, 0xF0, 0xF0);
/// Diff-row background for additions (light green).
const DIFF_BG_ADD_LIGHT: Color = Color::from_rgb8(0xF0, 0xFA, 0xF4);
/// Diff-row background for deletions (muted dark red).
const DIFF_BG_DEL_DARK: Color = Color::from_rgb8(0x3D, 0x20, 0x25);
/// Diff-row background for additions (muted dark green).
const DIFF_BG_ADD_DARK: Color = Color::from_rgb8(0x1E, 0x2D, 0x25);

pub(crate) const CRABOT_DIALOG_BG: Color = Color::WHITE;
pub(crate) const CRABOT_DIALOG_RADIUS: f32 = 10.0;
/// Semi-transparent scrim drawn behind in-app modal dialogs.
pub(crate) const CRABOT_MODAL_SCRIM: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.5);

// ── dark mode state ───────────────────────────────────────────────

static DARK_MODE: AtomicBool = AtomicBool::new(false);

/// Set the active color mode. Called once at boot and on every toggle;
/// all `color_*` accessors below read this flag.
pub(crate) fn set_dark_mode(dark: bool) {
    DARK_MODE.store(dark, Ordering::Relaxed);
}

/// Whether dark mode is currently active.
pub(crate) fn is_dark() -> bool {
    DARK_MODE.load(Ordering::Relaxed)
}

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

pub(crate) fn dark_palette() -> iced::theme::Palette {
    iced::theme::Palette {
        background: DARK_BG,
        text: DARK_TEXT,
        primary: CRABOT_PRIMARY,
        success: Color::from_rgb8(0x4C, 0xAF, 0x50),
        warning: Color::from_rgb8(0xFF, 0xA0, 0x00),
        danger: Color::from_rgb8(0xE8, 0x4E, 0x4E),
    }
}

pub(crate) fn default_theme() -> Theme {
    Theme::custom("Crabot Light", crabot_palette())
}

pub(crate) fn dark_theme() -> Theme {
    Theme::custom("Crabot Dark", dark_palette())
}

/// The application theme for the given mode.
pub(crate) fn theme_for(dark: bool) -> Theme {
    if dark { dark_theme() } else { default_theme() }
}

fn thin_scrollbar() -> Scrollbar {
    Scrollbar::new().width(4).scroller_width(4)
}

/// Thin vertical scrollbar direction for all scrollable widgets.
pub(crate) fn thin_vertical() -> Direction {
    Direction::Vertical(thin_scrollbar())
}

/// Thin horizontal scrollbar direction, used by the process list.
pub(crate) fn thin_horizontal() -> Direction {
    Direction::Horizontal(thin_scrollbar())
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

// ── mode-aware color accessors ────────────────────────────────────

/// Side-pane background.
pub(crate) fn color_panel() -> Color {
    if is_dark() { DARK_PANEL } else { CRABOT_PANEL }
}
/// Raised card background (center pane, input fields, status bars).
pub(crate) fn color_card() -> Color {
    if is_dark() { DARK_CARD } else { Color::WHITE }
}
/// Neutral surface for buttons, menus, and hover highlights.
pub(crate) fn color_surface() -> Color {
    if is_dark() {
        DARK_SURFACE
    } else {
        CRABOT_SURFACE
    }
}
/// Primary text color.
pub(crate) fn color_text_strong() -> Color {
    if is_dark() { DARK_TEXT } else { CRABOT_TEXT }
}
/// Dimmed text color for secondary labels.
pub(crate) fn color_muted() -> Color {
    if is_dark() {
        DARK_TEXT_MUTED
    } else {
        CRABOT_TEXT_MUTED
    }
}
/// Subtle border color.
pub(crate) fn color_border() -> Color {
    if is_dark() {
        DARK_BORDER
    } else {
        CRABOT_BORDER
    }
}
/// User message bubble background.
pub(crate) fn color_user_bg() -> Color {
    if is_dark() {
        DARK_USER_BG
    } else {
        CRABOT_USER_BG
    }
}
/// Assistant message bubble background.
pub(crate) fn color_assistant_bg() -> Color {
    if is_dark() {
        DARK_ASSISTANT_BG
    } else {
        CRABOT_ASSISTANT_BG
    }
}
/// Tool message bubble background.
pub(crate) fn color_tool_bg() -> Color {
    if is_dark() {
        DARK_TOOL_BG
    } else {
        CRABOT_TOOL_BG
    }
}
/// Expanded tool-call content background.
pub(crate) fn color_tool_content_bg() -> Color {
    if is_dark() {
        DARK_TOOL_CONTENT_BG
    } else {
        CRABOT_TOOL_CONTENT_BG
    }
}
/// Expanded tool-call content border.
pub(crate) fn color_tool_content_border() -> Color {
    if is_dark() {
        DARK_TOOL_CONTENT_BORDER
    } else {
        CRABOT_TOOL_CONTENT_BORDER
    }
}
/// Dialog and modal background.
pub(crate) fn color_dialog_bg() -> Color {
    if is_dark() {
        DARK_DIALOG_BG
    } else {
        CRABOT_DIALOG_BG
    }
}
/// Session header bar background (faint gray).
pub(crate) fn color_header_bg() -> Color {
    if is_dark() {
        DARK_HEADER_BG
    } else {
        CRABOT_HEADER_BG
    }
}
/// Diff-row background for deletions.
pub(crate) fn color_diff_bg_del() -> Color {
    if is_dark() {
        DIFF_BG_DEL_DARK
    } else {
        DIFF_BG_DEL_LIGHT
    }
}
/// Diff-row background for additions.
pub(crate) fn color_diff_bg_add() -> Color {
    if is_dark() {
        DIFF_BG_ADD_DARK
    } else {
        DIFF_BG_ADD_LIGHT
    }
}
