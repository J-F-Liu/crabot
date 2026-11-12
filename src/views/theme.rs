use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::{Color, Theme};

use crabot::settings::ColorStyle;

// ── layout constants ──────────────────────────────────────────────

pub(crate) const MIN_W: f32 = 240.0;
pub(crate) const HANDLE: f32 = 4.0;
pub(crate) const CRABOT_DIALOG_RADIUS: f32 = 10.0;

// ── palette table ─────────────────────────────────────────────────

/// Colors for one palette; only the accessors below expose them.
struct ModeColors {
    /// Window frame and iced palette background.
    bg: Color,
    /// Side panes.
    panel: Color,
    /// Center pane, status bars, inputs.
    card: Color,
    /// Neutral control surfaces (buttons, menus).
    surface: Color,
    /// Hover background drawn on top of `surface`.
    hover: Color,
    /// Pressed background, one step stronger than `hover`.
    pressed: Color,
    /// Recessed card used for settings form sections and fields.
    recessed: Color,
    /// Session header bar.
    header: Color,
    /// Dialogs and modals.
    dialog: Color,
    /// Decorative borders and dividers.
    border: Color,
    /// Focus rings, active borders, scrollbar scrollers.
    border_strong: Color,
    text: Color,
    text_muted: Color,
    accent: Color,
    accent_hover: Color,
    accent_pressed: Color,
    /// Glyph/ink color on top of [`accent`](Self::accent).
    on_accent: Color,
    user_bg: Color,
    assistant_bg: Color,
    tool_bg: Color,
    tool_content_bg: Color,
    tool_content_border: Color,
    /// Tool-name accent (amber family).
    tool_accent: Color,
    success: Color,
    /// Solid destructive button background.
    danger: Color,
    danger_hover: Color,
    danger_pressed: Color,
    /// Bright red for error text and failure glyphs.
    danger_text: Color,
    on_danger: Color,
    warning: Color,
    /// Pending-task dot and other orange highlights.
    orange: Color,
    /// Informational blue (in-progress todo rows, ACP status).
    info: Color,
    diff_add: Color,
    diff_del: Color,
    reasoning_bg: Color,
    reasoning_border: Color,
    /// Role badge colors for user, assistant, and tool labels.
    role_user: Color,
    role_assistant: Color,
    role_tool: Color,
    code_bg: Color,
    code_border: Color,
    code_text: Color,
    tooltip_bg: Color,
    tooltip_text: Color,
    /// Scrim behind in-app modals.
    scrim: Color,
    /// Selection highlight painted behind selected text.
    selection: Color,
    /// Search-hit highlight behind matched keywords.
    search_highlight: Color,
}

