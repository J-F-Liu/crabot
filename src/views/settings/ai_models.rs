use super::{
    NEW_LABEL_INPUT_ID, NEW_PROVIDER_NAME_INPUT_ID, SettingsEvent, SettingsState, SettingsTab,
    delete_button_style, field_row, form_card_style, section_header,
};
use crate::views::theme::{
    CRABOT_DANGER, CRABOT_PRIMARY, color_border, color_card, color_muted, color_surface,
    color_text_strong,
};
use crabot::model::{Model, currency_symbol};
use crabot::model_database::ModelDatabase;
use iced::{
    Alignment, Border, Color, Element, Length, mouse,
    widget::{
        Row, button, checkbox, column, container, mouse_area, pick_list, row, scrollable, text,
        text_input,
    },
};

// ── Events ──────────────────────────────────────────────────────────

/// Events for the AI Models tab: provider, model, and label editing.
#[derive(Debug, Clone)]
pub(crate) enum ModelsEvent {
    // Provider actions
    SelectProvider(String),
    EditProviderName(String),
    EditProviderBaseUrl(String),
    EditProviderApiType(String),
    EditProviderAuth(String),
    EditProviderApiKey(String),
    ToggleProviderStrictMode(bool),
    NewProvider,
    DeleteProvider(String),
    CancelNewProvider,
    ModelsFetched(String, Result<Vec<String>, String>),
    /// Manually refresh the available-model list for the current provider.
    RefreshModels,
    /// Raw text of the model search box.
    EditModelSearch(String),
    /// Apply the search text as the model-list filter.
    ApplyModelFilter,
    ToggleModel(String, bool),
    SelectModelDetail(String),
    /// Edit one parameter of the currently-selected checked model.
    EditModelParam(ModelParam),
    /// Choose which pricing offer to display / use when adding a model.
    SelectOfferSource(String),
    // Label actions
    DeleteLabel(String),
    /// Show the blank new-label capsule and focus its input.
    StartAddLabel,
    NewLabelName(String),
    /// Confirm the new-label input (Enter or focus loss).
    AddLabel,
    /// Begin dragging the label capsule at the given index.
    LabelDragStart(usize),
    /// Cursor entered the capsule at the given index mid-drag.
    LabelDragEnter(usize),
    /// End the capsule drag, saving if the order changed.
    LabelDragEnd,
    /// Result of a focus check on the new-label input; `false` means the
    /// input lost focus and the pending label should be confirmed.
    LabelInputFocus(bool),
    /// Persist the working model list to disk.
    SaveModels,
}

// ── Model parameter editor ───────────────────────────────────────

/// One editable parameter of a checked model.
#[derive(Debug, Clone)]
pub(crate) enum ModelParam {
    Name(String),
    Thinking(bool),
    /// Comma-separated level names.
    ThinkingLevels(String),
    /// Comma-separated input modes ("text", "image", …).
    Input(String),
    ContextWindow(String),
    MaxTokens(String),
    CostInput(String),
    CostOutput(String),
    CostCacheRead(String),
    CostCacheWrite(String),
    Currency(String),
    DoubleOnPeakHour(bool),
}

/// Raw-text drafts backing the parameter editor — keeps partially-typed
/// numbers visible instead of snapping back to the last valid value.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModelEditDraft {
    pub(crate) name: String,
    pub(crate) thinking_levels: String,
    pub(crate) input: String,
    pub(crate) context_window: String,
    pub(crate) max_tokens: String,
    pub(crate) cost_input: String,
    pub(crate) cost_output: String,
    pub(crate) cost_cache_read: String,
    pub(crate) cost_cache_write: String,
    pub(crate) currency: String,
}

impl ModelEditDraft {
    /// Seed raw-text drafts from a model's current parameters.
    fn from_model(m: &Model) -> Self {
        Self {
            name: m.name.clone(),
            thinking_levels: m.thinking_levels.join(", "),
            input: m.input.join(", "),
            context_window: m.context_window.to_string(),
            max_tokens: m.max_tokens.to_string(),
            cost_input: m.cost.input.to_string(),
            cost_output: m.cost.output.to_string(),
            cost_cache_read: m.cost.cache_read.to_string(),
            cost_cache_write: m.cost.cache_write.to_string(),
            currency: m.cost.currency.to_string(),
        }
    }

    /// Record the raw text of an edited field (flags have no draft).
    fn record(&mut self, param: &ModelParam) {
        let (field, value) = match param {
            ModelParam::Name(v) => (&mut self.name, v),
            ModelParam::ThinkingLevels(v) => (&mut self.thinking_levels, v),
            ModelParam::Input(v) => (&mut self.input, v),
            ModelParam::ContextWindow(v) => (&mut self.context_window, v),
            ModelParam::MaxTokens(v) => (&mut self.max_tokens, v),
            ModelParam::CostInput(v) => (&mut self.cost_input, v),
            ModelParam::CostOutput(v) => (&mut self.cost_output, v),
            ModelParam::CostCacheRead(v) => (&mut self.cost_cache_read, v),
            ModelParam::CostCacheWrite(v) => (&mut self.cost_cache_write, v),
            ModelParam::Currency(v) => (&mut self.currency, v),
            ModelParam::Thinking(_) | ModelParam::DoubleOnPeakHour(_) => return,
        };
        *field = value.clone();
    }
}

