use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, row, text, text_input},
};

use super::{
    SettingsEvent, SettingsState, SettingsTab, card_rule, delete_button_style, form_card_style,
};
use crate::views::theme::{CRABOT_PRIMARY, CRABOT_TEXT_MUTED};
use crabot::user::WorkMode;

// ── Page ───────────────────────────────────────────────────────────

pub(super) fn prompt_recipes_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = row![
        text("Prompt Recipes")
            .size(13)
            .font(iced::Font {
                weight: iced::font::Weight::Bold,
                ..iced::Font::DEFAULT
            })
            .color(CRABOT_PRIMARY),
    ]
    .align_y(Alignment::Center);

    let work_modes = WorkMode::all();
    let body: Element<'a, SettingsEvent> = if work_modes.is_empty() {
        container(
            text("No work modes found. Ensure workmode.md is configured.")
                .size(12)
                .color(CRABOT_TEXT_MUTED),
        )
        .padding(16)
        .center_x(Length::Fill)
        .into()
    } else {
        let cards: Vec<Element<'a, SettingsEvent>> = work_modes
            .iter()
            .enumerate()
            .map(|(i, mode)| {
                let mode_key = mode.name.to_lowercase();
                let recipes: &[String] = state
                    .working_prompt_recipes
                    .get(&mode_key)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let expanded = state.expanded_recipe_mode == Some(i);
                mode_card(i, mode.name.to_string(), mode_key, recipes, expanded)
            })
            .collect();
        column(cards).spacing(8).into()
    };

    let action_row = super::save_action_row(
        state,
        SettingsTab::PromptRecipes,
        SettingsEvent::SavePromptRecipes,
    );

    column![header, body, action_row].spacing(12).into()
}

// ── Mode card ─────────────────────────────────────────────────────

/// A collapsible card: header with the work mode name and recipe count;
/// the edit form appears below when expanded.
fn mode_card<'a>(
    index: usize,
    display_name: String,
    mode_key: String,
    recipes: &'a [String],
    expanded: bool,
) -> Element<'a, SettingsEvent> {
    let arrow = if expanded { "▼" } else { "⯈" };
    let count = recipes.len();
    let summary = format!("{count} recipe{}", if count == 1 { "" } else { "s" });

    let title = iced::widget::mouse_area(
        container(
            row![
                text(arrow).size(10).color(CRABOT_TEXT_MUTED).width(14),
                text(display_name).size(13).font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..iced::Font::DEFAULT
                }),
                text(summary).size(11).color(CRABOT_TEXT_MUTED),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill),
    )
    .on_press(SettingsEvent::ToggleRecipeMode(index));

    let header_row = row![title].spacing(4).align_y(Alignment::Center);

    container(if expanded {
        column![header_row, card_rule(), recipe_list(&mode_key, recipes)].spacing(10)
    } else {
        column![header_row]
    })
    .padding([10, 12])
    .style(form_card_style)
    .width(Length::Fill)
    .into()
}

// ── Recipe list within an expanded mode ────────────────────────────

fn recipe_list<'a>(mode_key: &str, recipes: &'a [String]) -> Element<'a, SettingsEvent> {
    let mk = mode_key.to_string();

    if recipes.is_empty() {
        return column![
            text("No recipes for this mode. Click + Add Recipe to create one.")
                .size(12)
                .color(CRABOT_TEXT_MUTED),
            add_recipe_button(mk),
        ]
        .spacing(8)
        .into();
    }

    let items: Vec<Element<'a, SettingsEvent>> = recipes
        .iter()
        .enumerate()
        .map(|(i, recipe)| recipe_row(mk.clone(), i, recipe.as_str()))
        .collect();

    column![column(items).spacing(8), add_recipe_button(mk),]
        .spacing(8)
        .into()
}

fn add_recipe_button(mode_key: String) -> Element<'static, SettingsEvent> {
    button(text("+ Add Recipe").size(12))
        .padding([4, 10])
        .style(crate::views::styles::primary_button)
        .on_press(SettingsEvent::NewRecipe(mode_key))
        .into()
}

// ── Recipe row ────────────────────────────────────────────────────

fn recipe_row<'a>(mode_key: String, index: usize, recipe: &'a str) -> Element<'a, SettingsEvent> {
    let label_text = format!("Recipe {}", index + 1);
    let label = container(text(label_text).size(12).color(CRABOT_TEXT_MUTED))
        .width(Length::Fixed(70.0))
        .align_x(Alignment::End)
        .align_y(Alignment::Center);

    let mk_del = mode_key.clone();
    let mk_edit = mode_key;

    let delete = button(text("✕").size(11))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(SettingsEvent::DeleteRecipe(mk_del, index));

    let input: Element<'a, SettingsEvent> = text_input("Enter recipe prompt...", recipe)
        .on_input(move |v| SettingsEvent::EditRecipe(mk_edit.clone(), index, v))
        .width(Length::Fill)
        .padding(4)
        .size(13)
        .into();

    container(
        row![label, input, delete]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .into()
}
