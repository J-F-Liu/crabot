use super::icons;
use super::theme::{
    CRABOT_BORDER, CRABOT_DANGER, CRABOT_DIALOG_BG, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY,
    CRABOT_SURFACE, CRABOT_TEXT, CRABOT_TEXT_MUTED,
};
use crate::Message;
use crabot::model::{Model, ModelConfig, ModelList, Provider, currency_symbol};
use crabot::model_database::ModelDatabase;
use iced::padding;
use iced::{
    Alignment, Border, Color, Element, Length, mouse,
    widget::{
        button, checkbox, column, container, mouse_area, pick_list, row, rule, scrollable, svg,
        text, text_input,
    },
};
use std::collections::HashMap;

/// Widget id of the new-label text input — used to focus it and detect blur.
pub(crate) const NEW_LABEL_INPUT_ID: &str = "settings-new-label-input";
/// Widget id of the new-provider name input — used to focus it.
pub(crate) const NEW_PROVIDER_NAME_INPUT_ID: &str = "settings-new-provider-name-input";

// ── Events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum SettingsEvent {
    Close,
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
    ToggleModel(String, bool),
    SelectModelDetail(String),
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
    /// Save all changes and close the dialog.
    Save,
}

// ── State ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub(crate) struct SettingsState {
    // Provider editing
    selected_provider_id: String,
    provider_name: String,
    provider_base_url: String,
    provider_api_type: String,
    provider_auth: String,
    provider_api_key: String,
    provider_strict_mode: bool,
    is_new_provider: bool,
    // Model fetching from /models endpoint
    fetching_models: bool,
    available_model_ids: Vec<String>,
    models_fetch_error: Option<String>,
    /// Cache of fetched model IDs keyed by provider ID — avoids re-fetching on switch.
    cached_model_ids: HashMap<String, Vec<String>>,
    /// Which model ID is currently selected for detail display.
    selected_model_id: Option<String>,
    // Label editing
    new_label_name: String,
    /// Whether the blank new-label capsule is being edited.
    adding_label: bool,
    /// Index of the label capsule currently being dragged.
    drag_label: Option<usize>,
    /// Whether the current drag changed the label order.
    drag_reordered: bool,
    /// Model database loaded from embedded assets for detail lookup.
    model_db: ModelDatabase,
    /// Which offer source is selected for the currently-viewed model detail.
    selected_offer_source: Option<String>,
    /// Working copy of models edited within the dialog — saved to disk on Save.
    pub(crate) working_models: ModelList,
}

impl SettingsState {
    /// Load provider fields from an existing provider for editing.
    fn load_provider(&mut self, p: &Provider) {
        self.provider_name = p.name.clone();
        self.provider_base_url = p.base_url.clone();
        self.provider_api_type = p.api_type.clone();
        self.provider_auth = p.auth.clone();
        self.provider_api_key = p.api_key.clone();
        self.provider_strict_mode = p.strict_mode;
        self.is_new_provider = false;
        self.selected_model_id = None;
        // Use cached model IDs if available, otherwise trigger a fetch.
        if let Some(cached) = self.cached_model_ids.get(&self.selected_provider_id) {
            self.available_model_ids = cached.clone();
            self.fetching_models = false;
            self.models_fetch_error = None;
        } else {
            self.available_model_ids.clear();
            self.fetching_models = true;
            self.models_fetch_error = None;
        }
    }

    /// Reset provider fields to defaults (for new provider).
    fn reset_provider_fields(&mut self) {
        self.provider_name.clear();
        self.provider_base_url.clear();
        self.provider_api_type = String::from("openai");
        self.provider_auth = String::from("apiKey");
        self.provider_api_key.clear();
        self.provider_strict_mode = false;
        self.is_new_provider = true;
    }

    /// Select the first provider from the working models.
    pub(crate) fn select_first_provider(&mut self) {
        self.model_db = ModelDatabase::load_embedded();
        if let Some(first) = self.working_models.providers.keys().next() {
            self.selected_provider_id = first.clone();
            if let Some(p) = self.working_models.providers.get(first).cloned() {
                self.load_provider(&p);
            }
        }
    }