const LIGHT: ModeColors = ModeColors {
    bg: Color::from_rgb8(0xF0, 0xF0, 0xF0),
    panel: Color::from_rgb8(0xF2, 0xF2, 0xF2),
    card: Color::WHITE,
    surface: Color::from_rgb8(0xE8, 0xE8, 0xE8),
    hover: Color::from_rgb8(0xD8, 0xD8, 0xD8),
    pressed: Color::from_rgb8(0xC8, 0xC8, 0xC8),
    recessed: Color::from_rgb8(0xF4, 0xF4, 0xF4),
    header: Color::from_rgb8(0xF5, 0xF5, 0xF5),
    dialog: Color::WHITE,
    border: Color::from_rgb8(0xE0, 0xE0, 0xE0),
    border_strong: Color::from_rgb8(0xB8, 0xB8, 0xB8),
    text: Color::from_rgb8(0x33, 0x33, 0x33),
    text_muted: Color::from_rgb8(0x66, 0x66, 0x66),
    accent: Color::from_rgb8(0x1A, 0x9A, 0x8C),
    accent_hover: Color::from_rgb8(0x15, 0x8C, 0x7F),
    accent_pressed: Color::from_rgb8(0x11, 0x7A, 0x70),
    on_accent: Color::WHITE,
    user_bg: Color::from_rgb8(0xEF, 0xF5, 0xFD),
    assistant_bg: Color::from_rgb8(0xF3, 0xF7, 0xF6),
    tool_bg: Color::from_rgb8(0xFB, 0xFB, 0xF8),
    tool_content_bg: Color::from_rgb8(0xFF, 0xF8, 0xF2),
    tool_content_border: Color::from_rgb8(0xF4, 0xF0, 0xEC),
    tool_accent: Color::from_rgb8(0xD9, 0xA5, 0x58),
    success: Color::from_rgb8(0x2E, 0xB6, 0x7F),
    danger: Color::from_rgb8(0xE5, 0x4D, 0x4D),
    danger_hover: Color::from_rgb8(0xC9, 0x3A, 0x3A),
    danger_pressed: Color::from_rgb8(0xAF, 0x2E, 0x2E),
    danger_text: Color::from_rgb8(0xE5, 0x4D, 0x4D),
    on_danger: Color::WHITE,
    warning: Color::from_rgb8(0xF0, 0xCC, 0x00),
    orange: Color::from_rgb8(0xFF, 0x8C, 0x00),
    info: Color::from_rgb8(0x29, 0x76, 0xFF),
    diff_add: Color::from_rgb8(0xF0, 0xFA, 0xF4),
    diff_del: Color::from_rgb8(0xFF, 0xF0, 0xF0),
    reasoning_bg: Color::from_rgba(0.0, 0.0, 0.0, 0.035),
    reasoning_border: Color::from_rgba(0.0, 0.0, 0.0, 0.06),
    role_user: Color::from_rgb8(0x4A, 0x90, 0xD9),
    role_assistant: Color::from_rgb8(0x1A, 0x9A, 0x8C),
    role_tool: Color::from_rgb8(0xD0, 0x8F, 0x33),
    code_bg: Color::from_rgb8(0x11, 0x11, 0x11),
    code_border: Color::from_rgb8(0x11, 0x11, 0x11),
    code_text: Color::WHITE,
    tooltip_bg: Color::from_rgba(0.2, 0.2, 0.2, 0.95),
    tooltip_text: Color::WHITE,
    scrim: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
    selection: Color::from_rgba(0.1, 0.6, 0.55, 0.28),
    search_highlight: Color::from_rgba(1.0, 0.92, 0.0, 0.35),
};

/// Style A — cool graphite neutrals with a brightened brand teal.
const DARK_GRAPHITE: ModeColors = ModeColors {
    bg: Color::from_rgb8(0x0F, 0x11, 0x15),
    panel: Color::from_rgb8(0x17, 0x1A, 0x20),
    card: Color::from_rgb8(0x1D, 0x21, 0x27),
    surface: Color::from_rgb8(0x26, 0x2B, 0x33),
    hover: Color::from_rgb8(0x2F, 0x35, 0x3F),
    pressed: Color::from_rgb8(0x37, 0x3F, 0x4A),
    recessed: Color::from_rgb8(0x20, 0x25, 0x2C),
    header: Color::from_rgb8(0x23, 0x28, 0x30),
    dialog: Color::from_rgb8(0x23, 0x29, 0x32),
    border: Color::from_rgb8(0x33, 0x3A, 0x46),
    border_strong: Color::from_rgb8(0x49, 0x52, 0x5F),
    text: Color::from_rgb8(0xE7, 0xEA, 0xF0),
    text_muted: Color::from_rgb8(0xA4, 0xAC, 0xB8),
    accent: Color::from_rgb8(0x33, 0xB6, 0xA5),
    accent_hover: Color::from_rgb8(0x46, 0xC6, 0xB4),
    accent_pressed: Color::from_rgb8(0x27, 0xA3, 0x92),
    on_accent: Color::from_rgb8(0x06, 0x23, 0x1F),
    user_bg: Color::from_rgb8(0x1C, 0x2E, 0x42),
    assistant_bg: Color::from_rgb8(0x25, 0x2B, 0x34),
    tool_bg: Color::from_rgb8(0x2A, 0x28, 0x20),
    tool_content_bg: Color::from_rgb8(0x2F, 0x2C, 0x24),
    tool_content_border: Color::from_rgb8(0x40, 0x3C, 0x31),
    tool_accent: Color::from_rgb8(0xE2, 0xAC, 0x60),
    success: Color::from_rgb8(0x4C, 0xC4, 0x92),
    danger: Color::from_rgb8(0xBE, 0x45, 0x3E),
    danger_hover: Color::from_rgb8(0xCF, 0x55, 0x4C),
    danger_pressed: Color::from_rgb8(0xA5, 0x3B, 0x35),
    danger_text: Color::from_rgb8(0xF0, 0x7C, 0x74),
    on_danger: Color::WHITE,
    warning: Color::from_rgb8(0xE8, 0xB0, 0x4B),
    orange: Color::from_rgb8(0xEE, 0x9A, 0x4B),
    info: Color::from_rgb8(0x7A, 0xA7, 0xF0),
    diff_add: Color::from_rgb8(0x17, 0x30, 0x26),
    diff_del: Color::from_rgb8(0x3A, 0x1F, 0x26),
    reasoning_bg: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
    reasoning_border: Color::from_rgba(1.0, 1.0, 1.0, 0.09),
    role_user: Color::from_rgb8(0x7F, 0xB8, 0xE8),
    role_assistant: Color::from_rgb8(0x33, 0xB6, 0xA5),
    role_tool: Color::from_rgb8(0xE2, 0xAC, 0x60),
    code_bg: Color::from_rgb8(0x14, 0x17, 0x1C),
    code_border: Color::from_rgb8(0x33, 0x3A, 0x46),
    code_text: Color::from_rgb8(0xE7, 0xEA, 0xF0),
    tooltip_bg: Color::from_rgb8(0x0B, 0x0D, 0x11),
    tooltip_text: Color::from_rgb8(0xE7, 0xEA, 0xF0),
    scrim: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
    selection: Color::from_rgba(0.2, 0.71, 0.65, 0.35),
    search_highlight: Color::from_rgba(0.95, 0.78, 0.2, 0.3),
};

