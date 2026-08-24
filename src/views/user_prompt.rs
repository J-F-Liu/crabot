use iced::{
    Alignment, Border, Element, Fill, Length, Padding, padding,
    widget::{
        button, checkbox, column, container, row, scrollable, text, text::Wrapping, text_editor,
    },
};

use crate::views::theme::{color_border, color_dialog_bg, color_surface, color_text_strong};
use iced_aw::{
    style::{status::Status, tab_bar::Style as TabBarStyle},
    widget::tab_bar::{TabBar, TabLabel},
};

use super::system_prompt::expandable_header;
use crate::WORKSPACE_TREE;
use crate::app::ExpandableEditor;
use crate::widgets::popup_menu::PopupMenu;
use crate::widgets::textarea::TextArea;
use crate::{FocusedTarget, PromptEvent};
use crabot::user::WorkMode;

pub(crate) fn user_prompt_view<'a>(
    user_prompt: &'a TextArea,
    workmode: WorkMode,
    workmode_enabled: bool,
    prompt_recipes: &'a [String],
    recipe_dropdown_expanded: bool,
    files: &'a ExpandableEditor,
) -> Element<'a, PromptEvent> {
    let mut tab_bar_builder = TabBar::new(PromptEvent::SelectWorkMode);
    for mode in WorkMode::all() {
        tab_bar_builder = tab_bar_builder.push(*mode, TabLabel::Text(mode.name.to_string()));
    }
    let tab_bar: Element<'_, PromptEvent> = tab_bar_builder
        .set_active_tab(&workmode)
        .tab_width(Length::Shrink)
        .width(Length::Shrink)
        .text_size(13.0)
        .padding([0, 8])
        .style(|theme: &iced::Theme, status| TabBarStyle {
            tab_label_background: match status {
                Status::Active => iced::Background::Color(theme.palette().primary),
                Status::Hovered => {
                    iced::Background::Color(theme.extended_palette().primary.weak.color)
                }
                _ => iced::Background::Color(theme.extended_palette().background.weak.color),
            },
            text_color: match status {
                Status::Active => iced::Color::WHITE,
                _ => theme.palette().text,
            },
            ..Default::default()
        })
        .into();

    // ── Recipe dropdown ──────────────────────────────────────────
    // 按钮自适应宽度，下拉菜单固定360px
    let underlay = button(text("Recipes ▾").size(14))
        .on_press(PromptEvent::ToggleRecipeDropdown)
        .padding([4, 8])
        .style(crate::views::secondary_button);

    let overlay = container(
        scrollable(
            column(prompt_recipes.iter().enumerate().map(|(i, recipe)| {
                button(text(recipe.clone()).size(13))
                    .on_press(PromptEvent::SelectRecipe(i))
                    .padding([4, 10])
                    .width(Length::Fill)
                    .style(menu_item_style)
                    .into()
            }))
            .padding([4, 0]),
        )
        .height(Length::Fill),
    )
    .style(menu_container_style);

    let recipe_dropdown: Element<'_, PromptEvent> =
        PopupMenu::new(underlay, overlay, recipe_dropdown_expanded)
            .width(Length::Fixed(360.0))
            .height(Length::Fixed(180.0))
            .gap(2.0)
            .on_dismiss(PromptEvent::DismissRecipeDropdown)
            .into();

    column![
        row![
            checkbox(workmode_enabled)
                .label("Work mode")
                .width(Length::Fill)
                .on_toggle(PromptEvent::ToggleWorkMode)
                .style(crate::views::primary_checkbox)
                .text_wrapping(Wrapping::None),
            tab_bar,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        files_field_view(files),
        user_prompt
            .view(|msg| PromptEvent::EditTextArea(FocusedTarget::UserPrompt, msg))
            .height(120),
        row![
            recipe_dropdown,
            iced::widget::Space::new().width(Length::Fill),
            button(text("Send").size(13).align_x(Alignment::Center))
                .width(80)
                .on_press(PromptEvent::SendPrompt)
                .style(crate::views::primary_button),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(4)
    .padding(padding::bottom(4))
    .into()
}

// ── Workspace tree view ──────────────────────────────────────────

fn files_field_view<'a>(files: &'a ExpandableEditor) -> Element<'a, PromptEvent> {
    let name = WORKSPACE_TREE;
    let header = expandable_header(name, files.enabled, files.expanded);

    use iced::widget::scrollable::{Direction, Scrollbar};

    if files.expanded {
        column![
            header,
            container(
                scrollable(
                    container(
                        text_editor(&files.content)
                            .on_action(move |a| PromptEvent::EditTextContent(name, a))
                            .font(iced::Font::MONOSPACE)
                            .wrapping(text::Wrapping::None),
                    )
                    .padding(Padding::new(0.0).bottom(12.0)),
                )
                .direction(Direction::Both {
                    vertical: Scrollbar::new().width(4).scroller_width(4),
                    horizontal: Scrollbar::new().width(4).scroller_width(4),
                })
                .height(Length::Fixed(200.0)),
            )
            .style(container::bordered_box)
            .width(Fill),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

// ── Popup menu styles (shared) ───────────────────────────────────

/// Popup menu container — surface card with subtle border.
pub(crate) fn menu_container_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(color_dialog_bg().into()),
        border: Border {
            color: color_border(),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// Flat menu-item button style with hover highlight, like a native context menu.
pub(crate) fn menu_item_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: None,
        text_color: color_text_strong(),
        border: Border::default(),
        ..button::Style::default()
    };
    match status {
        button::Status::Hovered | button::Status::Pressed => button::Style {
            background: Some(color_surface().into()),
            ..base
        },
        _ => base,
    }
}