    /// Build a `Provider` from the current form fields.
    fn build_provider(&self) -> Provider {
        Provider {
            name: self.provider_name.clone(),
            base_url: self.provider_base_url.clone(),
            api_type: self.provider_api_type.clone(),
            auth: self.provider_auth.clone(),
            api_key: self.provider_api_key.clone(),
            strict_mode: self.provider_strict_mode,
            headers: Default::default(),
            models: vec![], // models preserved separately
        }
    }

    /// Write the current form fields back into `working_models` for the
    /// selected provider (or create a new provider entry if `is_new_provider`).
    fn flush_current_provider(&mut self) {
        let name = self.provider_name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let provider = self.build_provider();
        if self.is_new_provider {
            let base = name.to_lowercase().replace(' ', "-");
            let mut id = base.clone();
            let mut suffix = 2;
            while self.working_models.providers.contains_key(&id) {
                id = format!("{}-{}", base, suffix);
                suffix += 1;
            }
            self.selected_provider_id = id.clone();
            self.working_models.providers.insert(id, provider);
            self.is_new_provider = false;
        } else {
            let id = self.selected_provider_id.clone();
            if id.is_empty() || !self.working_models.providers.contains_key(&id) {
                return;
            }
            if let Some(existing) = self.working_models.providers.get_mut(&id) {
                let models_list = std::mem::take(&mut existing.models);
                let headers = std::mem::take(&mut existing.headers);
                *existing = provider;
                existing.models = models_list;
                existing.headers = headers;
            }
        }
    }

    // ── Update ──────────────────────────────────────────────────────

