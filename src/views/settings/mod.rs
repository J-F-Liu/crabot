use super::icons;
use super::theme::{
    CRABOT_DANGER, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY, color_border, color_card, color_dialog_bg,
    color_muted, color_surface, color_text_strong,
};
use crate::widgets::textarea::TextArea;
use crabot::model::{ModelConfig, ModelList, Provider, TaskModels};
use crabot::model_database::ModelDatabase;
use crabot::tools::custom::{CustomTool, ToolList, ToolParameter};
use crabot::tools::mcp::{McpList, McpServer, McpTransport};
use iced::padding;
use iced::{
    Alignment, Border, Color, Element, Length,
    widget::{button, column, container, mouse_area, row, rule, scrollable, svg, text, text_input},
};
use indexmap::IndexMap;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

pub mod about;
pub mod ai_models;
pub mod builtin_tools;
pub mod custom_tools;
pub mod mcp_servers;
pub mod prompt_recipes;
pub mod tool_playground;

/// Widget id of the new-label text input — used to focus it and detect blur.
pub(crate) const NEW_LABEL_INPUT_ID: &str = "settings-new-label-input";
/// Widget id of the new-provider name input — used to focus it.
pub(crate) const NEW_PROVIDER_NAME_INPUT_ID: &str = "settings-new-provider-name-input";

/// Bold font for headings.
pub(super) const BOLD: iced::Font = iced::Font {
    weight: iced::font::Weight::Bold,
    ..iced::Font::DEFAULT
};

// ── Tabs ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "title_case")]
pub(crate) enum SettingsTab {
    #[strum(serialize = "AI Models")]
    AiModels,
    PromptRecipes,
    BuiltinTools,
    CustomTools,
    #[strum(serialize = "MCP Servers")]
    McpServers,
    ToolPlayground,
    About,
}

// ── Events ──────────────────────────────────────────────────────────

/// Top-level settings-dialog event: tab navigation plus one sub-event per
/// tab, each defined in its own tab module.
#[derive(Debug, Clone)]
pub(crate) enum SettingsEvent {
    SelectTab(SettingsTab),
    Close,
    /// Events for the AI Models tab (providers, models, labels).
    Models(ai_models::ModelsEvent),
    /// Events for the Prompt Recipes tab.
    Recipes(prompt_recipes::RecipesEvent),
    /// Events for the Builtin Tools tab (agent limits, tool limits, task models).
    BuiltinTools(builtin_tools::BuiltinToolsEvent),
    /// Events for the Custom Tools tab.
    CustomTools(custom_tools::CustomToolsEvent),
    /// Events for the MCP Servers tab.
    Mcp(mcp_servers::McpEvent),
    /// Events for the Tool Playground tab.
    Playground(tool_playground::PlaygroundEvent),
    /// Events for the About tab.
    About(about::AboutEvent),
}

