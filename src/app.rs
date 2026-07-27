//! Application state, message routing, and the Iced Program implementation.
//!
//! This module owns the root [`App`] state, the nested domain [`Message`] enum,
//! and the boot / update / view / subscription methods that drive the GUI.

use crabot::{model, setup, tools, workspace};

use iced::widget::scrollable::Viewport;
use iced::widget::{column, container, row, text_editor};
use iced::{Element, Length, Point, Size, Subscription, Task, Theme};
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;

use crabot::model::{Cost, Model, ModelList};
use crabot::session::Session;
use crabot::tools::todo::TodoItem;
use crabot::user::WorkMode;
use prompt::{FilepathEntry, TOOLS, WORKSPACE_TREE};

use crate::views::{
    DividerState, center_pane, divider, left_pane,
    model_config::ProviderEntry,
    right_pane,
    session_list::SessionEntry,
    theme::{CRABOT_MODAL_SCRIM, set_dark_mode, theme_for},
    tool_list::ToolListState,
};
use crate::widgets::textarea::{self, TextArea};

mod conversation;
mod layout;
mod overlay;
pub(crate) mod prompt;
pub(crate) mod session_state;
mod settings;
mod subscription;
mod tool_state;

// ── App ───────────────────────────────────────────────────────────

/// Which widget currently holds keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedTarget {
    /// The user prompt text area.
    UserPrompt,
    /// A system-prompt text editor identified by its field name.
    EditText(&'static str),
    /// The session pick_list in the left pane.
    SessionPicker,
}

// ── Domain state groups ────────────────────────────────────────────

/// Window geometry, cursor, dividers, modifier keys, focus, and scroll state.
#[derive(Debug)]
pub(crate) struct LayoutState {
    pub(crate) window_size: Size,
    pub(crate) window_pos: Point,
    pub(crate) cursor: Point,
    pub(crate) left_divider: DividerState,
    pub(crate) right_divider: DividerState,
    pub(crate) theme: Theme,
    pub(crate) shift_held: bool,
    pub(crate) ctrl_held: bool,
    pub(crate) scroll_viewport_height: f32,
    pub(crate) focused: Option<FocusedTarget>,
}

/// Collapsible text-editor field with enable/expand state.
#[derive(Debug)]
pub(crate) struct ExpandableEditor {
    pub(crate) expanded: bool,
    pub(crate) enabled: bool,
    pub(crate) content: text_editor::Content,
}

/// System prompt, workspace, prompt-file options, and user-prompt editor.
pub(crate) struct PromptWorkspaceState {
    pub(crate) preamble: (bool, String),
    pub(crate) rules: (bool, String),
    pub(crate) workspace: (bool, PathBuf),
    pub(crate) agents_md: (bool, String),
    pub(crate) date: (bool, String),
    pub(crate) preamble_options: Vec<FilepathEntry>,
    pub(crate) rules_options: Vec<FilepathEntry>,
    pub(crate) workspace_options: Vec<FilepathEntry>,
    pub(crate) agents_md_exists: bool,
    pub(crate) files: ExpandableEditor,
    pub(crate) tools: ExpandableEditor,
    pub(crate) user_prompt: TextArea,
    pub(crate) workmode: WorkMode,
    pub(crate) workmode_enabled: bool,
    pub(crate) recipe_dropdown_expanded: bool,
}

impl PromptWorkspaceState {
    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut (bool, String)> {
        match name {
            prompt::PREAMBLE => Some(&mut self.preamble),
            prompt::RULES => Some(&mut self.rules),
            prompt::AGENTS_MD => Some(&mut self.agents_md),
            prompt::DATE => Some(&mut self.date),
            _ => None,
        }
    }

    pub(crate) fn content_mut(&mut self, name: &str) -> Option<&mut text_editor::Content> {
        match name {
            TOOLS => Some(&mut self.tools.content),
            WORKSPACE_TREE => Some(&mut self.files.content),
            _ => None,
        }
    }