    /// Handle a `SettingsEvent`, mutating `self.working_models`.
    pub(crate) fn update(&mut self, event: SettingsEvent) {
        match event {
            SettingsEvent::Close => {
                // Drop any in-progress label editing / dragging.
                self.adding_label = false;
                self.drag_label = None;
            }
            SettingsEvent::Save => {
                self.adding_label = false;
                self.drag_label = None;
                self.flush_current_provider();
                // Also confirm any pending label input.
                let name = self.new_label_name.trim().to_string();
                self.new_label_name.clear();
                if !name.is_empty() && !self.working_models.models.contains_key(&name) {
                    self.working_models
                        .models
                        .insert(name, ModelConfig::default());
                }
            }
            // ── Provider actions ──────────────────────────────────
            SettingsEvent::SelectProvider(id) => {
                self.flush_current_provider();
                self.selected_provider_id = id.clone();
                if let Some(p) = self.working_models.providers.get(&id).cloned() {
                    self.load_provider(&p);
                }
            }
            SettingsEvent::EditProviderName(v) => self.provider_name = v,
            SettingsEvent::EditProviderBaseUrl(v) => {
                // Clear cached models — the URL changed, old list is stale.
                self.cached_model_ids.remove(&self.selected_provider_id);
                self.available_model_ids.clear();
                self.models_fetch_error = None;
                self.provider_base_url = v;
            }
            SettingsEvent::EditProviderApiType(v) => self.provider_api_type = v,
            SettingsEvent::EditProviderAuth(v) => self.provider_auth = v,
            SettingsEvent::EditProviderApiKey(v) => self.provider_api_key = v,
            SettingsEvent::ToggleProviderStrictMode(v) => self.provider_strict_mode = v,
            SettingsEvent::RefreshModels => {
                self.cached_model_ids.remove(&self.selected_provider_id);
                self.available_model_ids.clear();
                self.models_fetch_error = None;
                self.fetching_models = true;
            }
            SettingsEvent::ModelsFetched(provider_id, result) => {
                self.fetching_models = false;
                match result {
                    Ok(ids) => {
                        if !provider_id.is_empty() {
                            self.cached_model_ids
                                .insert(provider_id.clone(), ids.clone());
                        }
                        // Only update display if we're still looking at this provider.
                        if provider_id == self.selected_provider_id {
                            self.available_model_ids = ids;
                        }
                    }
                    Err(e) => {
                        if provider_id == self.selected_provider_id {
                            self.models_fetch_error = Some(e);
                        }
                    }
                }
            }
            SettingsEvent::ToggleModel(id, checked) => {
                if let Some(provider) = self
                    .working_models
                    .providers
                    .get_mut(&self.selected_provider_id)
                {
                    if checked {
                        if !provider.models.iter().any(|m| m.id == id) {
                            let model = if let Some(db_model) = self.model_db.get(&id) {
                                let cost = self
                                    .selected_offer_source
                                    .as_deref()
                                    .and_then(|src| {
                                        db_model.offers.iter().find(|o| o.source == src)
                                    })
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
                                Model {
                                    id,
                                    ..Default::default()
                                }
                            };
                            provider.models.push(model);
                        }
                    } else {
                        provider.models.retain(|m| m.id != id);
                    }
                }
            }
            SettingsEvent::SelectModelDetail(id) => {
                if self.selected_model_id.as_deref() == Some(&id) {
                    self.selected_model_id = None;
                    self.selected_offer_source = None;
                } else {
                    self.selected_model_id = Some(id);
                    self.selected_offer_source = None;
                }
            }
            SettingsEvent::SelectOfferSource(source) => {
                self.selected_offer_source = Some(source);
            }
            SettingsEvent::NewProvider => {
                self.flush_current_provider();
                self.reset_provider_fields();
                self.selected_model_id = None;
                self.selected_offer_source = None;
                self.available_model_ids.clear();
                self.selected_provider_id.clear();
            }
            SettingsEvent::CancelNewProvider => {
                self.is_new_provider = false;
                self.select_first_provider();
            }
            SettingsEvent::DeleteProvider(id) => {
                self.working_models.providers.shift_remove(&id);
                // Remove any labels referencing this provider
                self.working_models
                    .models
                    .retain(|_, cfg| cfg.provider_id != id);
                if self.selected_provider_id == id {
                    self.selected_provider_id.clear();
                    self.select_first_provider();
                }
            }
            // ── Label actions ─────────────────────────────────────
            SettingsEvent::DeleteLabel(name) => {
                self.working_models.models.shift_remove(&name);
            }
            SettingsEvent::StartAddLabel => {
                self.adding_label = true;
                self.new_label_name.clear();
            }
            SettingsEvent::NewLabelName(v) => self.new_label_name = v,
            SettingsEvent::AddLabel => {
                self.adding_label = false;
                let name = self.new_label_name.trim().to_string();
                self.new_label_name.clear();
                if !name.is_empty() && !self.working_models.models.contains_key(&name) {
                    self.working_models
                        .models
                        .insert(name, ModelConfig::default());
                }
            }
            SettingsEvent::LabelDragStart(index) => {
                self.drag_label = Some(index);
                self.drag_reordered = false;
            }
            SettingsEvent::LabelDragEnter(index) => {
                if let Some(from) = self.drag_label
                    && from != index
                    && index < self.working_models.models.len()
                {
                    self.working_models.models.move_index(from, index);
                    self.drag_label = Some(index);
                    self.drag_reordered = true;
                }
            }
            SettingsEvent::LabelDragEnd => {
                self.drag_label = None;
                self.drag_reordered = false;
            }
        }
    }

    /// Whether the new-label capsule input is currently active.
    pub(crate) fn is_adding_label(&self) -> bool {
        self.adding_label
    }

    /// Index of the label capsule currently being dragged, if any.
    pub(crate) fn dragging_label(&self) -> Option<usize> {
        self.drag_label
    }

    /// Whether a label capsule drag is in progress.
    pub(crate) fn is_label_dragging(&self) -> bool {
        self.drag_label.is_some()
    }

    /// Current provider base URL (used for model fetching).
    pub(crate) fn provider_base_url(&self) -> &str {
        &self.provider_base_url
    }