/// Split a comma-separated draft string into trimmed, non-empty entries.
fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

// Models of the currently-edited provider.
fn provider_models(state: &SettingsState) -> Option<&[Model]> {
    state
        .working_models
        .providers
        .get(&state.selected_provider_id)
        .map(|p| p.models.as_slice())
}

/// Rows visible in the 200px model list; longer lists show the search box.
const MODEL_LIST_SCROLL_ROWS: usize = 8;

/// IDs shown in the model list: checked models first (configured order),
/// then remaining fetched IDs.
fn display_model_ids(state: &SettingsState) -> Vec<&str> {
    let provider_ids: Vec<&str> = provider_models(state)
        .map(|ms| ms.iter().map(|m| m.id.as_str()).collect())
        .unwrap_or_default();
    let fetched = &state.available_model_ids;
    if fetched.is_empty() {
        return provider_ids;
    }
    // Checked models always stay visible — they may not be offered by the
    // /models endpoint (custom or stale IDs) but are still in the config.
    let tail: Vec<&str> = fetched
        .iter()
        .filter(|id| !provider_ids.contains(&id.as_str()))
        .map(String::as_str)
        .collect();
    let mut ids = provider_ids;
    ids.extend(tail);
    ids
}

/// True when the model ID or its database name matches `query` (case-insensitive).
fn model_matches_filter(id: &str, query: &str, db: &ModelDatabase) -> bool {
    id.to_lowercase().contains(query)
        || db
            .get(id)
            .is_some_and(|m| m.name.to_lowercase().contains(query))
}

/// Parse a numeric field: empty means unset (default), invalid keeps `current`.
fn parse_field<T: Default + std::str::FromStr>(raw: &str, current: T) -> T {
    let raw = raw.trim();
    if raw.is_empty() {
        T::default()
    } else {
        raw.parse().unwrap_or(current)
    }
}

/// Apply one [`ModelParam`] to a model.
fn apply_to_model(model: &mut Model, param: &ModelParam) {
    use ModelParam::*;
    match param {
        Name(v) => model.name = v.clone(),
        Thinking(v) => model.thinking = *v,
        ThinkingLevels(v) => model.thinking_levels = split_csv(v),
        Input(v) => model.input = split_csv(v),
        ContextWindow(v) => model.context_window = parse_field(v, model.context_window),
        MaxTokens(v) => model.max_tokens = parse_field(v, model.max_tokens),
        CostInput(v) => model.cost.input = parse_field(v, model.cost.input),
        CostOutput(v) => model.cost.output = parse_field(v, model.cost.output),
        CostCacheRead(v) => model.cost.cache_read = parse_field(v, model.cost.cache_read),
        CostCacheWrite(v) => model.cost.cache_write = parse_field(v, model.cost.cache_write),
        DoubleOnPeakHour(v) => model.cost.double_on_peak_hour = *v,
        // Keep the current currency if the new value doesn't fit.
        Currency(v) => {
            model.cost.currency = v.trim().try_into().unwrap_or(model.cost.currency);
        }
    }
}

/// Apply one [`ModelParam`] to the selected checked model, keeping the
/// raw-text drafts in sync so partially-typed input stays visible.
fn apply_model_param(state: &mut SettingsState, param: ModelParam) {
    let Some(id) = state.selected_model_id.clone() else {
        return;
    };
    let Some(provider) = state
        .working_models
        .providers
        .get_mut(&state.selected_provider_id)
    else {
        return;
    };
    if let Some(model) = provider.models.iter_mut().find(|m| m.id == id) {
        apply_to_model(model, &param);
        // Keep the draft in sync only after a successful apply.
        if let Some(d) = state.model_edit.as_mut() {
            d.record(&param);
        }
    }
}

// ── Provider entry for pick list ──────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
struct ProviderPickEntry {
    id: String,
    name: String,
}

impl std::fmt::Display for ProviderPickEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

// ── Provider section ────────────────────────────────────────────────