    /// Concatenate all enabled components, returning the full prompt string.
    pub(crate) fn get_prompt(&self) -> String {
        let mut prompt = String::new();
        if let (true, content) = &self.preamble
            && !content.is_empty()
        {
            prompt.push_str(content);
            prompt.push('\n');
        }
        if self.workmode_enabled
            && let Some(file) = crabot::setup::ASSETS.get_file("workmode.md")
            && let Some(contents) = file.contents_utf8()
        {
            prompt.push_str(contents);
        }
        if let (true, content) = &self.rules
            && !content.is_empty()
        {
            prompt.push_str(content);
            prompt.push('\n');
        }
        if self.tools.enabled && !self.tools.content.text().is_empty() {
            prompt.push_str(&self.tools.content.text());
            prompt.push('\n');
        }
        if let (true, workspace) = &self.workspace
            && workspace.is_dir()
        {
            let path = crabot::tools::convert_path_to_unix_style(workspace);
            prompt.push_str(&format!("Current Workspace: {}\n", path));
        }
        if let (true, agents_md) = &self.agents_md
            && !agents_md.is_empty()
        {
            prompt.push_str(agents_md);
            prompt.push('\n');
        }
        if let (true, date) = &self.date
            && !date.is_empty()
        {
            prompt.push_str(&format!("Current Date: {}\n", date));
        }
        prompt
    }
}

/// Tool registry, enabled-tool sets, and todo snapshot.
pub(crate) struct ToolState {
    pub(crate) tool_registry: tools::ToolRegistry,
    pub(crate) enabled_tools: HashSet<String>,
    pub(crate) enabled_mcp_servers: HashSet<String>,
    pub(crate) tool_list_state: ToolListState,
    pub(crate) cached_todo_items: Vec<TodoItem>,
}

impl ToolState {
    /// Generate an XML-formatted summary of enabled tools.
    pub(crate) fn summary(&self) -> String {
        let all_tools = self
            .tool_registry
            .enabled_tools(&self.enabled_tools, &self.enabled_mcp_servers);
        let mut result = String::new();
        result.push_str("<available-tools>\n");

        for tool in &all_tools {
            let inst = tool.instruction();
            if inst.is_empty() {
                continue;
            }
            result.push_str(&format!("<tool name=\"{}\">{}</tool>\n", tool.name(), inst));
        }

        // Build the MCP tools prompt section for the system prompt.
        for server in &self.tool_registry.mcp_servers {
            if self.enabled_mcp_servers.contains(&server.name)
                && !server.prompt.is_empty()
                && self
                    .tool_registry
                    .get_mcp_tool_names(&server.name)
                    .iter()
                    .any(|name| self.enabled_tools.contains(name))
            {
                result.push_str(&server.prompt);
            }
        }
        result.push_str("</available-tools>\n");
        result.push_str("Tools can be enabled or disabled at any time. A tool used earlier in the conversation may no longer be available. Before using a tool, verify that it is currently available. You may also have access to additional tools not listed here.\n");
        result
    }
}

/// Session Conversation State.
pub(crate) struct ConversationState {
    pub(crate) session: Session,
    pub(crate) session_list: Vec<SessionEntry>,
    pub(crate) session_state: session_state::SessionState,
    pub(crate) expanded_turns: HashSet<(usize, usize)>,
    pub(crate) expanded_dialogs: HashSet<usize>,
    pub(crate) last_usage: genai::chat::Usage,
    pub(crate) center_pane_title: String,
    pub(crate) selectable_msgs: HashSet<usize>,
    pub(crate) search: crate::views::search_bar::SearchState,
}

impl ConversationState {
    pub(crate) fn status(&self) -> &str {
        self.session_state.status(self.session.is_empty())
    }
}

/// Model list, provider entries, and settings-dialog visibility / working state.
pub(crate) struct ModelSettingsState {
    pub(crate) provided_models: ModelList,
    pub(crate) provider_entries: Vec<ProviderEntry>,
    pub(crate) show_settings_dialog: bool,
    pub(crate) settings_state: crate::views::SettingsState,
}

/// Overlays: empty-workspace confirmation, restart button, update banner.
pub(crate) struct OverlayState {
    pub(crate) show_workspace_dialog: bool,
    pub(crate) show_restart: bool,
    pub(crate) default_workspace_path: PathBuf,
    pub(crate) update_available: Option<String>,
}