    /// Current provider ID (used to tag async fetch results).
    pub(crate) fn current_provider_id(&self) -> &str {
        &self.selected_provider_id
    }

    /// Current provider API key (used for model fetching).
    pub(crate) fn provider_api_key(&self) -> &str {
        &self.provider_api_key
    }

    /// Whether a model-list fetch is needed for the current provider.
    pub(crate) fn needs_fetch(&self) -> bool {
        self.fetching_models
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

// ── View ────────────────────────────────────────────────────────────

/// Returns the settings dialog content with Providers and Labels sections
/// displayed on a single scrollable page. The caller is responsible for
/// placing it inside a modal structure (e.g. a stack with a backdrop).
pub(crate) fn settings_dialog<'a>(state: &'a SettingsState) -> Element<'a, Message> {
    let header = container(
        row![
            text("Settings")
                .size(18)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..iced::Font::DEFAULT
                })
                .color(CRABOT_PRIMARY),
            iced::widget::Space::new().width(Length::Fill),
            button(
                svg(svg::Handle::from_memory(icons::CLOSE))
                    .width(16)
                    .height(16)
                    .style(|theme: &iced::Theme, _status| svg::Style {
                        color: Some(theme.palette().text),
                    }),
            )
            .padding([4, 8])
            .style(crate::views::styles::secondary_button)
            .on_press(Message::SettingsEvent(SettingsEvent::Close)),
        ]
        .align_y(Alignment::Center),
    );

    let providers_section = provider_tab_view(state);
    let labels_section = label_tab_view(state);

    let save_button = button(text("OK"))
        .style(crate::views::styles::primary_button)
        .on_press(Message::SettingsEvent(SettingsEvent::Save));

    let cancel_button = button(text("Cancel"))
        .style(crate::views::styles::secondary_button)
        .on_press(Message::SettingsEvent(SettingsEvent::Close));

    let action_row = row![
        iced::widget::Space::new().width(Length::Fill),
        cancel_button,
        save_button,
    ]
    .spacing(10)
    .padding(padding::top(8));

    container(
        column![
            header,
            section_rule(),
            providers_section,
            labels_section,
            action_row,
        ]
        .spacing(16)
        .padding(20),
    )
    .style(|_: &iced::Theme| container::Style {
        background: Some(CRABOT_DIALOG_BG.into()),
        border: Border::default().rounded(CRABOT_DIALOG_RADIUS),
        ..container::Style::default()
    })
    .max_width(720)
    .max_height(800)
    .into()
}

fn section_rule() -> Element<'static, Message> {
    rule::horizontal(1)
        .style(|_: &iced::Theme| rule::Style {
            color: CRABOT_PRIMARY,
            fill_mode: rule::FillMode::Full,
            radius: 0.0.into(),
            snap: false,
        })
        .into()
}

// ── Provider section ────────────────────────────────────────────────

