use super::icons;
use super::theme::{
    CRABOT_BORDER, CRABOT_DANGER, CRABOT_DIALOG_BG, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY,
    CRABOT_SURFACE, CRABOT_TEXT, CRABOT_TEXT_MUTED,
};
use crate::Message;
use crabot::model::{Model, ModelConfig, ModelList, Provider};
use iced::{
    Alignment, Border, Color, Element, Length, mouse,
    widget::{
        button, checkbox, column, container, mouse_area, pick_list, row, rule, scrollable, svg,
        text, text_input,
    },
};
use std::collections::HashSet;

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
    SaveProvider,
    CancelNewProvider,
    DeleteProvider(String),
    ModelsFetched(Result<Vec<String>, String>),
    ToggleModel(String, bool),
    SelectModelDetail(String),
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
        // Reset model-fetch state when switching providers
        self.fetching_models = true;
        self.available_model_ids.clear();
        self.models_fetch_error = None;
        self.selected_model_id = None;
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

    /// Select the first provider when the settings dialog opens.
    pub(crate) fn select_first_provider(&mut self, models: &ModelList) {
        if let Some(first) = models.providers.keys().next() {
            self.selected_provider_id = first.clone();
            if let Some(p) = models.providers.get(first) {
                self.load_provider(p);
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

    // ── Update ──────────────────────────────────────────────────────

    /// Handle a `SettingsEvent`, mutating `self` and `models`.
    /// Returns `true` if `models` was modified and needs saving.
    pub(crate) fn update(&mut self, event: SettingsEvent, models: &mut ModelList) -> bool {
        match event {
            SettingsEvent::Close => {
                // Drop any in-progress label editing / dragging.
                self.adding_label = false;
                self.drag_label = None;
                false
            }
            // ── Provider actions ──────────────────────────────────
            SettingsEvent::SelectProvider(id) => {
                self.selected_provider_id = id.clone();
                if let Some(p) = models.providers.get(&id) {
                    self.load_provider(p);
                }
                false
            }
            SettingsEvent::EditProviderName(v) => {
                self.provider_name = v;
                false
            }
            SettingsEvent::EditProviderBaseUrl(v) => {
                self.provider_base_url = v;
                false
            }
            SettingsEvent::EditProviderApiType(v) => {
                self.provider_api_type = v;
                false
            }
            SettingsEvent::EditProviderAuth(v) => {
                self.provider_auth = v;
                false
            }
            SettingsEvent::EditProviderApiKey(v) => {
                self.provider_api_key = v;
                false
            }
            SettingsEvent::ToggleProviderStrictMode(v) => {
                self.provider_strict_mode = v;
                false
            }
            SettingsEvent::ModelsFetched(result) => {
                self.fetching_models = false;
                match result {
                    Ok(ids) => self.available_model_ids = ids,
                    Err(e) => self.models_fetch_error = Some(e),
                }
                false
            }
            SettingsEvent::ToggleModel(id, checked) => {
                if let Some(provider) = models.providers.get_mut(&self.selected_provider_id) {
                    if checked {
                        if !provider.models.iter().any(|m| m.id == id) {
                            provider.models.push(Model {
                                id,
                                ..Default::default()
                            });
                            return true;
                        }
                    } else {
                        let len_before = provider.models.len();
                        provider.models.retain(|m| m.id != id);
                        if provider.models.len() != len_before {
                            return true;
                        }
                    }
                }
                false
            }
            SettingsEvent::SelectModelDetail(id) => {
                if self.selected_model_id.as_deref() == Some(&id) {
                    self.selected_model_id = None;
                } else {
                    self.selected_model_id = Some(id);
                }
                false
            }
            SettingsEvent::NewProvider => {
                self.reset_provider_fields();
                self.selected_provider_id.clear();
                false
            }
            SettingsEvent::SaveProvider => {
                let name = self.provider_name.trim().to_string();
                if name.is_empty() {
                    return false;
                }
                let provider = self.build_provider();
                if self.is_new_provider {
                    let base = name.to_lowercase().replace(' ', "-");
                    let mut id = base.clone();
                    let mut suffix = 2;
                    while models.providers.contains_key(&id) {
                        id = format!("{}-{}", base, suffix);
                        suffix += 1;
                    }
                    self.selected_provider_id = id.clone();
                    models.providers.insert(id, provider);
                    self.is_new_provider = false;
                } else {
                    let id = self.selected_provider_id.clone();
                    if let Some(existing) = models.providers.get_mut(&id) {
                        // Preserve existing models and headers
                        let models_list = std::mem::take(&mut existing.models);
                        let headers = std::mem::take(&mut existing.headers);
                        *existing = provider;
                        existing.models = models_list;
                        existing.headers = headers;
                    }
                }
                true
            }
            SettingsEvent::CancelNewProvider => {
                self.is_new_provider = false;
                self.select_first_provider(models);
                false
            }
            SettingsEvent::DeleteProvider(id) => {
                models.providers.shift_remove(&id);
                // Remove any labels referencing this provider
                models.models.retain(|_, cfg| cfg.provider_id != id);
                if self.selected_provider_id == id {
                    self.selected_provider_id.clear();
                    self.select_first_provider(models);
                }
                true
            }
            // ── Label actions ─────────────────────────────────────
            SettingsEvent::DeleteLabel(name) => {
                models.models.shift_remove(&name);
                true
            }
            SettingsEvent::StartAddLabel => {
                self.adding_label = true;
                self.new_label_name.clear();
                false
            }
            SettingsEvent::NewLabelName(v) => {
                self.new_label_name = v;
                false
            }
            SettingsEvent::AddLabel => {
                self.adding_label = false;
                let name = self.new_label_name.trim().to_string();
                self.new_label_name.clear();
                if !name.is_empty() && !models.models.contains_key(&name) {
                    models.models.insert(name, ModelConfig::default());
                    return true;
                }
                false
            }
            SettingsEvent::LabelDragStart(index) => {
                self.drag_label = Some(index);
                self.drag_reordered = false;
                false
            }
            SettingsEvent::LabelDragEnter(index) => {
                if let Some(from) = self.drag_label
                    && from != index
                    && index < models.models.len()
                {
                    models.models.move_index(from, index);
                    self.drag_label = Some(index);
                    self.drag_reordered = true;
                }
                false
            }
            SettingsEvent::LabelDragEnd => {
                self.drag_label = None;
                std::mem::take(&mut self.drag_reordered)
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

    /// Current provider API key (used for model fetching).
    pub(crate) fn provider_api_key(&self) -> &str {
        &self.provider_api_key
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
/// The dialog can only be dismissed via its ✕ close button.
pub(crate) fn settings_dialog<'a>(
    state: &'a SettingsState,
    models: &'a ModelList,
) -> Element<'a, Message> {
    let header = row![
        text("Settings")
            .size(16)
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
        .padding([2, 8])
        .style(crate::views::styles::secondary_button)
        .on_press(Message::SettingsEvent(SettingsEvent::Close)),
    ]
    .align_y(Alignment::Center);

    let section_rule = || {
        rule::horizontal(1).style(|_: &iced::Theme| rule::Style {
            color: CRABOT_PRIMARY,
            fill_mode: rule::FillMode::Full,
            radius: 0.0.into(),
            snap: false,
        })
    };

    let section_header = |label: &'static str| {
        text(label)
            .size(13)
            .font(iced::Font {
                weight: iced::font::Weight::Bold,
                ..iced::Font::DEFAULT
            })
            .color(CRABOT_PRIMARY)
    };

    let providers_section = provider_tab_view(state, models);
    let labels_section = label_tab_view(state, models);

    let body = column![
        section_header("Model Providers"),
        section_rule(),
        providers_section,
        iced::widget::Space::new().height(8),
        section_header("Model Labels"),
        section_rule(),
        labels_section,
    ]
    .spacing(10);

    let scrollable_body = scrollable(body).width(Length::Fill);

    container(
        column![header, section_rule(), scrollable_body,]
            .spacing(8)
            .padding(20),
    )
    .style(|_: &iced::Theme| container::Style {
        background: Some(CRABOT_DIALOG_BG.into()),
        border: Border::default().rounded(CRABOT_DIALOG_RADIUS),
        ..container::Style::default()
    })
    .max_width(600)
    .max_height(800)
    .into()
}

// ── Provider section ────────────────────────────────────────────────

fn provider_tab_view<'a>(state: &'a SettingsState, models: &'a ModelList) -> Element<'a, Message> {
    let entries: Vec<ProviderPickEntry> = models
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
        let api_types = vec![
            "openai",
            "openai-completions",
            "anthropic",
            "gemini",
            "groq",
            "ollama",
            "deepseek",
        ];
        let selected_api_type = api_types
            .iter()
            .position(|&t| t == state.provider_api_type)
            .and_then(|i| api_types.get(i).copied());

        let auth_types = vec!["apiKey", "bearer", "basic", "none"];
        let selected_auth = auth_types
            .iter()
            .position(|&t| t == state.provider_auth)
            .and_then(|i| auth_types.get(i).copied());

        let form_body = column![
            field_row(
                "Name",
                &state.provider_name,
                |v| { Message::SettingsEvent(SettingsEvent::EditProviderName(v)) },
                "Provider name",
                Some(NEW_PROVIDER_NAME_INPUT_ID)
            ),
            field_row(
                "Base URL",
                &state.provider_base_url,
                |v| { Message::SettingsEvent(SettingsEvent::EditProviderBaseUrl(v)) },
                "",
                None
            ),
            label_pick_row("API Type", api_types, selected_api_type, |v| {
                Message::SettingsEvent(SettingsEvent::EditProviderApiType(v.to_string()))
            }),
            label_pick_row("Auth", auth_types, selected_auth, |v| {
                Message::SettingsEvent(SettingsEvent::EditProviderAuth(v.to_string()))
            }),
            field_row(
                "API Key",
                &state.provider_api_key,
                |v| { Message::SettingsEvent(SettingsEvent::EditProviderApiKey(v)) },
                "Enter API Key or its enviroment variable name",
                None
            ),
            checkbox_row("Strict Mode", state.provider_strict_mode, |v| {
                Message::SettingsEvent(SettingsEvent::ToggleProviderStrictMode(v))
            },),
            models_section_view(state, models),
            {
                let save_button: Element<'_, Message> = if state.provider_name.trim().is_empty() {
                    button(text("Save"))
                        .style(crate::views::styles::secondary_button)
                        .into()
                } else {
                    button(text("Save"))
                        .style(crate::views::styles::primary_button)
                        .on_press(Message::SettingsEvent(SettingsEvent::SaveProvider))
                        .into()
                };
                let secondary_button: Element<'_, Message> = if state.is_new_provider {
                    button(text("Cancel"))
                        .style(crate::views::styles::secondary_button)
                        .on_press(Message::SettingsEvent(SettingsEvent::CancelNewProvider))
                        .into()
                } else {
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
                };
                row![
                    iced::widget::Space::new().width(Length::Fill),
                    save_button,
                    secondary_button,
                ]
                .spacing(10)
                .width(Length::Fill)
            },
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

    column![
        row![
            picker,
            button(text("New"))
                .style(crate::views::styles::primary_button)
                .on_press(Message::SettingsEvent(SettingsEvent::NewProvider)),
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
fn label_tab_view<'a>(state: &'a SettingsState, models: &'a ModelList) -> Element<'a, Message> {
    let dragging = state.dragging_label();

    let mut chips: Vec<Element<'a, Message>> = models
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

    let hint = text("Drag labels to reorder · Click + to add a new label")
        .size(12)
        .color(CRABOT_TEXT_MUTED);

    let chip_row = scrollable(row(chips).spacing(8).align_y(Alignment::Center))
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default(),
        ))
        .width(Length::Fill);

    column![hint, chip_row].spacing(8).into()
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
) -> Element<'a, Message> {
    let mut input = iced::widget::text_input(placeholder, value)
        .on_input(on_input)
        .width(Length::Fill)
        .padding(4);
    if let Some(input_id) = id {
        input = input.id(input_id);
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
    options: Vec<&'static str>,
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
    if state.provider_base_url.is_empty() {
        return iced::widget::Space::new()
            .width(Length::Fill)
            .height(0)
            .into();
    }

    let provider_model_ids: HashSet<&str> = models
        .providers
        .get(&state.selected_provider_id)
        .map(|p| p.models.iter().map(|m| m.id.as_str()).collect())
        .unwrap_or_default();

    // Header: "Models" label
    let header = container(text("Models").size(14))
        .width(90)
        .align_x(iced::Alignment::End);

    // Body: status or table + details
    let body: Element<'_, Message> = if state.fetching_models {
        text("Loading models…")
            .size(12)
            .color(CRABOT_TEXT_MUTED)
            .into()
    } else if let Some(err) = &state.models_fetch_error {
        text(format!("Error: {err}"))
            .size(12)
            .color(CRABOT_DANGER)
            .into()
    } else if state.available_model_ids.is_empty() {
        text("No models available.")
            .size(12)
            .color(CRABOT_TEXT_MUTED)
            .into()
    } else {
        // ── Table column ──────────────────────────────────────────
        // Header row
        let table_header = row![iced::widget::Space::new().width(20), text("Model ID")]
            .spacing(8)
            .height(0);

        // Data rows
        let model_rows: Vec<Element<'_, Message>> = state
            .available_model_ids
            .iter()
            .map(|id| {
                let checked = provider_model_ids.contains(id.as_str());
                let id_clone = id.clone();
                let id_clone2 = id.clone();
                let is_selected = state.selected_model_id.as_deref() == Some(id);

                let cb = checkbox(checked)
                    .on_toggle(move |_| {
                        Message::SettingsEvent(SettingsEvent::ToggleModel(
                            id_clone.clone(),
                            !checked,
                        ))
                    })
                    .style(crate::views::primary_checkbox);

                let id_text = text(id.clone()).size(12);
                let id_inner = container(id_text).padding([2, 4]);
                let id_cell = if is_selected {
                    mouse_area(container(id_inner).style(move |_: &iced::Theme| {
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
                    }))
                    .on_press(Message::SettingsEvent(
                        SettingsEvent::SelectModelDetail(id_clone2),
                    ))
                } else {
                    mouse_area(id_inner).on_press(Message::SettingsEvent(
                        SettingsEvent::SelectModelDetail(id_clone2),
                    ))
                };

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
                let thinking_levels = model.thinking_levels.join(", ");
                let input_modes = model.input.join(", ");
                let context = if model.context_window > 0 {
                    model.context_window.to_string()
                } else {
                    "—".into()
                };
                let max_tok = if model.max_tokens > 0 {
                    model.max_tokens.to_string()
                } else {
                    "—".into()
                };
                let cost_in = format!("${:.4}/M", model.cost.input);
                let cost_out = format!("${:.4}/M", model.cost.output);
                let cost_cache_read = format!("${:.4}/M", model.cost.cache_read);
                let cost_cache_write = format!("${:.4}/M", model.cost.cache_write);

                let items = column![
                    detail_row(
                        "Name",
                        if model.name.is_empty() {
                            "—".into()
                        } else {
                            model.name.clone()
                        },
                    ),
                    detail_row(
                        "Thinking",
                        if model.thinking {
                            "yes".into()
                        } else {
                            "no".into()
                        },
                    ),
                    if !model.thinking_levels.is_empty() {
                        detail_row("Think Levels", thinking_levels)
                    } else {
                        iced::widget::Space::new()
                            .height(0)
                            .width(Length::Fill)
                            .into()
                    },
                    if !model.input.is_empty() {
                        detail_row("Input Modes", input_modes)
                    } else {
                        iced::widget::Space::new()
                            .height(0)
                            .width(Length::Fill)
                            .into()
                    },
                    detail_row("Context", context),
                    detail_row("Max Tokens", max_tok),
                    detail_row("Cost (in)", cost_in),
                    detail_row("Cost (out)", cost_out),
                    detail_row("Cache read", cost_cache_read),
                    detail_row("Cache write", cost_cache_write),
                ]
                .spacing(2);

                container(items)
                    .padding(8)
                    .style(form_card_style)
                    .width(Length::FillPortion(1))
                    .into()
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
    row![header, body].spacing(10).into()
}

/// Single label–value row for the model detail panel.
fn detail_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
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