// ── State ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct SettingsState {
    /// Whether the settings dialog is currently open.
    pub(crate) open: bool,
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
    /// Raw-text drafts backing the checked-model parameter editor.
    pub(super) model_edit: Option<ai_models::ModelEditDraft>,
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
    /// Working copy of prompt recipes edited within the dialog — saved on Save.
    pub(crate) working_prompt_recipes: indexmap::IndexMap<String, Vec<String>>,
    /// Index of the work-mode recipe card currently expanded, if any.
    pub(super) expanded_recipe_mode: Option<usize>,
    /// Working copy of max agent-loop iterations (raw text) — parsed on Save.
    pub(crate) working_max_iterations: String,
    /// Working copy of the context-fill renew threshold, percent (raw text) — parsed on Save.
    pub(crate) working_fill_ratio_threshold: String,
    /// Working copy of the stream stall timeout (s, raw text) — parsed on Save.
    pub(crate) working_stream_stall_timeout: String,
    /// Working copies of built-in tool limits (raw text) — parsed on Save.
    pub(crate) working_tool_limits: builtin_tools::ToolLimitStrings,
    /// Working copy of sub-agent task models — saved on Save (Builtin Tools tab).
    pub(crate) working_task_models: TaskModels,
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
    // ── Tool Playground state ────────────────────────────────
    /// Snapshot of all registered tools for the playground picker.
    pub(crate) playground_tools: Vec<tool_playground::ToolInfo>,
    /// Index of the currently selected tool, if any.
    pub(crate) playground_selected_index: Option<usize>,
    /// Per-parameter text values for the selected tool's parameter form.
    pub(crate) playground_param_values: std::collections::HashMap<String, String>,
    /// Last execution result (Ok or Err), if any.
    pub(crate) playground_result: Option<Result<String, String>>,
    /// Whether a playground tool is currently executing.
    pub(crate) playground_running: bool,
    /// Cancellation token for in-progress playground execution.
    pub(crate) playground_cancel: CancellationToken,
    /// Monotonically incrementing generation counter — used to discard stale
    /// [`PlaygroundToolResult`] messages from cancelled or superseded executions.
    pub(crate) playground_generation: u64,
    // ── About state ────────────────────────────────────────
    /// Current state of the update check.
    pub(crate) update_check: about::UpdateCheck,
    /// Whether auto-check-updates is enabled.
    pub(crate) auto_check_updates: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            open: false,
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
            model_edit: None,
            new_label_name: String::new(),
            adding_label: false,
            drag_label: None,
            drag_reordered: false,
            model_db: ModelDatabase::default(),
            selected_offer_source: None,
            working_models: ModelList::default(),
            working_prompt_recipes: indexmap::IndexMap::new(),
            expanded_recipe_mode: None,
            working_max_iterations: String::new(),
            working_fill_ratio_threshold: String::new(),
            working_stream_stall_timeout: String::new(),
            working_tool_limits: builtin_tools::ToolLimitStrings::default(),
            working_task_models: TaskModels::default(),
            working_tools: ToolList::default(),
            expanded_tool: None,
            tool_desc_area: TextArea::new(),
            tool_instr_area: TextArea::new(),
            working_mcp: McpList::default(),
            expanded_mcp: None,
            mcp_prompt_area: TextArea::new(),
            save_feedback: None,
            update_check: about::UpdateCheck::Idle,
            auto_check_updates: true,
            playground_tools: Vec::new(),
            playground_selected_index: None,
            playground_param_values: std::collections::HashMap::new(),
            playground_result: None,
            playground_running: false,
            playground_cancel: CancellationToken::new(),
            playground_generation: 0,
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
        self.model_edit = None;
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

    /// Load tool snapshots from the live registry for the playground picker.
    /// Resets selection, parameters, result, and cancels any in-flight execution.
    pub(crate) fn load_playground_tools(&mut self, tools: Vec<tool_playground::ToolInfo>) {
        self.refresh_playground_tools(tools);
        self.playground_selected_index = None;
        self.playground_param_values.clear();
        self.playground_result = None;
        self.playground_running = false;
        // Cancel any in-flight execution and bump generation.
        self.reset_playground_cancel();
        self.playground_generation = self.playground_generation.wrapping_add(1);
    }

    /// Cancel the in-flight playground execution and hand out a fresh token.
    fn reset_playground_cancel(&mut self) {
        self.playground_cancel.cancel();
        self.playground_cancel = CancellationToken::new();
    }

    /// Refresh the playground tool list without resetting user selection or parameters.
    pub(crate) fn refresh_playground_tools(&mut self, tools: Vec<tool_playground::ToolInfo>) {
        self.playground_tools = tools;
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

    /// Trim the pending new-label name and insert if non-empty and not already present.
    fn commit_new_label(&mut self) {
        let name = self.new_label_name.trim().to_string();
        self.new_label_name.clear();
        if !name.is_empty() && !self.working_models.models.contains_key(&name) {
            self.working_models
                .models
                .insert(name, ModelConfig::default());
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

    /// Handle a `SettingsEvent`, dispatching it to the active tab's handler.
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
            SettingsEvent::Models(e) => ai_models::update(self, e),
            SettingsEvent::Recipes(e) => prompt_recipes::update(self, e),
            SettingsEvent::BuiltinTools(e) => builtin_tools::update(self, e),
            SettingsEvent::CustomTools(e) => custom_tools::update(self, e),
            SettingsEvent::Mcp(e) => mcp_servers::update(self, e),
            SettingsEvent::Playground(e) => tool_playground::update(self, e),
            SettingsEvent::About(e) => about::update(self, e),
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

    /// Load prompt recipes into the dialog's working copy (on dialog open).
    pub(crate) fn load_prompt_recipes(&mut self, recipes: indexmap::IndexMap<String, Vec<String>>) {
        self.working_prompt_recipes = recipes;
        self.expanded_recipe_mode = None;
    }

    /// Load builtin-tool settings into the dialog's working copies (on dialog open).
    pub(crate) fn load_builtin_tools(&mut self, settings: &crabot::settings::Settings) {
        self.working_max_iterations = settings.max_iterations.to_string();
        self.working_fill_ratio_threshold = settings.fill_ratio_threshold.to_string();
        self.working_stream_stall_timeout = settings.stream_stall_timeout.to_string();
        self.working_tool_limits =
            builtin_tools::ToolLimitStrings::from_limits(&settings.tool_limits);
        self.working_task_models = settings.task_models.clone();
    }

    /// Apply the parsed agent limits and task models to `settings`.
    pub(crate) fn apply_builtin_tools(&self, settings: &mut crabot::settings::Settings) {
        settings.max_iterations = self.parsed_max_iterations();
        settings.fill_ratio_threshold = self.parsed_fill_ratio_threshold();
        settings.stream_stall_timeout = self.parsed_stream_stall_timeout();
        settings.tool_limits = self.parsed_tool_limits();
        settings.task_models = self.working_task_models.clone();
    }

    /// Parsed max agent-loop iterations, falling back to the current default.
    pub(crate) fn parsed_max_iterations(&self) -> usize {
        builtin_tools::parse_num(&self.working_max_iterations, 100, None, false)
    }

    /// Parsed context-fill renew threshold (percent), clamped to (0, 100].
    pub(crate) fn parsed_fill_ratio_threshold(&self) -> f32 {
        builtin_tools::parse_num(&self.working_fill_ratio_threshold, 25.0, Some(100.0), false)
    }

    /// Parsed stream stall timeout in seconds (0 = off), clamped to 1h.
    pub(crate) fn parsed_stream_stall_timeout(&self) -> u64 {
        builtin_tools::parse_num(&self.working_stream_stall_timeout, 120, Some(3600), true)
    }

    /// Parsed tool limits, falling back per-field to the defaults.
    pub(crate) fn parsed_tool_limits(&self) -> crabot::tools::ToolLimits {
        self.working_tool_limits.to_limits()
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

// ── View ────────────────────────────────────────────────────────────

/// Returns the settings dialog content with a left sidebar of vertical tabs
/// and a content area that switches between tab pages.
/// The caller is responsible for placing it inside a modal structure.
pub(crate) fn settings_dialog<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = container(
        row![
            text("Settings").size(18).font(BOLD).color(CRABOT_PRIMARY),
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
            .on_press(SettingsEvent::Close),
        ]
        .align_y(Alignment::Center),
    );

    // ── Sidebar ────────────────────────────────────────────────────
    let tabs = [
        SettingsTab::AiModels,
        SettingsTab::PromptRecipes,
        SettingsTab::BuiltinTools,
        SettingsTab::CustomTools,
        SettingsTab::McpServers,
        SettingsTab::ToolPlayground,
        SettingsTab::About,
    ];
    let sidebar_buttons: Vec<Element<'a, SettingsEvent>> = tabs
        .iter()
        .map(|&tab| {
            let is_active = state.selected_tab == tab;
            button(text(tab.to_string()).size(13))
                .width(Length::Fill)
                .style(sidebar_tab_style(is_active))
                .on_press(SettingsEvent::SelectTab(tab))
                .into()
        })
        .collect();

    let sidebar = container(column(sidebar_buttons).spacing(2).padding([8, 0]))
        .width(160)
        .height(Length::Fill)
        .style(|_: &iced::Theme| container::Style {
            background: Some(color_surface().into()),
            border: Border::default().rounded(CRABOT_DIALOG_RADIUS),
            ..container::Style::default()
        });

    // ── Tab content ────────────────────────────────────────────────
    let tab_content: Element<'a, SettingsEvent> = match state.selected_tab {
        SettingsTab::AiModels => ai_models::ai_models_page(state),
        SettingsTab::PromptRecipes => prompt_recipes::prompt_recipes_page(state),
        SettingsTab::BuiltinTools => builtin_tools::builtin_tools_page(state),
        SettingsTab::CustomTools => custom_tools::custom_tools_page(state),
        SettingsTab::McpServers => mcp_servers::mcp_servers_page(state),
        SettingsTab::ToolPlayground => tool_playground::playground_page(state),
        SettingsTab::About => about::about_page(state),
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
        background: Some(color_dialog_bg().into()),
        border: Border::default().rounded(CRABOT_DIALOG_RADIUS),
        ..container::Style::default()
    })
    .max_width(900)
    .max_height(800)
    .into()
}

pub(super) fn form_card_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(
            if crate::views::theme::is_dark() {
                color_card()
            } else {
                Color::from_rgb8(0xF4, 0xF4, 0xF4)
            }
            .into(),
        ),
        border: Border::default().rounded(8).width(1).color(color_border()),
        ..container::Style::default()
    }
}