fn provider_tab_view<'a>(state: &'a SettingsState) -> Element<'a, Message> {
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
        Message::SettingsEvent(SettingsEvent::SelectProvider(e.id))
    })
    .width(Length::Fill);

    let is_editing = !state.selected_provider_id.is_empty() || state.is_new_provider;

    let form: Element<_> = if is_editing {
        const API_TYPES: &[&str] = &[
            "openai",
            "openai-completions",
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

        const AUTH_TYPES: &[&str] = &["apiKey", "bearer", "basic", "none"];
        let selected_auth = AUTH_TYPES
            .iter()
            .find(|&&t| t == state.provider_auth)
            .copied();

        let form_body = column![
            field_row(
                "Name",
                &state.provider_name,
                |v| { Message::SettingsEvent(SettingsEvent::EditProviderName(v)) },
                "Provider name",
                Some(NEW_PROVIDER_NAME_INPUT_ID),
                None,
            ),
            field_row(
                "Base URL",
                &state.provider_base_url,
                |v| { Message::SettingsEvent(SettingsEvent::EditProviderBaseUrl(v)) },
                "Base URL of the provider, press Enter to fetch model list",
                None,
                Some(Message::SettingsEvent(SettingsEvent::RefreshModels)),
            ),
            label_pick_row("API Type", API_TYPES, selected_api_type, |v| {
                Message::SettingsEvent(SettingsEvent::EditProviderApiType(v.to_string()))
            }),
            label_pick_row("Auth", AUTH_TYPES, selected_auth, |v| {
                Message::SettingsEvent(SettingsEvent::EditProviderAuth(v.to_string()))
            }),
            field_row(
                "API Key",
                &state.provider_api_key,
                |v| { Message::SettingsEvent(SettingsEvent::EditProviderApiKey(v)) },
                "API Key or its enviroment variable name",
                None,
                None,
            ),
            checkbox_row("Strict Mode", state.provider_strict_mode, |v| {
                Message::SettingsEvent(SettingsEvent::ToggleProviderStrictMode(v))
            },),
            models_section_view(state, &state.working_models),
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
                .color(iced::Color::from_rgb8(0x66, 0x66, 0x66)),
            iced::widget::Space::new().height(Length::Fill),
            button(text("New"))
                .style(crate::views::styles::primary_button)
                .on_press(Message::SettingsEvent(SettingsEvent::NewProvider)),
        ]
        .spacing(12)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into()
    };

    let action_button: Element<'_, Message> = if state.is_new_provider {
        button(text("Cancel"))
            .style(crate::views::styles::secondary_button)
            .on_press(Message::SettingsEvent(SettingsEvent::CancelNewProvider))
            .into()
    } else if !state.selected_provider_id.is_empty() {
        button(text("Delete"))
            .style(|theme: &iced::Theme, status| {
                let mut s = crate::views::styles::secondary_button(theme, status);
                s.text_color = iced::Color::from_rgb8(0xE5, 0x4D, 0x4D);
                s
            })
            .on_press(Message::SettingsEvent(SettingsEvent::DeleteProvider(
                state.selected_provider_id.clone(),
            )))
            .into()
    } else {
        iced::widget::Space::new().width(0).into()
    };

    column![
        row![
            text("Model Providers")
                .size(13)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..iced::Font::DEFAULT
                })
                .color(CRABOT_PRIMARY),
            picker,
            row![
                button(text("New"))
                    .style(crate::views::styles::primary_button)
                    .on_press(Message::SettingsEvent(SettingsEvent::NewProvider)),
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
fn label_tab_view<'a>(state: &'a SettingsState) -> Element<'a, Message> {
    let section_header = text("Model Labels")
        .size(13)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::DEFAULT
        })
        .color(CRABOT_PRIMARY);

    let dragging = state.dragging_label();

    let mut chips: Vec<Element<'a, Message>> = state
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
                        .style(chip_close_style)
                        .on_press(Message::SettingsEvent(SettingsEvent::DeleteLabel(
                            name.clone(),
                        ))),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([5, 12])
            .style(chip_style(dragging == Some(i)));

            mouse_area(chip)
                .on_press(Message::SettingsEvent(SettingsEvent::LabelDragStart(i)))
                .on_enter(Message::SettingsEvent(SettingsEvent::LabelDragEnter(i)))
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
                .color(CRABOT_TEXT_MUTED)
                .into(),
        );
    }

    if state.is_adding_label() {
        chips.push(
            text_input("Label name", &state.new_label_name)
                .id(NEW_LABEL_INPUT_ID)
                .on_input(|v| Message::SettingsEvent(SettingsEvent::NewLabelName(v)))
                .on_submit(Message::SettingsEvent(SettingsEvent::AddLabel))
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
            .on_press(Message::SettingsEvent(SettingsEvent::StartAddLabel))
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
        .color(CRABOT_TEXT_MUTED);
    column![
        section_header,
        container(column![labels_section, section_rule(), hint].spacing(10))
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
        background: Some(CRABOT_SURFACE.into()),
        border: Border::default().rounded(999).width(1).color(if dragged {
            CRABOT_PRIMARY
        } else {
            CRABOT_BORDER
        }),
        ..container::Style::default()
    }
}

/// Outlined "+" capsule that starts a new label.
fn add_chip_style(_: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        border: Border::default()
            .rounded(999)
            .width(1)
            .color(CRABOT_PRIMARY),
        ..container::Style::default()
    }
}

