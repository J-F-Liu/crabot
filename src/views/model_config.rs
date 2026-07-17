use super::theme::CRABOT_BORDER;
use crabot::model::{Model, ModelList};
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Length,
    border::Radius,
    mouse,
    widget::{column, mouse_area, pick_list, row, text, toggler},
};
use iced_aw::{
    style::{status::Status, tab_bar::Style as TabBarStyle},
    widget::tab_bar::{TabBar, TabLabel},
};

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
    SelectModelConfig(String),
    SelectProvider(String),
    SelectModel(String),
    ToggleThinking(bool),
    SelectThinkingLevel(String),
}

pub(crate) fn model_config_view<'a>(
    provided_models: &'a ModelList,
    providers: &'a [ProviderEntry],
    selected: &'a String,
) -> Element<'a, Event> {
    // ── Tab bar for model config switching ───────────────────────
    let tab_bar: Element<_> = {
        let names: Vec<String> = provided_models.models.keys().cloned().collect();
        let mut bar = TabBar::new(move |name: String| Event::SelectModelConfig(name))
            .tab_width(Length::Shrink)
            .text_size(13.0)
            .padding([0, 8])
            .style(|theme: &iced::Theme, status| TabBarStyle {
                tab_border_radius: Radius {
                    top_left: 6.0,
                    top_right: 6.0,
                    bottom_right: 0.0,
                    bottom_left: 0.0,
                },
                tab_label_background: match status {
                    Status::Active => Background::Color(theme.palette().primary),
                    Status::Hovered => {
                        Background::Color(theme.extended_palette().primary.weak.color)
                    }
                    _ => Background::Color(theme.extended_palette().background.weak.color),
                },
                text_color: match status {
                    Status::Active => Color::WHITE,
                    _ => theme.palette().text,
                },
                ..Default::default()
            });

        for name in names {
            bar = bar.push(name.clone(), TabLabel::Text(name));
        }
        bar = bar.set_active_tab(selected);
        bar.into()
    };

    let selected_config = provided_models.get_config(selected);
    let selected_entry: Option<&ProviderEntry> =
        selected_config.and_then(|cfg| providers.iter().find(|e| e.id == cfg.provider_id));
    let selected_provider =
        selected_config.and_then(|cfg| provided_models.providers.get(&cfg.provider_id));

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
        mouse_area(toggler(false).style(crate::views::primary_toggler))
            .interaction(mouse::Interaction::None)
            .into()
    };

    let thinking_row: Element<_> = if supported {
        let levels: &[String] = selected_model.map(|m| &*m.thinking_levels).unwrap_or(&[]);
        let selected_level = thinking_level.and_then(|i| levels.get(i));
        let level_picker: Element<_> = if levels.is_empty() {
            iced::widget::Space::new().width(Fill).height(30.0).into()
        } else {
            pick_list(levels, selected_level, Event::SelectThinkingLevel)
                .width(Fill)
                .into()
        };
        row![
            text("Thinking").size(14).width(60.0),
            toggle,
            text("Level").size(14),
            level_picker,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        row![
            text("Thinking").size(14).width(60.0),
            toggle,
            iced::widget::Space::new().width(Fill).height(30.0),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    };

    column![
        tab_bar,
        iced::widget::container(
            column![
                row![
                    pick_list(providers, selected_entry, |e| Event::SelectProvider(e.id))
                        .width(Fill),
                    pick_list(models, selected_model, |m| Event::SelectModel(m.id.clone())),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                thinking_row,
            ]
            .spacing(8)
        )
        .padding(8)
        .style(|_theme: &iced::Theme| iced::widget::container::Style {
            background: None,
            border: Border {
                color: CRABOT_BORDER,
                radius: Radius {
                    top_left: 0.0,
                    top_right: 0.0,
                    bottom_right: 4.0,
                    bottom_left: 4.0,
                },
                width: 2.0,
            },
            ..iced::widget::container::Style::default()
        }),
    ]
    .spacing(0)
    .into()
}

pub(crate) fn update(
    event: Event,
    provided_models: &mut ModelList,
    selected_model: &mut String,
) -> bool {
    match event {
        Event::SelectModelConfig(name) => {
            if name != *selected_model {
                *selected_model = name;
                return false; // selected_model is saved in settings.ron, don't trigger models.ron save
            }
        }
        Event::SelectProvider(id) => {
            let Some(p) = provided_models.providers.get(&id) else {
                return false;
            };
            let Some(m) = p.models.first() else {
                return false;
            };
            // prepare values to avoid borrowing m
            let model_id = m.id.clone();
            let thinking = m.thinking;
            let thinking_level = m.thinking_levels.first().cloned().unwrap_or_default();
            if let Some(cfg) = provided_models.get_config_mut(selected_model) {
                cfg.provider_id = id;
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
                    cfg.model_id = id;
                    cfg.thinking = thinking;
                    cfg.thinking_level = thinking_level;
                    return true;
                }
            }
        }
        Event::ToggleThinking(enabled) => {
            if let Some(cfg) = provided_models.get_config_mut(selected_model)
                && cfg.thinking != enabled
            {
                cfg.thinking = enabled;
                return true;
            }
        }
        Event::SelectThinkingLevel(level) => {
            if let Some(cfg) = provided_models.get_config_mut(selected_model)
                && cfg.thinking_level != level
            {
                cfg.thinking_level = level;
                return true;
            }
        }
    }
    false
}