pub(super) fn provider_tab_view<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let entries: Vec<ProviderPickEntry> = state
        .working_models
        .providers
        .iter()
        .map(|(id, p)| ProviderPickEntry {
            id: id.clone(),
            name: p.name.clone(),
        })
        .collect();

    let selected = entries
        .iter()
        .find(|e| e.id == state.selected_provider_id)
        .cloned();

    let picker = pick_list(entries, selected, |e: ProviderPickEntry| {
        SettingsEvent::Models(ModelsEvent::SelectProvider(e.id))
    })
    .width(Length::Fill);

    let is_editing = !state.selected_provider_id.is_empty() || state.is_new_provider;

    let form: Element<_> = if is_editing {
        const API_TYPES: &[&str] = &[
            "openai",
            "openai_resp",
            "anthropic",
            "gemini",
            "groq",
            "ollama",
            "deepseek",
        ];
        let selected_api_type = API_TYPES
            .iter()
            .find(|&&t| t == state.provider_api_type)
            .copied();

        const AUTH_TYPES: &[&str] = &["apiKey", "none"];
        let selected_auth = AUTH_TYPES
            .iter()
            .find(|&&t| t == state.provider_auth)
            .copied();

        let form_body = column![
            field_row(
                "Name",
                &state.provider_name,
                "Provider name",
                false,
                Some(NEW_PROVIDER_NAME_INPUT_ID),
                None,
                move |v| SettingsEvent::Models(ModelsEvent::EditProviderName(v)),
            ),
            field_row(
                "Base URL",
                &state.provider_base_url,
                "Base URL of the provider, press Enter to fetch model list",
                false,
                None,
                Some(SettingsEvent::Models(ModelsEvent::RefreshModels)),
                move |v| SettingsEvent::Models(ModelsEvent::EditProviderBaseUrl(v)),
            ),
            {
                let label_col = |label: &'static str| {
                    container(text(label).size(14))
                        .width(90)
                        .align_x(Alignment::End)
                };
                row![
                    label_col("API Type"),
                    pick_list(API_TYPES, selected_api_type, |v| {
                        SettingsEvent::Models(ModelsEvent::EditProviderApiType(v.to_string()))
                    })
                    .width(Length::Fill),
                    label_col("Auth Type"),
                    pick_list(AUTH_TYPES, selected_auth, |v| {
                        SettingsEvent::Models(ModelsEvent::EditProviderAuth(v.to_string()))
                    })
                    .width(Length::Fill),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
            },
            field_row(
                "API Key",
                &state.provider_api_key,
                "API Key or its enviroment variable name",
                false,
                None,
                None,
                move |v| SettingsEvent::Models(ModelsEvent::EditProviderApiKey(v)),
            ),
            {
                // Search box appears once the model list is long enough to scroll,
                // and stays visible while a filter is applied.
                let search = (!state.fetching_models
                    && (!state.model_filter.is_empty()
                        || state.available_model_ids.len() > MODEL_LIST_SCROLL_ROWS))
                    .then(|| {
                        text_input("Search models…", &state.model_search)
                            .on_input(|v| SettingsEvent::Models(ModelsEvent::EditModelSearch(v)))
                            .on_submit(SettingsEvent::Models(ModelsEvent::ApplyModelFilter))
                            .size(12)
                            .padding([2, 6])
                            .width(Length::Fixed(160.0))
                            .into()
                    });
                checkbox_row(
                    "Strict Mode",
                    state.provider_strict_mode,
                    |v| SettingsEvent::Models(ModelsEvent::ToggleProviderStrictMode(v)),
                    search,
                )
            },
            models_section_view(state),
        ]
        .spacing(10);

        container(form_body)
            .padding(16)
            .style(form_card_style)
            .width(Length::Fill)
            .into()
    } else {
        column![
            text("Select a provider to edit, or create a new one.")
                .size(13)
                .color(Color::from_rgb8(0x66, 0x66, 0x66)),
            iced::widget::Space::new().height(Length::Fill),
            button(text("New"))
                .style(crate::views::styles::primary_button)
                .on_press(SettingsEvent::Models(ModelsEvent::NewProvider)),
        ]
        .spacing(12)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into()
    };

    let action_button: Element<'_, SettingsEvent> = if state.is_new_provider {
        button(text("Cancel"))
            .style(crate::views::styles::secondary_button)
            .on_press(SettingsEvent::Models(ModelsEvent::CancelNewProvider))
            .into()
    } else if !state.selected_provider_id.is_empty() {
        button(text("Delete"))
            .style(|theme: &iced::Theme, status| {
                let mut s = crate::views::styles::secondary_button(theme, status);
                s.text_color = Color::from_rgb8(0xE5, 0x4D, 0x4D);
                s
            })
            .on_press(SettingsEvent::Models(ModelsEvent::DeleteProvider(
                state.selected_provider_id.clone(),
            )))
            .into()
    } else {
        iced::widget::Space::new().width(0).into()
    };

    column![
        row![
            section_header("Model Providers"),
            picker,
            row![
                button(text("New"))
                    .style(crate::views::styles::primary_button)
                    .on_press(SettingsEvent::Models(ModelsEvent::NewProvider)),
                action_button,
            ]
            .spacing(8),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        form,
    ]
    .spacing(12)
    .into()
}

// ── Label section ──────────────────────────────────────────────────