pub(crate) struct App {
    /// Persisted configuration shared across domains.
    pub settings: crabot::settings::Settings,
    pub layout: LayoutState,
    pub prompt: PromptWorkspaceState,
    pub tools: ToolState,
    pub conversation: ConversationState,
    pub model_settings: ModelSettingsState,
    pub overlay: OverlayState,
}

// ── Domain event types ────────────────────────────────────────────

/// Events related to window geometry, input, and scrolling.
#[derive(Clone)]
pub(crate) enum LayoutEvent {
    CursorMoved(Point),
    LeftPressed,
    LeftReleased,
    WindowResized(Size),
    WindowMoved(Point),
    ShiftHeld(bool),
    CtrlHeld(bool),
    SessionViewScrolled(Viewport),
    ScrollPageDown,
    ScrollPageUp,
    ScrollToHome,
    ScrollToEnd,
    UndoRedo(textarea::Message),
    EscapePressed,
    Zoom(f32),
    ToggleTheme(bool),
}

/// Events from the prompt, workspace, and user-input area.
#[derive(Clone)]
pub(crate) enum PromptEvent {
    ToggleEnabled(&'static str, bool),
    ToggleExpanded(&'static str),
    EditTextField(&'static str, String),
    EditTextContent(&'static str, text_editor::Action),
    EditTextArea(FocusedTarget, textarea::Message),
    SelectWorkspace(FilepathEntry),
    WorkspaceDialogResult(Option<PathBuf>),
    SelectPreamble(FilepathEntry),
    PreambleFileResult(Result<String, String>),
    SelectRules(FilepathEntry),
    RulesFileResult(Result<String, String>),
    SelectWorkMode(WorkMode),
    ToggleWorkMode(bool),
    ToggleRecipeDropdown,
    SelectRecipe(usize),
    DismissRecipeDropdown,
    SendPrompt,
}

/// Events related to tools and MCP server management.
#[derive(Clone)]
pub(crate) enum ToolEvent {
    ToggleMcpServer(String, bool),
    ToggleAgentTool(String, bool),
    McpToolsDiscovered((String, Vec<crabot::tools::mcp::McpTool>)),
}

/// Events from the conversation, session management, and streaming.
#[derive(Clone)]
pub(crate) enum ConversationEvent {
    NewSession,
    LoadSession(SessionEntry),
    SessionListLoaded(Vec<SessionEntry>),
    ToggleTurnExpand(usize, usize),
    ToggleDialogExpand(usize),
    ToggleAllDialogsExpand,
    SessionPickerFocused,
    NavigateSession(bool),
    DefocusSessionPicker,
    ResendSessionHistory,
    AskInputChanged(String),
    AskAction(session_state::AskAction),
    SessionEvent(session_state::SessionEvent),
    CopySessionTitle,
    AppClosing,
    ToggleSelectableMode(Option<usize>),
    SearchEvent(crate::views::SearchEvent),
    TurnOffsetsMeasured(u64, Vec<f32>),
}

/// Events for model configuration and settings dialog.
#[derive(Clone)]
pub(crate) enum ModelSettingsEvent {
    ModelConfig(crate::views::model_config::Event),
    Settings(crate::views::SettingsEvent),
}

/// Events for overlays: update banner, external links, version check.
#[derive(Clone)]
pub(crate) enum OverlayEvent {
    VersionCheckResult(Option<String>),
    DismissUpdateBanner,
    OpenReleaseNotes,
    EmptyWorkspaceConfirm(Option<PathBuf>),
}

// ── Pane-level event types ─────────────────────────────────────────

/// Events emitted by the left pane (model config + prompt + conversation + tools).
#[derive(Clone)]
pub(crate) enum LeftPaneEvent {
    ModelConfig(crate::views::model_config::Event),
    Prompt(PromptEvent),
    Conversation(ConversationEvent),
    Tools(ToolEvent),
}

/// Events emitted by the center pane (conversation + scroll + links).
#[derive(Clone)]
pub(crate) enum CenterPaneEvent {
    Conversation(ConversationEvent),
    SessionViewScrolled(Viewport),
    LinkClicked(String),
}

/// Events emitted by the right pane (conversation stats + theme toggle + restart).
#[derive(Clone)]
pub(crate) enum RightPaneEvent {
    ToggleTheme(bool),
    Restart,
}

// ── Root Message ──────────────────────────────────────────────────

/// Root message type dispatched through the Iced event loop.
#[derive(Clone)]
pub(crate) enum Message {
    Layout(LayoutEvent),
    Prompt(PromptEvent),
    Tools(ToolEvent),
    Conversation(ConversationEvent),
    Overlay(OverlayEvent),
    ModelSettings(ModelSettingsEvent),
    RestartApp,
}

// ── App impl ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn boot(mut saved: crabot::settings::Settings) -> (Self, Task<Message>) {
        let provided_models = model::load_models();
        let provider_entries: Vec<ProviderEntry> = provided_models
            .providers
            .iter()
            .map(|(id, p)| ProviderEntry {
                id: id.clone(),
                name: p.name.clone(),
            })
            .collect();
        saved.selected_model = provided_models.ensure_valid_name(&saved.selected_model);

        let custom_tool_list = tools::custom::ToolList::load();
        let mcp_list = tools::mcp::McpList::load();

        let mut tool_registry = tools::ToolRegistry::new();
        tool_registry.register_custom(custom_tool_list);
        tool_registry.mcp_servers = mcp_list.servers.clone();

        let enabled_tools: HashSet<String> = tool_registry
            .builtin_names
            .iter()
            .cloned()
            .chain(tool_registry.custom_names())
            .filter(|name| saved.agent_tools.get(name).copied().unwrap_or(true))
            .collect();

        let (preamble_options, preamble_content) =
            crate::views::load_prompt_options("preamble", &saved.selected_preamble);

        let (rules_options, rules_content) =
            crate::views::load_prompt_options("rules", &saved.selected_rules);

        let workspace_path = saved.workspace.clone();
        let files_tree = workspace::build_files_tree(&workspace_path);
        let (agents_md_exists, agents_md_content) = prompt::load_agents_md(&workspace_path);
        let enabled_mcp_servers: HashSet<_> = saved
            .mcp_servers
            .iter()
            .filter(|(_, enabled)| **enabled)
            .map(|(name, _)| name.clone())
            .collect();
        let tools = ToolState {
            tool_registry,
            enabled_tools,
            enabled_mcp_servers: enabled_mcp_servers.clone(),
            tool_list_state: ToolListState::default(),
            cached_todo_items: Vec::new(),
        };
        let tools_summary = tools.summary();
        let files_content = text_editor::Content::with_text(&files_tree);
        let tools_content = text_editor::Content::with_text(&tools_summary);

        let show_restart = !workspace_path.as_os_str().is_empty()
            && env::current_exe()
                .ok()
                .is_some_and(|exe| exe.starts_with(&workspace_path));

        let date_str = chrono::Local::now().format("%Y-%m-%d").to_string();

        let update_available = saved
            .last_update_version
            .as_ref()
            .filter(|v| crate::views::update::version_gt(v, crate::views::update::CURRENT_VERSION))
            .cloned();

        let workspace_options = crate::views::build_workspace_options(&saved.recent_workspaces);
        let window_size = Size::new(saved.window_size.0, saved.window_size.1);
        let window_pos = Point::new(saved.window_pos.0, saved.window_pos.1);
        let tools_enabled = saved.tools_enabled;
        let dark_mode = saved.dark_mode;
        set_dark_mode(dark_mode);

        let prompt = PromptWorkspaceState {
            preamble: (saved.preamble_enabled, preamble_content),
            rules: (saved.rules_enabled, rules_content),
            workspace: (saved.workspace_enabled, workspace_path),
            agents_md: (saved.agents_md_enabled, agents_md_content),
            date: (saved.date_enabled, date_str),
            preamble_options,
            rules_options,
            workspace_options,
            agents_md_exists,
            files: ExpandableEditor {
                expanded: false,
                enabled: true,
                content: files_content,
            },
            tools: ExpandableEditor {
                expanded: false,
                enabled: tools_enabled,
                content: tools_content,
            },
            user_prompt: TextArea::new(),
            workmode: WorkMode::default_mode(),
            workmode_enabled: true,
            recipe_dropdown_expanded: false,
        };

        let app = Self {
            settings: saved,
            layout: LayoutState {
                window_size,
                window_pos,
                cursor: Point::ORIGIN,
                left_divider: DividerState::default(),
                right_divider: DividerState::default(),
                theme: theme_for(dark_mode),
                shift_held: false,
                ctrl_held: false,
                scroll_viewport_height: 0.0,
                focused: None,
            },
            prompt,
            tools,
            conversation: ConversationState {
                session: Session::new(),
                session_list: Vec::new(),
                session_state: session_state::SessionState::new(),
                expanded_turns: HashSet::new(),
                expanded_dialogs: HashSet::new(),
                last_usage: genai::chat::Usage::default(),
                center_pane_title: "New session".into(),
                selectable_msgs: HashSet::new(),
                search: crate::views::search_bar::SearchState::default(),
            },
            model_settings: ModelSettingsState {
                provided_models,
                provider_entries,
                show_settings_dialog: false,
                settings_state: crate::views::SettingsState::default(),
            },
            overlay: OverlayState {
                show_workspace_dialog: false,
                show_restart,
                default_workspace_path: setup::default_workspace_path(),
                update_available,
            },
        };
        let session_task = conversation::refresh_session_list(app.prompt.workspace.1.clone());
        let discover_task = mcp_list
            .servers
            .into_iter()
            .map(|s| {
                Task::perform(
                    async move { tools::mcp::discover_mcp_server(s).await },
                    |result| Message::Tools(ToolEvent::McpToolsDiscovered(result)),
                )
            })
            .fold(Task::none(), Task::chain);
        // Skip the network check when a cached update is already available.
        let update_task = if show_restart || app.overlay.update_available.is_some() {
            Task::none()
        } else {
            Task::perform(crate::views::update::check_for_updates(), |result| {
                Message::Overlay(OverlayEvent::VersionCheckResult(result))
            })
        };
        // Run session refresh, MCP discovery, and version check in parallel.
        (app, Task::batch([session_task, discover_task, update_task]))
    }

    /// Rebuild the tools summary and refresh all UI fields that depend on it.
    pub(crate) fn refresh_tools_summary(&mut self) {
        let summary = self.tools.summary();
        self.prompt.tools.content = text_editor::Content::with_text(&summary);
    }

    pub(crate) fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Layout(event) => layout::update(self, event),
            Message::Prompt(event) => prompt::update(self, event),
            Message::Tools(event) => tool_state::update(self, event),
            Message::Conversation(event) => conversation::update(self, event),
            Message::Overlay(event) => overlay::update(self, event),
            Message::ModelSettings(event) => match event {
                ModelSettingsEvent::ModelConfig(e) => {
                    if matches!(e, crate::views::model_config::Event::OpenSettings) {
                        settings::open_settings(self)
                    } else {
                        settings::handle_model_config(self, e)
                    }
                }
                ModelSettingsEvent::Settings(e) => settings::handle_event(self, e),
            },
            Message::RestartApp => {
                self.save_settings();
                let _ = std::process::Command::new("cargo")
                    .args(["run", "--release"])
                    .spawn();
                iced::exit()
            }
        }
    }