/// Style B — blue-slate surfaces with a sky-cyan accent.
const DARK_OCEAN: ModeColors = ModeColors {
    bg: Color::from_rgb8(0x0A, 0x0F, 0x16),
    panel: Color::from_rgb8(0x11, 0x1A, 0x24),
    card: Color::from_rgb8(0x16, 0x20, 0x2B),
    surface: Color::from_rgb8(0x1E, 0x2B, 0x39),
    hover: Color::from_rgb8(0x26, 0x37, 0x47),
    pressed: Color::from_rgb8(0x2F, 0x43, 0x56),
    recessed: Color::from_rgb8(0x1A, 0x26, 0x32),
    header: Color::from_rgb8(0x1B, 0x27, 0x33),
    dialog: Color::from_rgb8(0x1E, 0x2B, 0x39),
    border: Color::from_rgb8(0x29, 0x38, 0x49),
    border_strong: Color::from_rgb8(0x3F, 0x55, 0x6B),
    text: Color::from_rgb8(0xE4, 0xEB, 0xF3),
    text_muted: Color::from_rgb8(0x94, 0xA6, 0xB9),
    accent: Color::from_rgb8(0x5B, 0xB8, 0xE6),
    accent_hover: Color::from_rgb8(0x72, 0xC8, 0xF0),
    accent_pressed: Color::from_rgb8(0x43, 0xA4, 0xD4),
    on_accent: Color::from_rgb8(0x04, 0x22, 0x2F),
    user_bg: Color::from_rgb8(0x15, 0x2C, 0x44),
    assistant_bg: Color::from_rgb8(0x1C, 0x28, 0x35),
    tool_bg: Color::from_rgb8(0x1E, 0x2A, 0x35),
    tool_content_bg: Color::from_rgb8(0x23, 0x2F, 0x3B),
    tool_content_border: Color::from_rgb8(0x33, 0x41, 0x4F),
    tool_accent: Color::from_rgb8(0xE3, 0xB3, 0x75),
    success: Color::from_rgb8(0x4F, 0xC0, 0x8D),
    danger: Color::from_rgb8(0xBE, 0x46, 0x40),
    danger_hover: Color::from_rgb8(0xCF, 0x56, 0x4D),
    danger_pressed: Color::from_rgb8(0xA5, 0x3C, 0x37),
    danger_text: Color::from_rgb8(0xF0, 0x7F, 0x78),
    on_danger: Color::WHITE,
    warning: Color::from_rgb8(0xE8, 0xB0, 0x4B),
    orange: Color::from_rgb8(0xEE, 0x9A, 0x4B),
    info: Color::from_rgb8(0x7A, 0xA7, 0xF0),
    diff_add: Color::from_rgb8(0x15, 0x30, 0x2A),
    diff_del: Color::from_rgb8(0x37, 0x20, 0x2B),
    reasoning_bg: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
    reasoning_border: Color::from_rgba(1.0, 1.0, 1.0, 0.09),
    role_user: Color::from_rgb8(0x8A, 0xA9, 0xD6),
    role_assistant: Color::from_rgb8(0x5B, 0xB8, 0xE6),
    role_tool: Color::from_rgb8(0xE3, 0xB3, 0x75),
    code_bg: Color::from_rgb8(0x10, 0x18, 0x21),
    code_border: Color::from_rgb8(0x29, 0x38, 0x49),
    code_text: Color::from_rgb8(0xE4, 0xEB, 0xF3),
    tooltip_bg: Color::from_rgb8(0x08, 0x0D, 0x12),
    tooltip_text: Color::from_rgb8(0xE4, 0xEB, 0xF3),
    scrim: Color::from_rgba(0.0, 0.0, 0.0, 0.65),
    selection: Color::from_rgba(0.36, 0.72, 0.9, 0.32),
    search_highlight: Color::from_rgba(0.95, 0.78, 0.2, 0.3),
};