pub(super) fn section_rule() -> Element<'static, SettingsEvent> {
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

/// Build the bottom-right "Save" action row, showing "Saved ✓" briefly after `on_save`.
pub(super) fn save_action_row<'a>(
    state: &'a SettingsState,
    tab: SettingsTab,
    on_save: SettingsEvent,
) -> Element<'a, SettingsEvent> {
    let label = if state.save_feedback == Some(tab) {
        "Saved ✓"
    } else {
        "Save"
    };
    let save_button = button(text(label).size(13))
        .style(crate::views::styles::primary_button)
        .on_press(on_save);

    row![iced::widget::Space::new().width(Length::Fill), save_button]
        .spacing(10)
        .padding(padding::top(8))
        .into()
}

/// A labelled single-line text input row used by the settings forms.
pub(super) fn field_row<'a>(
    label: &'static str,
    value: &'a str,
    placeholder: &'a str,
    mono: bool,
    id: Option<&'static str>,
    on_submit: Option<SettingsEvent>,
    on_input: impl Fn(String) -> SettingsEvent + 'a,
) -> Element<'a, SettingsEvent> {
    let mut input = text_input(placeholder, value)
        .on_input(on_input)
        .width(Length::Fill)
        .padding(4)
        .size(13);
    if mono {
        input = input.font(iced::Font::MONOSPACE);
    }
    if let Some(id) = id {
        input = input.id(id);
    }
    if let Some(msg) = on_submit {
        input = input.on_submit(msg);
    }
    let label_col = container(text(label).size(14))
        .width(90)
        .align_x(Alignment::End);
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
    on_action: impl Fn(crate::widgets::textarea::Message) -> SettingsEvent + 'a,
) -> Element<'a, SettingsEvent> {
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
pub(super) fn card_rule() -> Element<'static, SettingsEvent> {
    rule::horizontal(1)
        .style(|_: &iced::Theme| rule::Style {
            color: color_border(),
            fill_mode: rule::FillMode::Full,
            radius: 0.0.into(),
            snap: false,
        })
        .into()
}

