use iced::{
    Alignment, Element, Fill, mouse,
    widget::{column, mouse_area, pick_list, row, text, toggler},
};

use crate::Message;
use crate::model::{Model, ModelConfig, Provider};

pub(crate) fn model_config_view<'a>(
    providers: &'a [Provider],
    selected: &Option<ModelConfig>,
) -> Element<'a, Message> {
    let selected_provider = selected
        .as_ref()
        .and_then(|cfg| providers.iter().find(|p| p.id == cfg.provider_id));
    let models: &[Model] = selected_provider.map(|p| &*p.models).unwrap_or(&[]);
    let selected_model = selected_provider.and_then(|p| {
        selected
            .as_ref()
            .and_then(|cfg| p.models.iter().find(|m| m.id == cfg.model_id))
    });
    let supported = selected_model.is_some_and(|m| m.thinking);
    let thinking_enabled = selected.as_ref().map(|cfg| cfg.thinking).unwrap_or(false);
    let thinking_level = selected.as_ref().and_then(|cfg| {
        selected_model.and_then(|m| {
            m.thinking_levels
                .iter()
                .position(|l| *l == cfg.thinking_level)
        })
    });

    let toggle: Element<_> = if supported {
        toggler(thinking_enabled)
            .on_toggle(Message::ToggleThinking)
            .style(crate::views::primary_toggler)
            .into()
    } else {
        mouse_area(toggler(thinking_enabled).style(crate::views::primary_toggler))
            .interaction(mouse::Interaction::None)
            .into()
    };

    let thinking_row: Element<_> = if supported {
        let levels: &[String] = selected_model.map(|m| &*m.thinking_levels).unwrap_or(&[]);
        let selected_level = thinking_level.and_then(|i| levels.get(i));
        row![
            text("Thinking").size(14).width(60.0),
            toggle,
            text("Level").size(14),
            pick_list(levels, selected_level, Message::SelectThinkingLevel).width(Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        row![text("Thinking").size(14).width(60.0), toggle,]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
    };

    column![
        row![
            text("Provider").size(14).width(60.0),
            pick_list(providers, selected_provider, |p| Message::SelectProvider(
                p.id
            ))
            .width(Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            text("Model").size(14).width(60.0),
            pick_list(models, selected_model, |m| Message::SelectModel(m.id)).width(Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        thinking_row,
    ]
    .spacing(8)
    .into()
}
