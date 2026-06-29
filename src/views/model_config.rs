use iced::{
    Alignment, Element, Fill, mouse,
    widget::{column, mouse_area, pick_list, row, text, toggler},
};

use crate::model::{Model, ModelList};

/// Pick-list entry pairing a provider id with its display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderEntry {
    pub id: String,
    pub name: String,
}

impl std::fmt::Display for ProviderEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Events emitted by the model config UI.
#[derive(Debug, Clone)]
pub(crate) enum Event {
    SelectProvider(String),
    SelectModel(String),
    ToggleThinking(bool),
    SelectThinkingLevel(String),
}

pub(crate) fn model_config_view<'a>(
    provided_models: &'a ModelList,
    provider_entries: &'a [ProviderEntry],
    selected: &'a str,
) -> Element<'a, Event> {
    let selected_config = provided_models.get_config(selected);
    let selected_entry: Option<&ProviderEntry> = selected_config
        .as_ref()
        .and_then(|cfg| provider_entries.iter().find(|e| e.id == cfg.provider_id));
    let selected_provider = selected_config
        .as_ref()
        .and_then(|cfg| provided_models.providers.get(&cfg.provider_id));

    let models: Vec<&Model> = selected_provider
        .map(|p| p.models.iter().collect())
        .unwrap_or_default();
    let selected_model = selected_config.and_then(|cfg| provided_models.get_model(cfg));

    let supported = selected_model.is_some_and(|m| m.thinking);
    let thinking_enabled = selected_config.is_some_and(|cfg| cfg.thinking);
    let thinking_level = selected_config.and_then(|cfg| {
        selected_model.and_then(|m| {
            m.thinking_levels
                .iter()
                .position(|l| *l == cfg.thinking_level)
        })
    });

    let toggle: Element<_> = if supported {
        toggler(thinking_enabled)
            .on_toggle(Event::ToggleThinking)
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
            pick_list(levels, selected_level, Event::SelectThinkingLevel).width(Fill),
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
            pick_list(provider_entries, selected_entry, move |e| {
                Event::SelectProvider(e.id.clone())
            })
            .width(Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            text("Model").size(14).width(60.0),
            pick_list(models, selected_model, |m| Event::SelectModel(m.id.clone())).width(Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        thinking_row,
    ]
    .spacing(8)
    .into()
}

pub(crate) fn update(event: &Event, provided_models: &mut ModelList, selected_model: &str) -> bool {
    match event {
        Event::SelectProvider(id) => {
            let Some(p) = provided_models.providers.get(id) else {
                return false;
            };
            let Some(m) = p.models.first() else {
                return false;
            };
            // prepair values to avoid borrowing m
            let model_id = m.id.clone();
            let thinking = m.thinking;
            let thinking_level = m.thinking_levels.first().cloned().unwrap_or_default();
            if let Some(cfg) = provided_models.get_config_mut(selected_model) {
                cfg.provider_id = id.clone();
                cfg.model_id = model_id;
                cfg.thinking = thinking;
                cfg.thinking_level = thinking_level;
                return true;
            }
        }
        Event::SelectModel(id) => {
            // Look up the model in the current provider to get thinking defaults.
            if let Some(provider) = provided_models.get_provider(selected_model)
                && let Some(m) = provider.models.iter().find(|m| m.id == *id)
            {
                let thinking = m.thinking;
                let thinking_level = m.thinking_levels.first().cloned().unwrap_or_default();
                if let Some(cfg) = provided_models.get_config_mut(selected_model) {
                    cfg.model_id = id.clone();
                    cfg.thinking = thinking;
                    cfg.thinking_level = thinking_level;
                    return true;
                }
            }
        }
        Event::ToggleThinking(enabled) => {
            if let Some(cfg) = provided_models.get_config_mut(selected_model)
                && cfg.thinking != *enabled
            {
                cfg.thinking = *enabled;
                return true;
            }
        }
        Event::SelectThinkingLevel(level) => {
            if let Some(cfg) = provided_models.get_config_mut(selected_model)
                && cfg.thinking_level != *level
            {
                cfg.thinking_level = level.clone();
                return true;
            }
        }
    }
    false
}
