use super::icons;
use super::theme::{
    CRABOT_DIALOG_BG, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY, CRABOT_SURFACE, CRABOT_TEXT,
    CRABOT_TEXT_MUTED,
};
use crate::Message;
use crabot::model::{Model, ModelConfig, ModelList, Provider};
use crabot::model_database::ModelDatabase;
use iced::padding;
use iced::{
    Alignment, Border, Color, Element, Length,
    widget::{button, column, container, row, rule, scrollable, svg, text},
};
use std::collections::HashMap;

pub mod ai_models;

/// Widget id of the new-label text input — used to focus it and detect blur.
pub(crate) const NEW_LABEL_INPUT_ID: &str = "settings-new-label-input";
/// Widget id of the new-provider name input — used to focus it.
pub(crate) const NEW_PROVIDER_NAME_INPUT_ID: &str = "settings-new-provider-name-input";

// ── Tabs ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    AiModels,
    CustomTools,
    McpServers,
}

impl SettingsTab {
    fn label(&self) -> &'static str {
        match self {
            SettingsTab::AiModels => "AI Models",
            SettingsTab::CustomTools => "Custom Tools",
            SettingsTab::McpServers => "MCP Servers",
        }
    }
}

// ── Events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum SettingsEvent {
    SelectTab(SettingsTab),
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

#[derive(Debug, Clone)]
pub(crate) struct SettingsState {
    /// Currently active tab in the settings sidebar.
    pub(crate) selected_tab: SettingsTab,
    // Provider editing
    pub(super) selected_provider_id: String,
    pub(super) provider_name: String,
    pub(super) provider_base_url: String,
    pub(super) provider_api_type: String,
    pub(super) provider_auth: String,
    pub(super) provider_api_key: String,
    pub(super) provider_strict_mode: bool,
    pub(super) is_new_provider: bool,
    // Model fetching from /models endpoint
    pub(super) fetching_models: bool,
    pub(super) available_model_ids: Vec<String>,
    pub(super) models_fetch_error: Option<String>,
    /// Cache of fetched model IDs keyed by provider ID — avoids re-fetching on switch.
    cached_model_ids: HashMap<String, Vec<String>>,
    /// Which model ID is currently selected for detail display.
    pub(super) selected_model_id: Option<String>,
    // Label editing
    pub(super) new_label_name: String,
    /// Whether the blank new-label capsule is being edited.
    pub(super) adding_label: bool,
    /// Index of the label capsule currently being dragged.
    drag_label: Option<usize>,
    /// Whether the current drag changed the label order.
    drag_reordered: bool,
    /// Model database loaded from embedded assets for detail lookup.
    pub(super) model_db: ModelDatabase,
    /// Which offer source is selected for the currently-viewed model detail.
    pub(super) selected_offer_source: Option<String>,
    /// Working copy of models edited within the dialog — saved to disk on Save.
    pub(crate) working_models: ModelList,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            selected_tab: SettingsTab::AiModels,
            selected_provider_id: String::new(),
            provider_name: String::new(),
            provider_base_url: String::new(),
            provider_api_type: String::new(),
            provider_auth: String::new(),
            provider_api_key: String::new(),
            provider_strict_mode: false,
            is_new_provider: false,
            fetching_models: false,
            available_model_ids: Vec::new(),
            models_fetch_error: None,
            cached_model_ids: HashMap::new(),
            selected_model_id: None,
            new_label_name: String::new(),
            adding_label: false,
            drag_label: None,
            drag_reordered: false,
            model_db: ModelDatabase::default(),
            selected_offer_source: None,
            working_models: ModelList::default(),
        }
    }
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
            SettingsEvent::SelectTab(tab) => {
                self.selected_tab = tab;
            }
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
    #[allow(dead_code)]
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

// ── View ────────────────────────────────────────────────────────────

/// Returns the settings dialog content with a left sidebar of vertical tabs
/// and a content area that switches between tab pages.
/// The caller is responsible for placing it inside a modal structure.
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

    // ── Sidebar ────────────────────────────────────────────────────
    let tabs = [
        SettingsTab::AiModels,
        SettingsTab::CustomTools,
        SettingsTab::McpServers,
    ];
    let sidebar_buttons: Vec<Element<'a, Message>> = tabs
        .iter()
        .map(|&tab| {
            let is_active = state.selected_tab == tab;
            button(text(tab.label()).size(13))
                .width(Length::Fill)
                .style(sidebar_tab_style(is_active))
                .on_press(Message::SettingsEvent(SettingsEvent::SelectTab(tab)))
                .into()
        })
        .collect();

    let sidebar = container(column(sidebar_buttons).spacing(2).padding([8, 0]))
        .width(160)
        .height(Length::Fill)
        .style(|_: &iced::Theme| container::Style {
            background: Some(CRABOT_SURFACE.into()),
            border: Border::default().rounded(CRABOT_DIALOG_RADIUS),
            ..container::Style::default()
        });

    // ── Tab content ────────────────────────────────────────────────
    let tab_content: Element<'a, Message> = match state.selected_tab {
        SettingsTab::AiModels => ai_models::ai_models_page(state),
        SettingsTab::CustomTools => blank_tab_page("Custom Tools"),
        SettingsTab::McpServers => blank_tab_page("MCP Servers"),
    };

    let content_area = scrollable(tab_content)
        .width(Length::Fill)
        .height(Length::Fill);

    // ── Layout ─────────────────────────────────────────────────────
    container(
        column![
            header,
            section_rule(),
            row![
                sidebar,
                container(content_area)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(padding::left(16)),
            ]
            .height(Length::Fill),
        ]
        .spacing(12)
        .padding(20),
    )
    .style(|_: &iced::Theme| container::Style {
        background: Some(CRABOT_DIALOG_BG.into()),
        border: Border::default().rounded(CRABOT_DIALOG_RADIUS),
        ..container::Style::default()
    })
    .max_width(900)
    .max_height(800)
    .into()
}

pub(super) fn section_rule() -> Element<'static, Message> {
    rule::horizontal(1)
        .style(|_: &iced::Theme| rule::Style {
            color: CRABOT_PRIMARY,
            fill_mode: rule::FillMode::Full,
            radius: 0.0.into(),
            snap: false,
        })
        .into()
}

// ── Sidebar & placeholder helpers ──────────────────────────────────

/// Style for a vertical tab button in the settings sidebar.
fn sidebar_tab_style(active: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_: &iced::Theme, _status: button::Status| {
        if active {
            button::Style {
                background: Some(CRABOT_PRIMARY.into()),
                text_color: Color::WHITE,
                border: Border::default().rounded(6),
                ..button::Style::default()
            }
        } else {
            button::Style {
                background: Some(Color::TRANSPARENT.into()),
                text_color: CRABOT_TEXT,
                border: Border::default().rounded(6),
                ..button::Style::default()
            }
        }
    }
}

/// A blank placeholder page shown for tabs that haven't been implemented yet.
fn blank_tab_page<'a>(title: &'static str) -> Element<'a, Message> {
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
    .spacing(10);

    container(
        column![
            column![
                text(title)
                    .size(13)
                    .font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..iced::Font::DEFAULT
                    })
                    .color(CRABOT_PRIMARY),
                text("Configuration will be available in a future update.")
                    .size(12)
                    .color(CRABOT_TEXT_MUTED),
            ]
            .spacing(8)
            .align_x(Alignment::Center)
            .width(Length::Fill),
            iced::widget::Space::new().height(Length::Fill),
            action_row,
        ]
        .spacing(16),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