/// Labels are shown as draggable capsules on a single (scrollable) row.
/// The trailing "+" capsule opens a blank input capsule; the new label is
/// confirmed with Enter or when the input loses focus.
pub(super) fn label_tab_view<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = section_header("Model Labels");

    let dragging = state.dragging_label();

    let mut chips: Vec<Element<'a, SettingsEvent>> = state
        .working_models
        .models
        .keys()
        .enumerate()
        .map(|(i, name)| {
            let chip = container(
                row![
                    text(name).size(13),
                    button(text("✕").size(10))
                        .padding(0)
                        .style(delete_button_style)
                        .on_press(SettingsEvent::Models(ModelsEvent::DeleteLabel(
                            name.clone(),
                        ))),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([5, 12])
            .style(chip_style(dragging == Some(i)));

            mouse_area(chip)
                .on_press(SettingsEvent::Models(ModelsEvent::LabelDragStart(i)))
                .on_enter(SettingsEvent::Models(ModelsEvent::LabelDragEnter(i)))
                .interaction(if dragging.is_some() {
                    mouse::Interaction::Grabbing
                } else {
                    mouse::Interaction::Grab
                })
                .into()
        })
        .collect();

    if chips.is_empty() && !state.is_adding_label() {
        chips.push(
            text("No labels yet. Click + to add one.")
                .size(13)
                .color(color_muted())
                .into(),
        );
    }

    if state.is_adding_label() {
        chips.push(
            text_input("Label name", &state.new_label_name)
                .id(NEW_LABEL_INPUT_ID)
                .on_input(move |v| SettingsEvent::Models(ModelsEvent::NewLabelName(v)))
                .on_submit(SettingsEvent::Models(ModelsEvent::AddLabel))
                .size(13)
                .padding([5, 12])
                .width(140)
                .style(chip_input_style)
                .into(),
        );
    } else {
        chips.push(
            mouse_area(
                container(text("+").size(13).color(CRABOT_PRIMARY))
                    .padding([5, 12])
                    .style(add_chip_style),
            )
            .on_press(SettingsEvent::Models(ModelsEvent::StartAddLabel))
            .into(),
        );
    }

    let labels_section = scrollable(row(chips).spacing(8).align_y(Alignment::Center))
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default(),
        ))
        .width(Length::Fill);

    let hint = text("Drag labels to reorder · Click + to add a new label")
        .size(12)
        .color(color_muted());
    column![
        header,
        container(column![labels_section, super::section_rule(), hint].spacing(10))
            .padding(16)
            .style(form_card_style)
            .width(Length::Fill)
    ]
    .spacing(10)
    .into()
}

// ── Capsule styles ────────────────────────────────────────────────

/// Filled capsule for an existing label; the border highlights while dragged.
fn chip_style(dragged: bool) -> impl Fn(&iced::Theme) -> container::Style {
    move |_: &iced::Theme| container::Style {
        background: Some(color_surface().into()),
        border: Border::default().rounded(999).width(1).color(if dragged {
            CRABOT_PRIMARY
        } else {
            color_border()
        }),
        ..container::Style::default()
    }
}

/// Outlined "+" capsule that starts a new label.
fn add_chip_style(_: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(color_card().into()),
        border: Border::default()
            .rounded(999)
            .width(1)
            .color(CRABOT_PRIMARY),
        ..container::Style::default()
    }
}

/// Capsule-shaped style for the new-label text input.
fn chip_input_style(_theme: &iced::Theme, _status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: color_card().into(),
        border: Border::default()
            .rounded(999)
            .width(1)
            .color(CRABOT_PRIMARY),
        icon: color_muted(),
        placeholder: color_muted(),
        value: color_text_strong(),
        selection: CRABOT_PRIMARY.scale_alpha(0.3),
    }
}

// ── Form helpers ──────────────────────────────────────────────────

fn checkbox_row<'a>(
    label: &'static str,
    checked: bool,
    on_toggle: impl Fn(bool) -> SettingsEvent + 'a,
    trailing: Option<Element<'a, SettingsEvent>>,
) -> Element<'a, SettingsEvent> {
    let label_col = container(text(label).size(14))
        .width(90)
        .align_x(Alignment::End);
    let cb = checkbox(checked)
        .label("")
        .on_toggle(on_toggle)
        .style(crate::views::primary_checkbox);
    let mut r = row![label_col, cb].spacing(10).align_y(Alignment::Center);
    if let Some(t) = trailing {
        r = r
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(t);
    }
    r.into()
}

