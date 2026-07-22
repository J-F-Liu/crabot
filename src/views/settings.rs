use super::icons;
use super::theme::{
    CRABOT_BORDER, CRABOT_DANGER, CRABOT_DIALOG_BG, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY,
    CRABOT_SURFACE, CRABOT_TEXT, CRABOT_TEXT_MUTED,
};
use crate::Message;
use crate::widgets::textarea::TextArea;
use crabot::model::{Model, ModelConfig, ModelList, Provider};
use crabot::model_database::ModelDatabase;
use crabot::tools::custom::{CustomTool, ParameterType, ToolList, ToolParameter};
use crabot::tools::mcp::{McpList, McpServer, McpTransport};
use iced::padding;
use iced::{
    Alignment, Border, Color, Element, Length,
    widget::{button, column, container, row, rule, scrollable, svg, text, text_input},
};
use indexmap::IndexMap;
use std::collections::HashMap;

pub mod ai_models;
pub mod custom_tools;
pub mod mcp_servers;

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

/// Identifies which text field in the custom-tool form is being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolTextField {
    Description,
    Instruction,
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
    // Custom tool actions
    /// Expand/collapse the tool card at the given index.
    ToggleTool(usize),
    /// Append a new blank tool and expand its card.
    NewTool,
    DeleteTool(usize),
    EditToolName(usize, String),
    EditToolCommand(usize, String),
    AddToolParam(usize),
    DeleteToolParam(usize, usize),
    EditParamName(usize, usize, String),
    EditParamKind(usize, usize, String),
    EditParamDescription(usize, usize, String),
    ToggleParamRequired(usize, usize, bool),
    SaveModels,
    SaveTools,
    /// A [`TextArea`] edit in the custom-tool form.
    ToolTextArea(ToolTextField, crate::widgets::textarea::Message),
    // MCP server actions
    /// Expand/collapse the MCP server card at the given index.
    ToggleMcp(usize),
    /// Append a new blank MCP server and expand its card.
    NewMcp,
    DeleteMcp(usize),
    EditMcpName(usize, String),
    /// Switch a server's transport kind ("stdio" or "http").
    EditMcpTransport(usize, String),
    /// Edit the spawn command of a stdio server.
    EditMcpCmd(usize, String),
    /// Edit the URL of an HTTP server.
    EditMcpUrl(usize, String),
    ToggleMcpQualify(usize, bool),
    /// Add a key/value entry to the active transport's option map
    /// (env vars for stdio servers, HTTP headers for http servers).
    AddMcpMapEntry(usize),
    DeleteMcpMapEntry(usize, usize),
    EditMcpMapKey(usize, usize, String),
    EditMcpMapValue(usize, usize, String),
    SaveMcp,
    /// A [`TextArea`] edit in the MCP server prompt form.
    McpTextArea(crate::widgets::textarea::Message),
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
    /// Working copy of custom tools edited within the dialog — saved on Save.
    pub(crate) working_tools: ToolList,
    /// Index of the custom-tool card currently expanded, if any.
    pub(super) expanded_tool: Option<usize>,
    /// `TextArea` for the description of the currently expanded tool.
    pub(super) tool_desc_area: TextArea,
    /// `TextArea` for the instruction of the currently expanded tool.
    pub(super) tool_instr_area: TextArea,
    /// Working copy of MCP servers edited within the dialog — saved on Save.
    pub(crate) working_mcp: McpList,
    /// Index of the MCP server card currently expanded, if any.
    pub(super) expanded_mcp: Option<usize>,
    /// `TextArea` for the prompt of the currently expanded MCP server.
    pub(super) mcp_prompt_area: TextArea,
    /// Which tab just saved — drives the "Saved ✓" button label.
    pub(super) save_feedback: Option<SettingsTab>,
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
            working_tools: ToolList::default(),
            expanded_tool: None,
            tool_desc_area: TextArea::new(),
            tool_instr_area: TextArea::new(),
            working_mcp: McpList::default(),
            expanded_mcp: None,
            mcp_prompt_area: TextArea::new(),
            save_feedback: None,
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

    /// Load custom tools into the dialog's working copy (on dialog open).
    pub(crate) fn load_tools(&mut self, tools: ToolList) {
        self.working_tools = tools;
        self.expanded_tool = None;
        self.tool_desc_area = TextArea::new();
        self.tool_instr_area = TextArea::new();
    }

    /// Load MCP servers into the dialog's working copy (on dialog open).
    pub(crate) fn load_mcp(&mut self, servers: McpList) {
        self.working_mcp = servers;
        self.expanded_mcp = None;
        self.mcp_prompt_area = TextArea::new();
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
        self.save_feedback = None;
        match event {
            SettingsEvent::SelectTab(tab) => {
                self.selected_tab = tab;
            }
            SettingsEvent::Close => {
                // Drop any in-progress label editing / dragging.
                self.adding_label = false;
                self.drag_label = None;
            }
            SettingsEvent::SaveModels => {
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
                self.save_feedback = Some(SettingsTab::AiModels);
            }
            SettingsEvent::SaveTools => {
                // Flush any pending TextArea edits to tool structs.
                self.flush_tool_text_areas();
                // Drop custom tools left with a blank name — they cannot be invoked.
                self.working_tools
                    .custom_tools
                    .retain(|t| !t.name.trim().is_empty());
                // Trim leading/trailing whitespace from remaining tool names.
                for t in &mut self.working_tools.custom_tools {
                    t.name = t.name.trim().to_string();
                }
                self.save_feedback = Some(SettingsTab::CustomTools);
            }
            SettingsEvent::SaveMcp => {
                // Flush any pending TextArea edits to server structs.
                self.flush_mcp_text_area();
                // Drop servers left with a blank name — they cannot be connected.
                self.working_mcp
                    .servers
                    .retain(|s| !s.name.trim().is_empty());
                for s in &mut self.working_mcp.servers {
                    s.name = s.name.trim().to_string();
                    // Drop key/value entries with a blank key.
                    match &mut s.transport {
                        McpTransport::Stdio { env_vars, .. } => {
                            env_vars.retain(|k, _| !k.trim().is_empty());
                        }
                        McpTransport::Http { headers, .. } => {
                            headers.retain(|k, _| !k.trim().is_empty());
                        }
                    }
                }
                // Deduplicate server names — keep the first occurrence of each name.
                // Duplicate names would corrupt the connection map and enable state.
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                self.working_mcp
                    .servers
                    .retain(|s| seen.insert(s.name.clone()));
                // Collapse the card — indices may have shifted after pruning.
                self.expanded_mcp = None;
                self.save_feedback = Some(SettingsTab::McpServers);
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
                // Auto-flush new provider so it exists in working_models.
                if self.is_new_provider {
                    // create provider id and set is_new_provider to false
                    self.flush_current_provider();
                }
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
                                let name = id.clone();
                                Model {
                                    id,
                                    name,
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
                self.models_fetch_error = None;
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
            // ── Custom tool actions ────────────────────────────────
            SettingsEvent::ToggleTool(index) => {
                self.flush_tool_text_areas();
                self.expanded_tool = if self.expanded_tool == Some(index) {
                    None
                } else {
                    Some(index)
                };
                self.init_tool_text_areas();
            }
            SettingsEvent::NewTool => {
                self.flush_tool_text_areas();
                let base = "new_tool";
                let mut name = base.to_string();
                let mut suffix = 2;
                while self
                    .working_tools
                    .custom_tools
                    .iter()
                    .any(|t| t.name == name)
                {
                    name = format!("{base}_{suffix}");
                    suffix += 1;
                }
                self.working_tools.custom_tools.push(CustomTool {
                    name,
                    description: String::new(),
                    instruction: String::new(),
                    parameters: vec![],
                    command: String::new(),
                });
                self.expanded_tool = Some(self.working_tools.custom_tools.len() - 1);
                self.init_tool_text_areas();
            }
            SettingsEvent::DeleteTool(index) => {
                self.flush_tool_text_areas();
                if index < self.working_tools.custom_tools.len() {
                    self.working_tools.custom_tools.remove(index);
                }
                self.expanded_tool = match self.expanded_tool {
                    Some(i) if i == index => None,
                    Some(i) if i > index => Some(i - 1),
                    other => other,
                };
                self.init_tool_text_areas();
            }
            SettingsEvent::EditToolName(index, v) => {
                if let Some(t) = self.tool_mut(index) {
                    t.name = v;
                }
            }
            SettingsEvent::EditToolCommand(index, v) => {
                if let Some(t) = self.tool_mut(index) {
                    t.command = v;
                }
            }
            SettingsEvent::AddToolParam(index) => {
                if let Some(t) = self.tool_mut(index) {
                    let mut n = t.parameters.len() + 1;
                    let mut name = format!("param{n}");
                    while t.parameters.iter().any(|p| p.name == name) {
                        n += 1;
                        name = format!("param{n}");
                    }
                    t.parameters.push(ToolParameter {
                        name,
                        kind: ParameterType::String,
                        description: String::new(),
                        required: true,
                    });
                }
            }
            SettingsEvent::DeleteToolParam(tool_index, index) => {
                if let Some(t) = self.tool_mut(tool_index)
                    && index < t.parameters.len()
                {
                    t.parameters.remove(index);
                }
            }
            SettingsEvent::EditParamName(tool_index, index, v) => {
                if let Some(p) = self.param_mut(tool_index, index) {
                    p.name = v;
                }
            }
            SettingsEvent::EditParamKind(tool_index, index, kind) => {
                if let Some(p) = self.param_mut(tool_index, index) {
                    p.kind = match kind.as_str() {
                        "integer" => ParameterType::Integer,
                        "number" => ParameterType::Number,
                        "boolean" => ParameterType::Boolean,
                        _ => ParameterType::String,
                    };
                }
            }
            SettingsEvent::EditParamDescription(tool_index, index, v) => {
                if let Some(p) = self.param_mut(tool_index, index) {
                    p.description = v;
                }
            }
            SettingsEvent::ToggleParamRequired(tool_index, index, v) => {
                if let Some(p) = self.param_mut(tool_index, index) {
                    p.required = v;
                }
            }
            SettingsEvent::ToolTextArea(field, msg) => match field {
                ToolTextField::Description => self.tool_desc_area.update(msg, false),
                ToolTextField::Instruction => self.tool_instr_area.update(msg, false),
            },
            // ── MCP server actions ────────────────────────────────
            SettingsEvent::ToggleMcp(index) => {
                self.flush_mcp_text_area();
                self.expanded_mcp = if self.expanded_mcp == Some(index) {
                    None
                } else {
                    Some(index)
                };
                self.init_mcp_text_area();
            }
            SettingsEvent::NewMcp => {
                self.flush_mcp_text_area();
                let base = "new_server";
                let mut name = base.to_string();
                let mut suffix = 2;
                while self.working_mcp.servers.iter().any(|s| s.name == name) {
                    name = format!("{base}_{suffix}");
                    suffix += 1;
                }
                self.working_mcp.servers.push(McpServer {
                    name,
                    transport: McpTransport::Stdio {
                        cmd: String::new(),
                        env_vars: IndexMap::new(),
                    },
                    qualify_tool_names: false,
                    prompt: String::new(),
                });
                self.expanded_mcp = Some(self.working_mcp.servers.len() - 1);
                self.init_mcp_text_area();
            }
            SettingsEvent::DeleteMcp(index) => {
                self.flush_mcp_text_area();
                if index < self.working_mcp.servers.len() {
                    self.working_mcp.servers.remove(index);
                }
                self.expanded_mcp = match self.expanded_mcp {
                    Some(i) if i == index => None,
                    Some(i) if i > index => Some(i - 1),
                    other => other,
                };
                self.init_mcp_text_area();
            }
            SettingsEvent::EditMcpName(index, v) => {
                if let Some(s) = self.mcp_mut(index) {
                    s.name = v;
                }
            }
            SettingsEvent::EditMcpTransport(index, kind) => {
                if let Some(s) = self.mcp_mut(index) {
                    let new_transport = match (kind.as_str(), &s.transport) {
                        ("http", McpTransport::Stdio { .. }) => Some(McpTransport::Http {
                            url: String::new(),
                            headers: IndexMap::new(),
                        }),
                        ("stdio", McpTransport::Http { .. }) => Some(McpTransport::Stdio {
                            cmd: String::new(),
                            env_vars: IndexMap::new(),
                        }),
                        _ => None,
                    };
                    if let Some(transport) = new_transport {
                        s.transport = transport;
                    }
                }
            }
            SettingsEvent::EditMcpCmd(index, v) => {
                if let Some(s) = self.mcp_mut(index)
                    && let McpTransport::Stdio { cmd, .. } = &mut s.transport
                {
                    *cmd = v;
                }
            }
            SettingsEvent::EditMcpUrl(index, v) => {
                if let Some(s) = self.mcp_mut(index)
                    && let McpTransport::Http { url, .. } = &mut s.transport
                {
                    *url = v;
                }
            }
            SettingsEvent::ToggleMcpQualify(index, v) => {
                if let Some(s) = self.mcp_mut(index) {
                    s.qualify_tool_names = v;
                }
            }
            SettingsEvent::AddMcpMapEntry(index) => {
                if let Some(s) = self.mcp_mut(index) {
                    let (map, base) = match &mut s.transport {
                        McpTransport::Stdio { env_vars, .. } => (env_vars, "KEY"),
                        McpTransport::Http { headers, .. } => (headers, "HEADER"),
                    };
                    let mut n = map.len() + 1;
                    let mut key = format!("{base}{n}");
                    while map.contains_key(&key) {
                        n += 1;
                        key = format!("{base}{n}");
                    }
                    map.insert(key, String::new());
                }
            }
            SettingsEvent::DeleteMcpMapEntry(server_index, index) => {
                if let Some(map) = self.mcp_map_mut(server_index) {
                    map.shift_remove_index(index);
                }
            }
            SettingsEvent::EditMcpMapKey(server_index, index, new_key) => {
                // Rename the key in place, keeping the entry's position so
                // the row (and its input focus) doesn't jump while typing.
                // Renames that would collide with an existing key are ignored.
                if let Some(map) = self.mcp_map_mut(server_index)
                    && index < map.len()
                    && !map.contains_key(&new_key)
                    && let Some((_, value)) = map.shift_remove_index(index)
                {
                    let last = map.len();
                    map.insert(new_key, value);
                    map.move_index(last, index);
                }
            }
            SettingsEvent::EditMcpMapValue(server_index, index, v) => {
                if let Some(map) = self.mcp_map_mut(server_index)
                    && let Some((_, value)) = map.get_index_mut(index)
                {
                    *value = v;
                }
            }
            SettingsEvent::McpTextArea(msg) => self.mcp_prompt_area.update(msg, false),
        }
    }

    /// Borrow the custom tool at `index` for in-place editing.
    fn tool_mut(&mut self, index: usize) -> Option<&mut CustomTool> {
        self.working_tools.custom_tools.get_mut(index)
    }

    /// Borrow one parameter of a custom tool for in-place editing.
    fn param_mut(&mut self, tool_index: usize, index: usize) -> Option<&mut ToolParameter> {
        self.working_tools
            .custom_tools
            .get_mut(tool_index)
            .and_then(|t| t.parameters.get_mut(index))
    }

    /// Flush TextArea content back to the currently expanded tool.
    fn flush_tool_text_areas(&mut self) {
        if let Some(i) = self.expanded_tool
            && let Some(tool) = self.working_tools.custom_tools.get_mut(i)
        {
            tool.description = self.tool_desc_area.text();
            tool.instruction = self.tool_instr_area.text();
        }
    }

    /// Initialize TextArea content from the currently expanded tool.
    fn init_tool_text_areas(&mut self) {
        if let Some(i) = self.expanded_tool
            && let Some(tool) = self.working_tools.custom_tools.get(i)
        {
            self.tool_desc_area.set_text(&tool.description);
            self.tool_instr_area.set_text(&tool.instruction);
        }
    }

    /// Borrow the MCP server at `index` for in-place editing.
    fn mcp_mut(&mut self, index: usize) -> Option<&mut McpServer> {
        self.working_mcp.servers.get_mut(index)
    }

    /// Borrow the active transport's option map (env vars or HTTP headers)
    /// of the MCP server at `index` for in-place editing.
    fn mcp_map_mut(&mut self, index: usize) -> Option<&mut IndexMap<String, String>> {
        self.working_mcp
            .servers
            .get_mut(index)
            .map(|s| match &mut s.transport {
                McpTransport::Stdio { env_vars, .. } => env_vars,
                McpTransport::Http { headers, .. } => headers,
            })
    }

    /// Flush TextArea content back to the currently expanded MCP server.
    fn flush_mcp_text_area(&mut self) {
        if let Some(i) = self.expanded_mcp
            && let Some(server) = self.working_mcp.servers.get_mut(i)
        {
            server.prompt = self.mcp_prompt_area.text();
        }
    }

    /// Initialize TextArea content from the currently expanded MCP server.
    fn init_mcp_text_area(&mut self) {
        if let Some(i) = self.expanded_mcp
            && let Some(server) = self.working_mcp.servers.get(i)
        {
            self.mcp_prompt_area.set_text(&server.prompt);
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
        SettingsTab::CustomTools => custom_tools::custom_tools_page(state),
        SettingsTab::McpServers => mcp_servers::mcp_servers_page(state),
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

pub(super) fn form_card_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(0xF4, 0xF4, 0xF4).into()),
        border: Border::default().rounded(8).width(1).color(CRABOT_BORDER),
        ..container::Style::default()
    }
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

// ── Shared form helpers ────────────────────────────────────────────

/// A labelled single-line text input row used by the settings forms.
pub(super) fn field_row<'a>(
    label: &'static str,
    value: &'a str,
    placeholder: &'a str,
    mono: bool,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    let label_col = container(text(label).size(14))
        .width(90)
        .align_x(Alignment::End);
    let mut input = text_input(placeholder, value)
        .on_input(on_input)
        .width(Length::Fill)
        .padding(4)
        .size(13);
    if mono {
        input = input.font(iced::Font::MONOSPACE);
    }
    row![label_col, input]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

/// A labelled multi-line [`TextArea`] row for editing longer text fields.
pub(super) fn textarea_field_row<'a>(
    label: &'static str,
    area: &'a TextArea,
    placeholder: &'a str,
    on_action: impl Fn(crate::widgets::textarea::Message) -> Message + 'a,
) -> Element<'a, Message> {
    let label_col = container(text(label).size(14))
        .width(90)
        .align_x(Alignment::End)
        .align_y(Alignment::Start)
        .padding(padding::top(4));
    let editor = area
        .view(on_action)
        .placeholder(placeholder)
        .height(Length::Fixed(64.0));
    row![label_col, container(editor).width(Length::Fill)]
        .spacing(10)
        .align_y(Alignment::Start)
        .into()
}

/// Thin separator between a card header and the expanded form.
pub(super) fn card_rule() -> Element<'static, Message> {
    rule::horizontal(1)
        .style(|_: &iced::Theme| rule::Style {
            color: CRABOT_BORDER,
            fill_mode: rule::FillMode::Full,
            radius: 0.0.into(),
            snap: false,
        })
        .into()
}

/// White sub-card used for nested editors inside a form card.
pub(super) fn sub_card_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        border: Border::default().rounded(6).width(1).color(CRABOT_BORDER),
        ..container::Style::default()
    }
}

/// Subtle "✕" button — muted normally, red on hover.
pub(super) fn delete_button_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => CRABOT_DANGER,
            _ => CRABOT_TEXT_MUTED,
        },
        ..button::Style::default()
    }
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