/// Style C — warm charcoal surfaces with an amber accent.
const DARK_EMBER: ModeColors = ModeColors {
    bg: Color::from_rgb8(0x10, 0x0E, 0x0C),
    panel: Color::from_rgb8(0x17, 0x14, 0x0F),
    card: Color::from_rgb8(0x1E, 0x1A, 0x15),
    surface: Color::from_rgb8(0x27, 0x21, 0x19),
    hover: Color::from_rgb8(0x32, 0x2A, 0x20),
    pressed: Color::from_rgb8(0x3C, 0x33, 0x27),
    recessed: Color::from_rgb8(0x24, 0x1E, 0x15),
    header: Color::from_rgb8(0x26, 0x21, 0x1A),
    dialog: Color::from_rgb8(0x28, 0x22, 0x18),
    border: Color::from_rgb8(0x3A, 0x32, 0x26),
    border_strong: Color::from_rgb8(0x54, 0x49, 0x37),
    text: Color::from_rgb8(0xED, 0xE5, 0xD8),
    text_muted: Color::from_rgb8(0xA9, 0x9E, 0x8C),
    accent: Color::from_rgb8(0xE0, 0xA4, 0x58),
    accent_hover: Color::from_rgb8(0xED, 0xB4, 0x69),
    accent_pressed: Color::from_rgb8(0xC9, 0x8F, 0x46),
    on_accent: Color::from_rgb8(0x24, 0x1A, 0x0B),
    user_bg: Color::from_rgb8(0x30, 0x28, 0x18),
    assistant_bg: Color::from_rgb8(0x27, 0x22, 0x19),
    tool_bg: Color::from_rgb8(0x2C, 0x24, 0x1A),
    tool_content_bg: Color::from_rgb8(0x31, 0x29, 0x20),
    tool_content_border: Color::from_rgb8(0x45, 0x3A, 0x28),
    tool_accent: Color::from_rgb8(0xC9, 0xA8, 0x7C),
    success: Color::from_rgb8(0x9C, 0xC4, 0x74),
    danger: Color::from_rgb8(0xBE, 0x4C, 0x39),
    danger_hover: Color::from_rgb8(0xCF, 0x5B, 0x47),
    danger_pressed: Color::from_rgb8(0xA5, 0x40, 0x2F),
    danger_text: Color::from_rgb8(0xF0, 0x85, 0x73),
    on_danger: Color::WHITE,
    warning: Color::from_rgb8(0xE8, 0xB0, 0x4B),
    orange: Color::from_rgb8(0xEE, 0x9A, 0x4B),
    info: Color::from_rgb8(0xE0, 0xA4, 0x58),
    diff_add: Color::from_rgb8(0x23, 0x2D, 0x19),
    diff_del: Color::from_rgb8(0x38, 0x21, 0x1C),
    reasoning_bg: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
    reasoning_border: Color::from_rgba(1.0, 1.0, 1.0, 0.09),
    role_user: Color::from_rgb8(0x8F, 0xB4, 0xD9),
    role_assistant: Color::from_rgb8(0xE0, 0xA4, 0x58),
    role_tool: Color::from_rgb8(0xC9, 0xA8, 0x7C),
    code_bg: Color::from_rgb8(0x14, 0x11, 0x0D),
    code_border: Color::from_rgb8(0x3A, 0x32, 0x26),
    code_text: Color::from_rgb8(0xED, 0xE5, 0xD8),
    tooltip_bg: Color::from_rgb8(0x0C, 0x0A, 0x08),
    tooltip_text: Color::from_rgb8(0xF0, 0xE9, 0xDD),
    scrim: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
    selection: Color::from_rgba(0.88, 0.64, 0.35, 0.32),
    search_highlight: Color::from_rgba(0.95, 0.75, 0.35, 0.3),
};