/// Renders the models section with a table of checkboxes and model IDs.
fn models_section_view<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = container(text("Models").size(14))
        .width(90)
        .align_x(Alignment::End);

    // All IDs, narrowed by the applied filter when set.
    let all_ids = display_model_ids(state);
    let display_ids: Vec<&str> = all_ids
        .iter()
        .copied()
        .filter(|id| {
            state.model_filter.is_empty()
                || model_matches_filter(id, &state.model_filter, &state.model_db)
        })
        .collect();

    // Body: status or table + details
    let body: Element<'_, SettingsEvent> = if state.fetching_models {
        text("Loading models…").size(12).color(color_muted()).into()
    } else if all_ids.is_empty() {
        let fetch = button(text("Fetch Models").size(12))
            .style(crate::views::styles::primary_button)
            .on_press_maybe(
                (!state.provider_base_url.trim().is_empty())
                    .then_some(SettingsEvent::Models(ModelsEvent::RefreshModels)),
            );
        if let Some(err) = &state.models_fetch_error {
            column![fetch, text(err).size(11).color(CRABOT_DANGER),]
                .spacing(4)
                .into()
        } else {
            fetch.into()
        }
    } else if display_ids.is_empty() {
        container(
            text("No models match the search filter.")
                .size(11)
                .color(color_muted()),
        )
        .padding(8)
        .style(form_card_style)
        .into()
    } else {
        // ── Table column ──────────────────────────────────────────
        let model_rows: Vec<Element<'_, SettingsEvent>> = display_ids
            .iter()
            .map(|&id| {
                let checked =
                    provider_models(state).is_some_and(|ms| ms.iter().any(|m| m.id == id));
                let is_selected = state.selected_model_id.as_deref() == Some(id);

                // Disable checkbox when the new provider hasn't been named yet.
                let can_toggle = !(state.is_new_provider && state.provider_name.trim().is_empty());
                let mut cb = checkbox(checked).style(crate::views::primary_checkbox);
                if can_toggle {
                    cb = cb.on_toggle(move |v| {
                        SettingsEvent::Models(ModelsEvent::ToggleModel(id.to_string(), v))
                    });
                }

                let id_cell = mouse_area(
                    container(container(text(id.to_string()).size(12)).padding([2, 4])).style(
                        move |_: &iced::Theme| {
                            if is_selected {
                                container::Style {
                                    background: Some(
                                        Color::from_rgb8(0x3B, 0x82, 0xF6).scale_alpha(0.12).into(),
                                    ),
                                    border: Border::default()
                                        .rounded(4)
                                        .width(1)
                                        .color(Color::from_rgb8(0x3B, 0x82, 0xF6).scale_alpha(0.3)),
                                    ..container::Style::default()
                                }
                            } else {
                                container::Style::default()
                            }
                        },
                    ),
                )
                .on_press(SettingsEvent::Models(ModelsEvent::SelectModelDetail(
                    id.to_string(),
                )));

                container(row![cb, id_cell].spacing(8).align_y(Alignment::Center))
                    .padding(1)
                    .into()
            })
            .collect();

        let table = container(
            scrollable(column(model_rows).spacing(1))
                .height(Length::Fixed(200.0))
                .width(Length::FillPortion(1)),
        )
        .padding(2)
        .style(|_: &iced::Theme| container::Style {
            border: Border::default().rounded(4).width(1).color(color_border()),
            ..container::Style::default()
        });

        // ── Details panel ────────────────────────────────────────
        let details: Element<'_, SettingsEvent> =
            if let Some(selected_id) = &state.selected_model_id {
                if let Some(model) =
                    provider_models(state).and_then(|ms| ms.iter().find(|m| &m.id == selected_id))
                {
                    // Checked model: show the editable parameter form.
                    match state.model_edit.as_ref() {
                        Some(draft) => model_edit_panel(model, draft),
                        None => readonly_model_detail(model),
                    }
                } else if let Some(details) = state.model_db.get(selected_id) {
                    // Pick the active offer: user-selected source, or first.
                    let active_cost = state
                        .selected_offer_source
                        .as_deref()
                        .and_then(|src| details.offers.iter().find(|o| o.source == src))
                        .unwrap_or_else(|| details.offers.first().unwrap_or(&details.cost));

                    let header = base_header(&details.name, details.thinking, &[]);

                    let detail = model_detail_panel(
                        active_cost,
                        &details.input,
                        details.context_window,
                        details.max_tokens,
                        header,
                    );

                    // Show offer-source picker when multiple offers exist.
                    if details.offers.len() > 1 {
                        let sources: Vec<String> =
                            details.offers.iter().map(|o| o.source.clone()).collect();
                        let selected_source = state
                            .selected_offer_source
                            .clone()
                            .unwrap_or_else(|| active_cost.source.clone());
                        let picker = pick_list(sources, Some(selected_source), |src| {
                            SettingsEvent::Models(ModelsEvent::SelectOfferSource(src))
                        })
                        .text_size(12);
                        column![
                            container(
                                row![
                                    text("Offer").size(12).color(color_muted()).width(60),
                                    picker.width(Length::Fill),
                                ]
                                .spacing(10)
                                .align_y(Alignment::Center),
                            )
                            .padding([4, 0]),
                            detail,
                        ]
                        .spacing(4)
                        .into()
                    } else {
                        detail
                    }
                } else {
                    container(
                        text("Check the box to add this model,\nthen save to see parameters.")
                            .size(11)
                            .color(color_muted()),
                    )
                    .padding(8)
                    .style(form_card_style)
                    .width(Length::FillPortion(1))
                    .into()
                }
            } else {
                container(
                    text("Click a model ID to see details.")
                        .size(11)
                        .color(color_muted()),
                )
                .padding(8)
                .style(form_card_style)
                .width(Length::FillPortion(1))
                .into()
            };
        row![table, details].spacing(10).into()
    };
    row![header, body]
        .spacing(10)
        .height(Length::Fixed(200.0))
        .into()
}