/// Borderless "✕" button inside a capsule; red on hover.
fn chip_close_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => CRABOT_DANGER,
            _ => CRABOT_TEXT_MUTED,
        },
        ..button::Style::default()
    }
}

/// Capsule-shaped style for the new-label text input.
fn chip_input_style(_theme: &iced::Theme, _status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Color::WHITE.into(),
        border: Border::default()
            .rounded(999)
            .width(1)
            .color(CRABOT_PRIMARY),
        icon: CRABOT_TEXT_MUTED,
        placeholder: CRABOT_TEXT_MUTED,
        value: CRABOT_TEXT,
        selection: CRABOT_PRIMARY.scale_alpha(0.3),
    }
}

// ── Helpers ──────────────────────────────────────────────────────

fn field_row<'a>(
    label: &'static str,
    value: &str,
    on_input: impl Fn(String) -> Message + 'a,
    placeholder: &'a str,
    id: Option<&'static str>,
    on_submit: Option<Message>,
) -> Element<'a, Message> {
    let mut input = iced::widget::text_input(placeholder, value)
        .on_input(on_input)
        .width(Length::Fill)
        .padding(4);
    if let Some(input_id) = id {
        input = input.id(input_id);
    }
    if let Some(msg) = on_submit {
        input = input.on_submit(msg);
    }
    let label_col = container(text(label).size(14))
        .width(90)
        .align_x(iced::Alignment::End);
    row![label_col, input]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