// ── active mode state ─────────────────────────────────────────────

static DARK_MODE: AtomicBool = AtomicBool::new(false);
/// Stored as `ColorStyle` discriminant so the setting survives table reorders.
static COLOR_STYLE: AtomicU8 = AtomicU8::new(ColorStyle::Graphite as u8);

/// Set the active color mode used by all palette accessors.
pub(crate) fn set_dark_mode(dark: bool) {
    DARK_MODE.store(dark, Ordering::Relaxed);
}

/// Whether dark mode is currently active.
pub(crate) fn is_dark() -> bool {
    DARK_MODE.load(Ordering::Relaxed)
}

/// Select the color style used while dark mode is active.
pub(crate) fn set_color_style(style: ColorStyle) {
    COLOR_STYLE.store(style as u8, Ordering::Relaxed);
}

/// The active color style.
fn color_style() -> ColorStyle {
    match COLOR_STYLE.load(Ordering::Relaxed) {
        value if value == ColorStyle::Ocean as u8 => ColorStyle::Ocean,
        value if value == ColorStyle::Ember as u8 => ColorStyle::Ember,
        _ => ColorStyle::Graphite,
    }
}

fn colors_for(dark: bool) -> &'static ModeColors {
    if dark { dark_colors() } else { &LIGHT }
}

fn colors() -> &'static ModeColors {
    colors_for(is_dark())
}

fn dark_colors() -> &'static ModeColors {
    match color_style() {
        ColorStyle::Graphite => &DARK_GRAPHITE,
        ColorStyle::Ocean => &DARK_OCEAN,
        ColorStyle::Ember => &DARK_EMBER,
    }
}

/// iced palette derived from a [`ModeColors`] table.
fn palette(colors: &ModeColors) -> iced::theme::Palette {
    iced::theme::Palette {
        background: colors.bg,
        text: colors.text,
        primary: colors.accent,
        success: colors.success,
        warning: colors.warning,
        danger: colors.danger,
    }
}

/// The application theme for the active mode and color style.
pub(crate) fn theme_for_current() -> Theme {
    let dark = is_dark();
    let name = if dark { "Crabot Dark" } else { "Crabot Light" };
    Theme::custom(name, palette(colors_for(dark)))
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

// ── mode-aware color accessors ────────────────────────────────────

macro_rules! color_accessors {
    ($($name:ident => $field:ident),+ $(,)?) => {
        $(
            pub(crate) fn $name() -> Color {
                colors().$field
            }
        )+
    };
}

color_accessors! {
    color_bg => bg,
    color_panel => panel,
    color_card => card,
    color_surface => surface,
    color_hover => hover,
    color_pressed => pressed,
    color_recessed => recessed,
    color_header_bg => header,
    color_dialog_bg => dialog,
    color_text_strong => text,
    color_muted => text_muted,
    color_border => border,
    color_border_strong => border_strong,
    color_accent => accent,
    color_accent_hover => accent_hover,
    color_accent_pressed => accent_pressed,
    color_on_accent => on_accent,
    color_user_bg => user_bg,
    color_assistant_bg => assistant_bg,
    color_tool_bg => tool_bg,
    color_tool_content_bg => tool_content_bg,
    color_tool_content_border => tool_content_border,
    color_tool_accent => tool_accent,
    color_success => success,
    color_danger => danger,
    color_danger_hover => danger_hover,
    color_danger_pressed => danger_pressed,
    color_danger_text => danger_text,
    color_on_danger => on_danger,
    color_warning => warning,
    color_orange => orange,
    color_info => info,
    color_diff_bg_add => diff_add,
    color_diff_bg_del => diff_del,
    color_reasoning_bg => reasoning_bg,
    color_reasoning_border => reasoning_border,
    color_role_user => role_user,
    color_role_assistant => role_assistant,
    color_role_tool => role_tool,
    color_code_bg => code_bg,
    color_code_border => code_border,
    color_code_text => code_text,
    color_tooltip_bg => tooltip_bg,
    color_tooltip_text => tooltip_text,
    color_scrim => scrim,
    color_selection => selection,
    color_search_highlight => search_highlight,
}
