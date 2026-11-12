use std::borrow::Borrow;

use iced::{
    Border, Color, Element, Font, Length, Shadow, Theme, Vector, font,
    widget::{button, checkbox, container, mouse_area, pick_list, rule, toggler},
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

/// Session header bar: thin border with a faint gray background
/// that subtly sets it apart from the center pane.
pub(crate) fn session_header_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_header_bg().into()),
        border: Border {
            color: color_border(),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

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
        color_accent()
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

/// Colored button style — `on` text on `base`, with hover/pressed variants.
fn colored(
    status: button::Status,
    base: Color,
    hover: Color,
    pressed: Color,
    on: Color,
) -> button::Style {
    let style = button::Style {
        background: Some(base.into()),
        text_color: on,
        border: iced::Border::default().rounded(6),
        ..button::Style::default()
    };
    match status {
        button::Status::Active => style,
        button::Status::Hovered => button::Style {
            background: Some(hover.into()),
            ..style
        },
        button::Status::Pressed => button::Style {
            background: Some(pressed.into()),
            ..style
        },
        button::Status::Disabled => button::Style {
            background: Some(base.scale_alpha(0.5).into()),
            ..style
        },
    }
}

pub(crate) fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    colored(
        status,
        color_accent(),
        color_accent_hover(),
        color_accent_pressed(),
        color_on_accent(),
    )
}

/// Neutral / secondary button style — surface background with a border.
pub(crate) fn secondary_button(_theme: &Theme, status: button::Status) -> button::Style {
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
            background: Some(color_hover().into()),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(color_pressed().into()),
            ..base
        },
        button::Status::Disabled => button::Style {
            background: Some(color_surface().scale_alpha(0.5).into()),
            ..base
        },
    }
}

/// Borderless text button — file paths that open an external diff.
pub(crate) fn link_button(_theme: &Theme, status: button::Status) -> button::Style {
    flat_button(status, color_accent())
}

/// Borderless button: transparent at rest, highlighted on hover/press.
fn flat_button(status: button::Status, text_color: Color) -> button::Style {
    let base = button::Style {
        background: None,
        text_color,
        ..button::Style::default()
    };
    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style {
            background: Some(color_hover().into()),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(color_pressed().into()),
            ..base
        },
        button::Status::Disabled => button::Style {
            text_color: color_muted(),
            ..base
        },
    }
}

/// Destructive red button — irreversible actions like Revert All.
pub(crate) fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    colored(
        status,
        color_danger(),
        color_danger_hover(),
        color_danger_pressed(),
        color_on_danger(),
    )
}

pub(crate) fn primary_toggler(_theme: &Theme, status: toggler::Status) -> toggler::Style {
    let base = toggler::Style {
        background: color_surface().into(),
        background_border_width: 1.0,
        background_border_color: color_border_strong(),
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
                style.background = color_accent().into();
                style.background_border_color = color_accent();
            }
            if matches!(status, toggler::Status::Hovered { .. }) {
                style.background = if is_toggled {
                    color_accent_hover().into()
                } else {
                    color_hover().into()
                };
                style.background_border_color = if is_toggled {
                    color_accent_hover()
                } else {
                    color_border_strong()
                };
            }
            style
        }
    }
}

pub(crate) fn primary_checkbox(_theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let base = checkbox::Style {
        background: color_card().into(),
        icon_color: color_on_accent(),
        border: iced::Border::default()
            .rounded(4)
            .width(1)
            .color(color_border_strong()),
        text_color: Some(color_text_strong()),
    };
    match status {
        checkbox::Status::Disabled { is_checked } => {
            let mut style = base;
            if is_checked {
                style.background = color_muted().into();
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(color_muted());
                style.icon_color = color_card();
            } else {
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(color_border());
            }
            style.text_color = Some(color_muted());
            style
        }
        checkbox::Status::Active { is_checked } | checkbox::Status::Hovered { is_checked } => {
            let mut style = base;
            if is_checked {
                style.background = color_accent().into();
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(color_accent());
                style.icon_color = color_on_accent();
            }
            if matches!(status, checkbox::Status::Hovered { .. }) && is_checked {
                style.background = color_accent_hover().into();
                style.border = iced::Border::default()
                    .rounded(4)
                    .width(1)
                    .color(color_accent_hover());
            }
            style
        }
    }
}