    // ── Settings helpers (used by layout) ─────────────────────────

    /// Confirm a pending new-label input (Enter or focus loss).
    pub(crate) fn confirm_pending_label(&mut self) {
        settings::confirm_pending_label(self);
    }

    /// Sync derived fields back into `settings` and persist to disk.
    pub(crate) fn save_settings(&mut self) {
        self.settings.window_size = (
            self.layout.window_size.width,
            self.layout.window_size.height,
        );
        self.settings.window_pos = (
            self.layout.window_pos.x.max(0.0),
            self.layout.window_pos.y.max(0.0),
        );
        self.settings.preamble_enabled = self.prompt.preamble.0;
        self.settings.rules_enabled = self.prompt.rules.0;
        self.settings.workspace_enabled = self.prompt.workspace.0;
        self.settings.agents_md_enabled = self.prompt.agents_md.0;
        self.settings.date_enabled = self.prompt.date.0;
        self.settings.workspace = self.prompt.workspace.1.clone();
        self.settings.tools_enabled = self.prompt.tools.enabled;
        self.settings.sync_tools(
            &self.tools.tool_registry,
            &self.tools.enabled_tools,
            &self.tools.enabled_mcp_servers,
        );
        self.settings.save();
    }

    fn get_current_model(&self) -> Option<&Model> {
        self.conversation
            .session
            .model
            .as_ref()
            .or_else(|| {
                self.model_settings
                    .provided_models
                    .get_config(&self.settings.selected_model)
            })
            .and_then(|cfg| self.model_settings.provided_models.get_model(cfg))
    }