/// Renders the common lower half of the model detail panel.
fn model_detail_panel<'a>(
    cost: &crabot::model::Cost,
    input: &[String],
    context_window: u32,
    max_tokens: u32,
    header: Vec<Element<'a, SettingsEvent>>,
) -> Element<'a, SettingsEvent> {
    let sym = currency_symbol(&cost.currency);
    let ctx = if context_window > 0 {
        context_window.to_string()
    } else {
        "—".into()
    };
    let max_tok = if max_tokens > 0 {
        max_tokens.to_string()
    } else {
        "—".into()
    };

    let mut rows = header;
    if !input.is_empty() {
        rows.push(detail_row("Input Modes", input.join(", ")));
    }
    rows.push(detail_row("Context", ctx));
    rows.push(detail_row("Max Tokens", max_tok));
    rows.push(detail_row("Cost (in)", format!("{sym}{:.4}/M", cost.input)));
    rows.push(detail_row(
        "Cost (out)",
        format!("{sym}{:.4}/M", cost.output),
    ));
    if cost.cache_read > 0.0 || cost.cache_write > 0.0 {
        rows.push(detail_row(
            "Cache read",
            format!("{sym}{:.4}/M", cost.cache_read),
        ));
        rows.push(detail_row(
            "Cache write",
            format!("{sym}{:.4}/M", cost.cache_write),
        ));
    }

    container(column(rows).spacing(2))
        .padding(8)
        .style(form_card_style)
        .width(Length::FillPortion(2))
        .into()
}

/// Shared Name/Thinking(/Levels) rows for model detail panels.
fn base_header(
    name: &str,
    thinking: bool,
    levels: &[String],
) -> Vec<Element<'static, SettingsEvent>> {
    let mut header = vec![
        detail_row(
            "Name",
            if name.is_empty() {
                "—".into()
            } else {
                name.to_string()
            },
        ),
        detail_row(
            "Thinking",
            if thinking { "yes".into() } else { "no".into() },
        ),
    ];
    if !levels.is_empty() {
        header.push(detail_row("Think Levels", levels.join(", ")));
    }
    header
}

/// Read-only fallback for a checked model whose editor drafts aren't seeded.
fn readonly_model_detail(model: &Model) -> Element<'static, SettingsEvent> {
    let header = base_header(&model.name, model.thinking, &model.thinking_levels);
    model_detail_panel(
        &model.cost,
        &model.input,
        model.context_window,
        model.max_tokens,
        header,
    )
}