/// Sub-card used for nested editors inside a form card.
pub(super) fn sub_card_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(color_card().into()),
        border: Border::default().rounded(6).width(1).color(color_border()),
        ..container::Style::default()
    }
}

/// Subtle "✕" button — muted normally, red on hover.
pub(super) fn delete_button_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => CRABOT_DANGER,
            _ => color_muted(),
        },
        ..button::Style::default()
    }
}

/// Bold primary-colored section heading.
pub(super) fn section_header(title: &'static str) -> Element<'static, SettingsEvent> {
    text(title).size(13).font(BOLD).color(CRABOT_PRIMARY).into()
}

/// Muted hint paragraph for an empty list.
pub(super) fn empty_hint(message: &'static str) -> Element<'static, SettingsEvent> {
    container(text(message).size(12).color(color_muted()))
        .padding(16)
        .center_x(Length::Fill)
        .into()
}

/// Clickable card header: expand arrow, bold title, muted summary.
pub(super) fn collapsible_header<'a>(
    expanded: bool,
    title: String,
    summary: String,
    on_toggle: SettingsEvent,
) -> Element<'a, SettingsEvent> {
    mouse_area(
        container(
            row![
                text(if expanded { "▼" } else { "⯈" })
                    .size(10)
                    .color(color_muted())
                    .width(14),
                text(title).size(13).font(BOLD),
                text(summary).size(11).color(color_muted()),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill),
    )
    .on_press(on_toggle)
    .into()
}

/// Section with a right-aligned label, a "+ Add" button, and optional
/// indented sub-cards beneath.
pub(super) fn add_section<'a>(
    label: &'static str,
    on_add: SettingsEvent,
    cards: Vec<Element<'a, SettingsEvent>>,
) -> Element<'a, SettingsEvent> {
    let header = row![
        container(text(label).size(14))
            .width(90)
            .align_x(Alignment::End),
        button(text("+ Add").size(12))
            .padding([4, 10])
            .style(crate::views::styles::secondary_button)
            .on_press(on_add),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if cards.is_empty() {
        return column![header].spacing(6).into();
    }
    column![
        header,
        row![
            iced::widget::Space::new().width(100),
            column(cards).spacing(6).width(Length::Fill),
        ],
    ]
    .spacing(6)
    .into()
}