    /// Compute the model cost from the current session or settings, if available.
    pub(crate) fn current_model_cost(&self) -> Option<Cost> {
        self.get_current_model().map(|m| m.cost.clone())
    }

    // ── View composition ──────────────────────────────────────────

    pub(crate) fn view(&self) -> Element<'_, Message> {
        let body = self.view_body();
        self.view_with_banner(body)
    }

    /// The main three-pane layout with optional overlays.
    fn view_body(&self) -> Element<'_, Message> {
        let main = self.view_main_content();
        if self.model_settings.show_settings_dialog {
            let backdrop = container(
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(CRABOT_MODAL_SCRIM.into()),
                ..container::Style::default()
            });
            let dialog = crate::views::settings_dialog(&self.model_settings.settings_state)
                .map(|e| Message::ModelSettings(ModelSettingsEvent::Settings(e)));
            iced::widget::stack![
                main,
                backdrop,
                container(dialog)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            ]
            .into()
        } else if self.overlay.show_workspace_dialog {
            iced::widget::stack![
                main,
                crate::views::workspace_modal(&self.overlay.default_workspace_path)
                    .map(Message::Overlay),
            ]
            .into()
        } else {
            iced::widget::stack![main].into()
        }
    }

    /// The three-pane layout (left, center, right) with dividers.
    fn view_main_content(&self) -> Element<'_, Message> {
        row![
            left_pane(
                &self.settings,
                &self.prompt,
                &self.conversation,
                &self.tools,
                &self.model_settings,
            )
            .map(|event| match event {
                LeftPaneEvent::ModelConfig(e) => {
                    Message::ModelSettings(ModelSettingsEvent::ModelConfig(e))
                }
                LeftPaneEvent::Prompt(e) => Message::Prompt(e),
                LeftPaneEvent::Conversation(e) => Message::Conversation(e),
                LeftPaneEvent::Tools(e) => Message::Tools(e),
            }),
            divider(&self.layout.left_divider),
            center_pane(
                &self.conversation,
                &self.layout.theme,
                self.settings.font_scale,
            )
            .map(|event| match event {
                CenterPaneEvent::Conversation(e) => Message::Conversation(e),
                CenterPaneEvent::SessionViewScrolled(vp) => {
                    Message::Layout(LayoutEvent::SessionViewScrolled(vp))
                }
                CenterPaneEvent::LinkClicked(url) => {
                    if self.layout.ctrl_held {
                        let _ = open::that_detached(&url);
                    }
                    Message::Conversation(ConversationEvent::DefocusSessionPicker)
                }
            }),
            divider(&self.layout.right_divider),
            right_pane(
                self.settings.right_pane_width,
                self.get_current_model(),
                &self.conversation,
                self.overlay.show_restart,
                &self.tools.cached_todo_items,
                self.settings.dark_mode,
            )
            .map(|event| match event {
                RightPaneEvent::ToggleTheme(dark) =>
                    Message::Layout(LayoutEvent::ToggleTheme(dark)),
                RightPaneEvent::Restart => Message::RestartApp,
            }),
        ]
        .spacing(0)
        .into()
    }

    /// Optionally prepend the update-available banner.
    fn view_with_banner<'a>(&'a self, body: Element<'a, Message>) -> Element<'a, Message> {
        if let Some(latest) = &self.overlay.update_available {
            column![
                crate::views::update::update_banner(latest).map(Message::Overlay),
                body
            ]
            .spacing(0)
            .into()
        } else {
            body
        }
    }

    pub(crate) fn subscription(state: &Self) -> Subscription<Message> {
        subscription::subscription(state)
    }
}