/// Session tab button style.
pub(crate) fn session_tab_style(
    active: bool,
    is_running_bg: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let bg = if active {
            color_card()
        } else if is_running_bg || hovered {
            color_panel()
        } else {
            color_surface()
        };
        let fg = if active || hovered {
            color_text_strong()
        } else {
            color_muted()
        };
        // Every state keeps the same border and radius so toggling `active` never shifts the tab's layout.
        let border = iced::Border {
            color: if active {
                color_border()
            } else {
                Color::TRANSPARENT
            },
            width: 1.0,
            radius: iced::border::Radius {
                top_left: 4.0,
                top_right: 4.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
        };
        button::Style {
            background: Some(bg.into()),
            text_color: fg,
            border,
            ..button::Style::default()
        }
    }
}

/// Small square close button inside a session tab — transparent at rest,
/// rounded highlight with stronger glyph on hover.
pub(crate) fn tab_close_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::Style {
        text_color: color_muted(),
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    };
    match status {
        button::Status::Hovered => {
            style.background = Some(color_border().into());
            style.text_color = color_text_strong();
        }
        button::Status::Pressed => {
            style.background = Some(color_muted().into());
            style.text_color = color_text_strong();
        }
        _ => {}
    }
    style
}

/// Subtle icon-button style — transparent background, dim text.
pub(crate) fn icon_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::Style::default();
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            style.background = Some(color_hover().into());
        }
        _ => {}
    }
    style.text_color = color_muted();
    style
}

