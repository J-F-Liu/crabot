use iced::{
    Alignment, Element, Fill, Length, Padding, padding,
    widget::{button, checkbox, column, container, row, scrollable, text, text::Wrapping},
};

use crate::views::theme::{color_border, color_muted, color_surface};
use iced_aw::{
    style::{status::Status, tab_bar::Style as TabBarStyle},
    widget::tab_bar::{TabBar, TabLabel},
};

use super::styles::{menu_container_style, menu_item_style};
use super::system_prompt::expandable_header;
use crate::WORKSPACE_TREE;
use crate::app::FileTreePane;
use crate::widgets::popup_menu::PopupMenu;
use crate::widgets::textarea::TextArea;
use crate::{FocusedTarget, PromptEvent};
use crabot::user::WorkMode;

// 8 params: input state, mode, recipes, files, and the UI language.
#[allow(clippy::too_many_arguments)]
pub(crate) fn user_prompt_view<'a>(
    user_prompt: &'a TextArea,
    workmode: WorkMode,
    workmode_enabled: bool,
    prompt_recipes: &'a [String],
    recipe_dropdown_expanded: bool,
    files: &'a FileTreePane,
    workspace_set: bool,
    lang: crabot::i18n::Lang,
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
                _ => iced::Background::Color(color_surface()),
            },
            tab_label_border_color: color_border(),
            text_color: match status {
                Status::Active => iced::Color::WHITE,
                _ => theme.palette().text,
            },
            ..Default::default()
        })
        .into();

    // ── Recipe dropdown ──────────────────────────────────────────
    // 按钮自适应宽度，下拉菜单固定360px
    let underlay = button(text(lang.tr("Recipes ▾")).size(14))
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
                .label(lang.tr("Work mode"))
                .width(Length::Fill)
                .on_toggle(PromptEvent::ToggleWorkMode)
                .style(crate::views::primary_checkbox)
                .text_wrapping(Wrapping::None),
            tab_bar,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        files_field_view(files, workspace_set, lang),
        user_prompt
            .view(|msg| PromptEvent::EditTextArea(FocusedTarget::UserPrompt, msg))
            .height(120),
        row![
            recipe_dropdown,
            iced::widget::Space::new().width(Length::Fill),
            button(text(lang.tr("Send")).size(13).align_x(Alignment::Center))
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

fn files_field_view<'a>(
    files: &'a FileTreePane,
    workspace_set: bool,
    lang: crabot::i18n::Lang,
) -> Element<'a, PromptEvent> {
    let name = WORKSPACE_TREE;
    let header = expandable_header(name, files.enabled, files.expanded, lang);

    use iced::widget::scrollable::{Direction, Scrollbar};

    if files.expanded {
        let body: Element<'_, PromptEvent> = if files.tree.is_empty() {
            text(if workspace_set {
                lang.tr("Loading…")
            } else {
                lang.tr("No workspace selected.")
            })
            .size(12)
            .style(|_| text::Style {
                color: Some(color_muted()),
            })
            .into()
        } else {
            scrollable(
                container(
                    text(&files.tree)
                        .font(iced::Font::MONOSPACE)
                        .wrapping(Wrapping::None),
                )
                .padding(Padding::new(0.0).bottom(12.0)),
            )
            .direction(Direction::Both {
                vertical: Scrollbar::new().width(4).scroller_width(4),
                horizontal: Scrollbar::new().width(4).scroller_width(4),
            })
            .height(Length::Fixed(200.0))
            .into()
        };
        column![header, body]
            .spacing(4)
            .padding(Padding::new(0.0).top(4.0))
            .width(Fill)
            .into()
    } else {
        header
    }
}