fn label_pick_row<'a>(
    label: &'static str,
    options: &'a [&'static str],
    selected: Option<&'static str>,
    on_select: impl Fn(&'static str) -> Message + 'a,
) -> Element<'a, Message> {
    let label_col = container(text(label).size(14))
        .width(90)
        .align_x(iced::Alignment::End);
    row![
        label_col,
        pick_list(options, selected, on_select).width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn checkbox_row<'a>(
    label: &'static str,
    checked: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> Element<'a, Message> {
    let label_col = container(text(label).size(14))
        .width(90)
        .align_x(iced::Alignment::End);
    let cb = iced::widget::checkbox(checked)
        .label("")
        .on_toggle(on_toggle)
        .style(crate::views::primary_checkbox);
    row![label_col, cb]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

/// Renders the models section with a table of checkboxes and model IDs.
fn models_section_view<'a>(
    state: &'a SettingsState,
    models: &'a ModelList,
) -> Element<'a, Message> {
    let provider_model_ids: Vec<&str> = models
        .providers
        .get(&state.selected_provider_id)
        .map(|p| p.models.iter().map(|m| m.id.as_str()).collect())
        .unwrap_or_default();

    // Header: "Models" label
    let header = container(text("Models").size(14))
        .width(90)
        .align_x(iced::Alignment::End);

    let display_ids: Vec<&str> = if state.available_model_ids.is_empty() {
        provider_model_ids.to_vec()
    } else {
        state
            .available_model_ids
            .iter()
            .map(|s| s.as_str())
            .collect()
    };

    // Body: status or table + details
    let body: Element<'_, Message> = if state.fetching_models {
        text("Loading models…")
            .size(12)
            .color(CRABOT_TEXT_MUTED)
            .into()
    } else if display_ids.is_empty() {
        button(text("Fetch Models").size(12))
            .style(crate::views::styles::primary_button)
            .on_press(Message::SettingsEvent(SettingsEvent::RefreshModels))
            .into()
    } else {
        // ── Table column ──────────────────────────────────────────
        // Header row
        let table_header = row![iced::widget::Space::new().width(20), text("Model ID")]
            .spacing(8)
            .height(0);

        // Data rows
        let model_rows: Vec<Element<'_, Message>> = display_ids
            .iter()
            .map(|&id| {
                let checked = provider_model_ids.contains(&id);
                let id_string = id.to_string();
                let id_string2 = id.to_string();
                let is_selected = state.selected_model_id.as_deref() == Some(id);

                let cb = checkbox(checked)
                    .label("")
                    .on_toggle(move |v| {
                        Message::SettingsEvent(SettingsEvent::ToggleModel(id_string.clone(), v))
                    })
                    .style(crate::views::primary_checkbox);

                let id_text = text(id.to_string()).size(12);
                let id_inner = container(id_text).padding([2, 4]);
                let id_cell = mouse_area(container(id_inner).style(move |_: &iced::Theme| {
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
                }))
                .on_press(Message::SettingsEvent(
                    SettingsEvent::SelectModelDetail(id_string2),
                ));

                let row_bg: Element<_> =
                    container(row![cb, id_cell].spacing(8).align_y(Alignment::Center))
                        .padding(1)
                        .into();
                row_bg
            })
            .collect();

        let table = container(
            scrollable(
                column![table_header]
                    .push(column(model_rows).spacing(1))
                    .spacing(4),
            )
            .height(Length::Fixed(200.0))
            .width(Length::FillPortion(1)),
        )
        .padding(2)
        .style(|_: &iced::Theme| container::Style {
            border: Border::default().rounded(4).width(1).color(CRABOT_BORDER),
            ..container::Style::default()
        });

        // ── Details panel ────────────────────────────────────────
        let details: Element<'_, Message> = if let Some(selected_id) = &state.selected_model_id {
            let provider_models = models
                .providers
                .get(&state.selected_provider_id)
                .map(|p| &p.models);

            if let Some(model) =
                provider_models.and_then(|ml| ml.iter().find(|m| &m.id == selected_id))
            {
                let name = if model.name.is_empty() {
                    "—".to_string()
                } else {
                    model.name.clone()
                };
                let mut header = vec![
                    detail_row("Name", name),
                    detail_row(
                        "Thinking",
                        if model.thinking {
                            "yes".into()
                        } else {
                            "no".into()
                        },
                    ),
                ];
                if !model.thinking_levels.is_empty() {
                    header.push(detail_row("Think Levels", model.thinking_levels.join(", ")));
                }
                model_detail_panel(
                    &model.cost,
                    &model.input,
                    model.context_window,
                    model.max_tokens,
                    header,
                )
            } else if let Some(details) = state.model_db.get(selected_id) {
                // Pick the active offer: user-selected source, or first.
                let active_cost = state
                    .selected_offer_source
                    .as_deref()
                    .and_then(|src| details.offers.iter().find(|o| o.source == src))
                    .unwrap_or_else(|| details.offers.first().unwrap_or(&details.cost));

                let header = vec![
                    detail_row("Name", details.name.clone()),
                    detail_row(
                        "Thinking",
                        if details.thinking {
                            "yes".into()
                        } else {
                            "no".into()
                        },
                    ),
                ];

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
                        Message::SettingsEvent(SettingsEvent::SelectOfferSource(src))
                    });
                    column![
                        container(
                            row![
                                text("Offer").size(12).color(CRABOT_TEXT_MUTED).width(60),
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
                        .color(CRABOT_TEXT_MUTED),
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
                    .color(CRABOT_TEXT_MUTED),
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
    header: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
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
        .width(Length::FillPortion(1))
        .into()
}

/// Single label–value row for the model detail panel.
fn detail_row(label: &'static str, value: String) -> Element<'static, Message> {
    row![
        text(label).size(11).color(CRABOT_TEXT_MUTED).width(70),
        text(value).size(11).color(CRABOT_TEXT),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn form_card_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(0xF4, 0xF4, 0xF4).into()),
        border: Border::default().rounded(8).width(1).color(CRABOT_BORDER),
        ..container::Style::default()
    }
}