/// Small chip holding one selected attachment above the send row.
pub(crate) fn attachment_chip_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_surface().into()),
        border: Border {
            color: color_border(),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
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
    container::Style {
        background: Some(color_reasoning_bg().into()),
        border: Border {
            color: color_reasoning_border(),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

/// Small role badge (User / Assistant / Tool).
pub(crate) fn role_badge_style(role: &str) -> impl Fn(&Theme) -> container::Style + '_ {
    let fg = match role {
        "User" => color_role_user(),
        "Assistant" => color_role_assistant(),
        "Tool" => color_role_tool(),
        _ => color_muted(),
    };
    move |_theme: &Theme| container::Style {
        text_color: Some(fg),
        ..container::Style::default()
    }
}

// ── dropdown styles ───────────────────────────────────────────────

/// Popup menu container — surface card with subtle border.
pub(crate) fn menu_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_surface().into()),
        border: Border {
            color: color_border(),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// Pick-list menu styled like [`menu_container_style`].
pub(crate) fn pick_list_menu_style(_theme: &Theme) -> iced::widget::overlay::menu::Style {
    iced::widget::overlay::menu::Style {
        background: color_surface().into(),
        border: iced::Border::default()
            .rounded(6)
            .width(1)
            .color(color_border()),
        text_color: color_text_strong(),
        selected_text_color: color_on_accent(),
        selected_background: color_accent().into(),
        shadow: Shadow::default(),
    }
}

/// Flat menu-item button style with hover highlight, like a native context menu.
pub(crate) fn menu_item_style(_theme: &Theme, status: button::Status) -> button::Style {
    flat_button(status, color_text_strong())
}

/// Hover/pressed trigger colors are shared with [`color_hover`] / [`color_pressed`].
struct DropdownColors {
    hover: Color,
    pressed: Color,
    surface: Color,
    border: iced::Border,
}

fn dropdown_colors() -> DropdownColors {
    DropdownColors {
        hover: color_hover(),
        pressed: color_pressed(),
        surface: color_surface(),
        border: iced::Border::default()
            .rounded(6)
            .width(1)
            .color(color_border()),
    }
}

/// Interaction state shared by the `pick_list` and `DropDown` trigger styles.
#[derive(Clone, Copy)]
enum TriggerStatus {
    Active,
    Hovered,
    Opened,
}

impl DropdownColors {
    /// Trigger background for the given interaction state.
    fn trigger_background(&self, state: TriggerStatus) -> iced::Background {
        match state {
            TriggerStatus::Active => self.surface,
            TriggerStatus::Hovered => self.hover,
            TriggerStatus::Opened => self.pressed,
        }
        .into()
    }
}

impl From<pick_list::Status> for TriggerStatus {
    fn from(status: pick_list::Status) -> Self {
        match status {
            pick_list::Status::Active => Self::Active,
            pick_list::Status::Hovered => Self::Hovered,
            pick_list::Status::Opened { .. } => Self::Opened,
        }
    }
}

impl From<crate::widgets::dropdown::Status> for TriggerStatus {
    fn from(status: crate::widgets::dropdown::Status) -> Self {
        match status {
            crate::widgets::dropdown::Status::Active => Self::Active,
            crate::widgets::dropdown::Status::Hovered => Self::Hovered,
            crate::widgets::dropdown::Status::Opened => Self::Opened,
        }
    }
}

/// iced `pick_list` trigger styled like [`secondary_button`].
pub(crate) fn pick_list_style(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let colors = dropdown_colors();
    pick_list::Style {
        text_color: color_text_strong(),
        placeholder_color: color_muted(),
        handle_color: color_muted(),
        background: colors.trigger_background(status.into()),
        border: colors.border,
    }
}

/// Custom `DropDown` trigger styled like [`secondary_button`].
pub(crate) fn secondary_dropdown_style(
    _theme: &Theme,
    status: crate::widgets::dropdown::Status,
) -> crate::widgets::dropdown::Style {
    let colors = dropdown_colors();
    crate::widgets::dropdown::Style {
        text_color: color_text_strong(),
        placeholder_color: color_muted(),
        handle_color: color_muted(),
        background: colors.trigger_background(status.into()),
        border: colors.border,
    }
}

/// Muted, hover-less trigger for a `DropDown` whose session is running.
pub(crate) fn muted_dropdown_style(
    _theme: &Theme,
    _status: crate::widgets::dropdown::Status,
) -> crate::widgets::dropdown::Style {
    let colors = dropdown_colors();
    crate::widgets::dropdown::Style {
        text_color: color_muted(),
        placeholder_color: color_muted(),
        handle_color: color_muted(),
        background: colors.surface.into(),
        border: colors.border,
    }
}

/// `pick_list` with the shared trigger and menu styles applied.
pub(crate) fn styled_pick_list<'a, T, L, V, Message>(
    options: L,
    selected: Option<V>,
    on_selected: impl Fn(T) -> Message + 'a,
) -> iced::widget::PickList<'a, T, L, V, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone,
{
    iced::widget::pick_list(options, selected, on_selected)
        .style(pick_list_style)
        .menu_style(pick_list_menu_style)
}

/// Floating tooltip box — dark rounded background with a subtle shadow.
pub(crate) fn tooltip_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_tooltip_bg().into()),
        border: Border {
            color: color_border(),
            width: 1.0,
            radius: 6.0.into(),
        },
        text_color: Some(color_tooltip_text()),
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}

// ── selectable text styles ────────────────────────────────────────

fn selection_style(color: Color) -> SelectionStyle {
    SelectionStyle {
        color: Some(color),
        selection: color_selection(),
    }
}

pub(crate) fn sel_default(_theme: &Theme) -> SelectionStyle {
    selection_style(color_text_strong())
}

pub(crate) fn sel_primary(_theme: &Theme) -> SelectionStyle {
    selection_style(color_accent())
}

pub(crate) fn sel_muted(_theme: &Theme) -> SelectionStyle {
    selection_style(color_muted())
}