/// Editable parameter form shown on the right for a checked model.
fn model_edit_panel<'a>(model: &'a Model, d: &'a ModelEditDraft) -> Element<'a, SettingsEvent> {
    // Right-aligned muted tag of fixed width (row labels and sub-labels).
    let tag = |t: &'static str, w: f32| {
        container(text(t).size(11).color(color_muted()))
            .width(Length::Fixed(w))
            .align_x(Alignment::End)
    };
    let input = |val: &'a str, ph: &'static str, w: Length, mk: fn(String) -> ModelParam| {
        text_input(ph, val)
            .on_input(move |v| SettingsEvent::Models(ModelsEvent::EditModelParam(mk(v))))
            .size(11)
            .padding([2, 4])
            .width(w)
    };
    let cb = |checked: bool, mk: fn(bool) -> ModelParam| {
        checkbox(checked)
            .label("")
            .on_toggle(move |v| SettingsEvent::Models(ModelsEvent::EditModelParam(mk(v))))
            .style(crate::views::primary_checkbox)
    };
    // One form row: leading label + widget body.
    let form_row = |label: &'static str, body: Row<'a, SettingsEvent>| -> Row<'a, SettingsEvent> {
        row![tag(label, 74.0)]
            .push(body.spacing(6).align_y(Alignment::Center))
            .spacing(6)
            .align_y(Alignment::Center)
    };
    let num = |val: &'a str, mk: fn(String) -> ModelParam| input(val, "0", Length::Fixed(72.0), mk);

    // Known currencies first; keep any custom value already on the model.
    let mut currencies: Vec<String> = ["USD", "CNY", "EUR", "GBP"]
        .into_iter()
        .map(String::from)
        .collect();
    if !d.currency.is_empty() && !currencies.contains(&d.currency) {
        currencies.push(d.currency.clone());
    }
    let selected_currency = currencies.iter().find(|c| **c == d.currency).cloned();

    let form = column![
        form_row(
            "Name",
            row![input(
                &d.name,
                "Display name",
                Length::Fill,
                ModelParam::Name
            )],
        ),
        form_row(
            "Thinking",
            row![
                cb(model.thinking, ModelParam::Thinking),
                text("Levels").size(11).color(color_muted()),
                input(
                    &d.thinking_levels,
                    "comma separated",
                    Length::Fill,
                    ModelParam::ThinkingLevels
                ),
            ],
        ),
        form_row(
            "Input Modes",
            row![input(
                &d.input,
                "comma separated",
                Length::Fill,
                ModelParam::Input
            )],
        ),
        form_row(
            "Context",
            row![
                num(&d.context_window, ModelParam::ContextWindow),
                text("Max Tokens").size(11).color(color_muted()),
                num(&d.max_tokens, ModelParam::MaxTokens),
            ],
        ),
        form_row(
            "Cost /M",
            row![
                tag("Input", 46.0),
                num(&d.cost_input, ModelParam::CostInput),
                tag("Output", 46.0),
                num(&d.cost_output, ModelParam::CostOutput),
            ],
        ),
        form_row(
            "Cache /M",
            row![
                tag("Read", 46.0),
                num(&d.cost_cache_read, ModelParam::CostCacheRead),
                tag("Write", 46.0),
                num(&d.cost_cache_write, ModelParam::CostCacheWrite),
            ],
        ),
        form_row(
            "Currency",
            row![
                pick_list(currencies, selected_currency, |c: String| {
                    SettingsEvent::Models(ModelsEvent::EditModelParam(ModelParam::Currency(c)))
                })
                .text_size(11)
                .padding([2, 4])
                .width(Length::Fixed(90.0)),
                text("Peak ×2").size(11).color(color_muted()),
                cb(model.cost.double_on_peak_hour, ModelParam::DoubleOnPeakHour),
            ],
        ),
    ]
    .spacing(6);

    container(scrollable(form).width(Length::Fill).height(Length::Fill))
        .padding(8)
        .style(form_card_style)
        .width(Length::FillPortion(2))
        .into()
}

