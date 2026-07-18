use iced::{
    Alignment, Border, Element, Length, padding,
    widget::{button, checkbox, column, container, row, scrollable, text, text::Wrapping},
};

use crate::views::theme::{CRABOT_BORDER, CRABOT_DIALOG_BG, CRABOT_SURFACE, CRABOT_TEXT};
use iced_aw::{
    DropDown,
    core::offset::Offset,
    drop_down,
    style::{status::Status, tab_bar::Style as TabBarStyle},
    widget::tab_bar::{TabBar, TabLabel},
};

use crate::FocusedTarget;
use crate::Message;
use crate::widgets::textarea::TextArea;
use crabot::user::WorkMode;

pub(crate) fn user_prompt_view<'a>(
    user_prompt: &'a TextArea,
    workmode: WorkMode,
    workmode_enabled: bool,
    prompt_recipes: &'a [String],
    recipe_dropdown_expanded: bool,
) -> Element<'a, Message> {
    let mut tab_bar_builder = TabBar::new(Message::SelectWorkMode);
    for mode in WorkMode::all() {
        tab_bar_builder = tab_bar_builder.push(*mode, TabLabel::Text(mode.name.to_string()));
    }
    let tab_bar: Element<'_, Message> = tab_bar_builder
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
    let underlay: Element<'_, Message> = button(text("Recipes ▾").size(13))
        .on_press(Message::ToggleRecipeDropdown)
        .padding([2, 8])
        .style(crate::views::secondary_button)
        .into();

    let overlay = container(
        scrollable(
            column(prompt_recipes.iter().enumerate().map(|(i, recipe)| {
                button(text(recipe.clone()).size(13))
                    .on_press(Message::SelectRecipe(i))
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

    let recipe_dropdown: Element<'_, Message> =
        DropDown::new(underlay, overlay, recipe_dropdown_expanded)
            .width(Length::Fixed(360.0))
            .height(Length::Fixed(180.0))
            .alignment(drop_down::Alignment::Bottom)
            .offset(Offset { x: 8.0, y: 4.0 })
            .on_dismiss(Message::DismissRecipeDropdown)
            .into();

    column![
        row![
            checkbox(workmode_enabled)
                .label("Work mode")
                .width(Length::Fill)
                .on_toggle(Message::ToggleWorkMode)
                .style(crate::views::primary_checkbox)
                .text_wrapping(Wrapping::None),
            tab_bar,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        user_prompt
            .view(|msg| Message::EditTextArea(FocusedTarget::UserPrompt, msg))
            .height(120),
        row![
            recipe_dropdown,
            iced::widget::Space::new().width(Length::Fill),
            button(text("Send").size(13).align_x(Alignment::Center))
                .width(80)
                .on_press(Message::SendPrompt)
                .style(crate::views::primary_button),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(4)
    .padding(padding::bottom(4))
    .into()
}

// ── Recipe dropdown menu styles ───────────────────────────────────

/// Container style for the recipe dropdown popup — surface card with subtle border.
fn menu_container_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(CRABOT_DIALOG_BG.into()),
        border: Border {
            color: CRABOT_BORDER,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// Flat menu-item button style with hover highlight, like a native context menu.
fn menu_item_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: None,
        text_color: CRABOT_TEXT,
        border: Border::default(),
        ..button::Style::default()
    };
    match status {
        button::Status::Hovered | button::Status::Pressed => button::Style {
            background: Some(CRABOT_SURFACE.into()),
            ..base
        },
        _ => base,
    }
}