/// `{n} {word}` with an `s` plural when `n != 1`.
pub(super) fn count_label(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// First `{base}`, `{base}_2`, `{base}_3`, … name not already `taken`.
pub(super) fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut name = base.to_string();
    let mut suffix = 2;
    while taken(&name) {
        name = format!("{base}_{suffix}");
        suffix += 1;
    }
    name
}

/// First `{prefix}{n}` name (n from `start` up) not already `taken`.
pub(super) fn numbered_name(prefix: &str, start: usize, taken: impl Fn(&str) -> bool) -> String {
    let mut n = start;
    loop {
        let name = format!("{prefix}{n}");
        if !taken(&name) {
            return name;
        }
        n += 1;
    }
}

/// Toggle an expanded-card index: `Some(index)` ↔ `None`.
pub(super) fn toggle_expanded(expanded: &mut Option<usize>, index: usize) {
    *expanded = if *expanded == Some(index) {
        None
    } else {
        Some(index)
    };
}

/// Shift the expanded index after removing `index`; `None` when it was removed.
pub(super) fn remove_expanded(expanded: Option<usize>, index: usize) -> Option<usize> {
    match expanded {
        Some(i) if i == index => None,
        Some(i) if i > index => Some(i - 1),
        other => other,
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
                text_color: color_text_strong(),
                border: Border::default().rounded(6),
                ..button::Style::default()
            }
        }
    }
}