/// Single label–value row for the model detail panel.
fn detail_row(label: &'static str, value: String) -> Element<'static, SettingsEvent> {
    row![
        text(label).size(11).color(color_muted()).width(70),
        text(value).size(11).color(color_text_strong()),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

// ── Page ───────────────────────────────────────────────────────────

/// Full "AI Models" tab page: provider editor, label editor, and action buttons.
pub(super) fn ai_models_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let providers_section = provider_tab_view(state);
    let labels_section = label_tab_view(state);

    let action_row = super::save_action_row(
        state,
        SettingsTab::AiModels,
        SettingsEvent::Models(ModelsEvent::SaveModels),
    );

    column![providers_section, labels_section, action_row]
        .spacing(16)
        .into()
}

// ── Update ─────────────────────────────────────────────────────────

/// Handle an AI Models tab event, mutating `state.working_models`.
pub(super) fn update(state: &mut SettingsState, event: ModelsEvent) {
    match event {
        ModelsEvent::SaveModels => {
            state.adding_label = false;
            state.drag_label = None;
            state.flush_current_provider();
            // Also confirm any pending label input.
            state.commit_new_label();
            state.save_feedback = Some(SettingsTab::AiModels);
        }
        // ── Provider actions ──────────────────────────────────
        ModelsEvent::SelectProvider(id) => {
            state.flush_current_provider();
            state.selected_provider_id = id.clone();
            if let Some(p) = state.working_models.providers.get(&id).cloned() {
                state.load_provider(&p);
            }
        }
        ModelsEvent::EditProviderName(v) => state.provider_name = v,
        ModelsEvent::EditProviderBaseUrl(v) => {
            // Clear cached models — the URL changed, old list is stale.
            state.cached_model_ids.remove(&state.selected_provider_id);
            state.available_model_ids.clear();
            state.models_fetch_error = None;
            state.provider_base_url = v;
        }
        ModelsEvent::EditProviderApiType(v) => state.provider_api_type = v,
        ModelsEvent::EditProviderAuth(v) => state.provider_auth = v,
        ModelsEvent::EditProviderApiKey(v) => state.provider_api_key = v,
        ModelsEvent::ToggleProviderStrictMode(v) => state.provider_strict_mode = v,
        ModelsEvent::RefreshModels => {
            state.cached_model_ids.remove(&state.selected_provider_id);
            state.available_model_ids.clear();
            state.models_fetch_error = None;
            state.fetching_models = true;
        }
        ModelsEvent::EditModelSearch(v) => {
            state.model_search = v;
            // Clearing the box drops the applied filter immediately so the
            // list can't be left stranded in a filtered state.
            if state.model_search.trim().is_empty() {
                state.model_filter.clear();
            }
        }
        ModelsEvent::ApplyModelFilter => {
            state.model_filter = state.model_search.trim().to_lowercase();
        }
        ModelsEvent::ModelsFetched(provider_id, result) => {
            state.fetching_models = false;
            match result {
                Ok(ids) => {
                    if !provider_id.is_empty() {
                        state
                            .cached_model_ids
                            .insert(provider_id.clone(), ids.clone());
                    }
                    // Only update display if we're still looking at this provider.
                    if provider_id == state.selected_provider_id {
                        state.available_model_ids = ids;
                    }
                }
                Err(e) => {
                    if provider_id == state.selected_provider_id {
                        state.models_fetch_error = Some(e);
                    }
                }
            }
        }
        ModelsEvent::ToggleModel(id, checked) => {
            // Auto-flush new provider so it exists in working_models.
            if state.is_new_provider {
                state.flush_current_provider();
            }
            if let Some(provider) = state
                .working_models
                .providers
                .get_mut(&state.selected_provider_id)
            {
                if checked {
                    if !provider.models.iter().any(|m| m.id == id) {
                        let model = if let Some(db_model) = state.model_db.get(&id) {
                            let cost = state
                                .selected_offer_source
                                .as_deref()
                                .and_then(|src| db_model.offers.iter().find(|o| o.source == src))
                                .cloned()
                                .unwrap_or_else(|| db_model.cost.clone());
                            Model {
                                id,
                                name: db_model.name.clone(),
                                thinking: db_model.thinking,
                                thinking_levels: db_model.thinking_levels.clone(),
                                input: db_model.input.clone(),
                                context_window: db_model.context_window,
                                max_tokens: db_model.max_tokens,
                                cost,
                                offers: db_model.offers.clone(),
                            }
                        } else {
                            let name = id.clone();
                            Model {
                                id,
                                name,
                                ..Default::default()
                            }
                        };
                        // Seed the editor if this model is being viewed.
                        if state.selected_model_id.as_deref() == Some(&model.id) {
                            state.model_edit = Some(ModelEditDraft::from_model(&model));
                        }
                        provider.models.push(model);
                    }
                } else {
                    provider.models.retain(|m| m.id != id);
                    if state.selected_model_id.as_deref() == Some(&id) {
                        state.selected_model_id = None;
                        state.model_edit = None;
                    }
                }
            }
        }
        ModelsEvent::SelectModelDetail(id) => {
            if state.selected_model_id.as_deref() == Some(&id) {
                state.selected_model_id = None;
                state.selected_offer_source = None;
                state.model_edit = None;
            } else {
                state.selected_model_id = Some(id.clone());
                state.selected_offer_source = None;
                // Seed the parameter editor when the clicked model is checked.
                state.model_edit = provider_models(state)
                    .and_then(|models| models.iter().find(|m| m.id == id))
                    .map(ModelEditDraft::from_model);
            }
        }
        ModelsEvent::EditModelParam(param) => apply_model_param(state, param),
        ModelsEvent::SelectOfferSource(source) => {
            state.selected_offer_source = Some(source);
        }
        ModelsEvent::NewProvider => {
            state.flush_current_provider();
            state.reset_provider_fields();
            state.selected_model_id = None;
            state.model_edit = None;
            state.selected_offer_source = None;
            state.available_model_ids.clear();
            state.selected_provider_id.clear();
            state.models_fetch_error = None;
        }
        ModelsEvent::CancelNewProvider => {
            state.is_new_provider = false;
            state.select_first_provider();
        }
        ModelsEvent::DeleteProvider(id) => {
            state.working_models.providers.shift_remove(&id);
            // Remove any labels referencing this provider
            state
                .working_models
                .models
                .retain(|_, cfg| cfg.provider_id != id);
            if state.selected_provider_id == id {
                state.selected_provider_id.clear();
                state.select_first_provider();
            }
        }
        // ── Label actions ─────────────────────────────────────
        ModelsEvent::DeleteLabel(name) => {
            state.working_models.models.shift_remove(&name);
        }
        ModelsEvent::StartAddLabel => {
            state.adding_label = true;
            state.new_label_name.clear();
        }
        ModelsEvent::NewLabelName(v) => state.new_label_name = v,
        ModelsEvent::AddLabel => {
            state.adding_label = false;
            state.commit_new_label();
        }
        ModelsEvent::LabelDragStart(index) => {
            state.drag_label = Some(index);
            state.drag_reordered = false;
        }
        ModelsEvent::LabelDragEnter(index) => {
            if let Some(from) = state.drag_label
                && from != index
                && index < state.working_models.models.len()
            {
                state.working_models.models.move_index(from, index);
                state.drag_label = Some(index);
                state.drag_reordered = true;
            }
        }
        ModelsEvent::LabelDragEnd => {
            state.drag_label = None;
            state.drag_reordered = false;
        }
        ModelsEvent::LabelInputFocus(focused) => {
            if !focused && state.adding_label {
                state.update(SettingsEvent::Models(ModelsEvent::AddLabel));
            }
        }
    }
}
