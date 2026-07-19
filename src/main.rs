// Hide the console window in release builds. Debug builds keep the console
// for `println!`/`eprintln!` output during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod fonts;
mod llm;
mod views;
mod widgets;

use crabot::{HashSetExt, model, settings, setup, system, tools, workspace};

use futures::{SinkExt, future::FutureExt};
use iced::widget::scrollable::Viewport;
use iced::widget::{column, row, text_editor};
use iced::{
    Element, Event, Point, Size, Subscription, Task, Theme, event, keyboard, mouse, window,
};
use indexmap::IndexMap;
use llm::DialogPhase;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;

use crabot::chat::Turn;
use crabot::model::{Model, ModelConfig, ModelList};
use crabot::session::Session;
use crabot::system::{FilepathEntry, SystemPrompt, TOOLS, WORKSPACE, WORKSPACE_TREE};
use crabot::tools::todo::TodoItem;

use crabot::user::{UserPrompt, WorkMode};
use views::model_config::ProviderEntry;
use views::session_list::SessionEntry;
use views::session_state::AskAction;
use views::system_prompt::PromptSectionState;
use views::theme::{HANDLE, MIN_W, default_theme};
use views::tool_list::ToolListState;
use views::{DividerState, center_pane, divider, left_pane, right_pane, scroll_to_end};
use widgets::textarea::{self, TextArea};

fn crabot_title() -> &'static str {
    concat!("Crabot v", env!("CARGO_PKG_VERSION"))
}

pub fn main() -> iced::Result {
    setup::ensure_default_files();
    fonts::load_system_fonts();
    let saved = settings::Settings::load();
    let size = Size::new(
        saved.window_size.0.max(MIN_W),
        saved.window_size.1.max(200.0),
    );
    let position =
        iced::window::Position::Specific(Point::new(saved.window_pos.0, saved.window_pos.1));
    let icon = setup::ASSETS.get_file("images/icon.ico").and_then(|f| {
        iced::window::icon::from_file_data(f.contents(), Some(image::ImageFormat::Ico)).ok()
    });
    iced::application(move || App::boot(saved.clone()), App::update, App::view)
        .subscription(App::subscription)
        .theme(|state: &App| state.theme.clone())
        .window(iced::window::Settings {
            size,
            position,
            exit_on_close_request: false,
            icon,
            ..Default::default()
        })
        .title(crabot_title())
        .antialiasing(true)
        .run()
}

// ── App ───────────────────────────────────────────────────────────

/// Which widget currently holds keyboard focus.
///
/// Stored as a single `Option` on `App` so that setting focus on one widget
/// implicitly clears focus on all others — no manual `set_focused(false)`
/// calls are needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedTarget {
    /// The user prompt text area.
    UserPrompt,
    /// A system-prompt text editor identified by its field name.
    EditText(&'static str),
    /// The session pick_list in the left pane.
    SessionPicker,
}

struct App {
    left_pane_width: f32,
    right_pane_width: f32,
    window_size: Size,
    window_pos: Point,
    cursor: Point,
    left_divider: DividerState,
    right_divider: DividerState,
    provided_models: ModelList,
    provider_entries: Vec<ProviderEntry>,
    selected_model: String,
    theme: Theme,
    system_prompt: SystemPrompt,
    prompt_section_state: PromptSectionState,
    tool_list_state: ToolListState,
    selected_preamble: String,
    preamble_options: Vec<FilepathEntry>,
    selected_rules: String,
    rules_options: Vec<FilepathEntry>,
    workspace_options: Vec<FilepathEntry>,
    /// Whether an AGENTS.md file exists in the current workspace.
    agents_md_exists: bool,
    /// Recent workspace paths with their per-workspace agents_md_enabled preference.
    recent_workspaces: Vec<(PathBuf, bool)>,
    // user-editable Content need to persist between view calls to maintain editor state
    files_content: text_editor::Content,
    tools_content: text_editor::Content,
    tool_registry: tools::ToolRegistry,
    enabled_tools: HashSet<String>,
    enabled_mcp_servers: HashSet<String>,
    /// Snapshot of saved agent-tool enable states.
    saved_agent_tools: IndexMap<String, bool>,
    user_prompt: TextArea,
    workmode: WorkMode,
    workmode_enabled: bool,
    session: Session,
    /// Available saved-sessions for the dropdown list in the left pane.
    session_options: Vec<SessionEntry>,
    /// Session state (LLM dialog phase, cancellation, auto-scroll).
    session_state: views::SessionState,
    /// Indices of turns whose collapsible body (tool result / reasoning) is expanded.
    expanded_turns: HashSet<(usize, usize)>,
    /// Indices of dialogs that are expanded.
    expanded_dialogs: HashSet<usize>,
    /// Token usage from the most recent completed LLM response.
    last_usage: genai::chat::Usage,
    /// Last-sent user prompt text, displayed in the center-pane header.
    center_pane_title: String,
    /// Whether to show the Restart button (current_exe within workspace).
    show_restart: bool,
    /// Indices of messages displayed as selectable plain text (double-click
    /// a single message to toggle; ESC clears all).
    selectable_msgs: HashSet<usize>,
    /// Whether the Shift key is currently held. Used to distinguish Enter
    /// (send prompt) from Shift+Enter (insert newline) in the text editor.
    shift_held: bool,
    /// Font scale factor for center pane dialog blocks.
    font_scale: f32,
    /// Which widget currently holds keyboard focus; `None` when no editable
    /// widget is focused. Setting this implicitly clears focus on all others.
    focused: Option<FocusedTarget>,
    /// Whether to show the in-app empty-workspace confirmation dialog.
    show_workspace_dialog: bool,
    /// Cached default workspace path shown in the confirmation dialog.
    default_workspace_path: PathBuf,
    /// Center-pane search UI state and measurement cache.
    search: views::search_bar::SearchState,
    /// Prompt recipes loaded from settings: work-mode name → list of prompt templates.
    prompt_recipe: IndexMap<String, Vec<String>>,
    /// Whether the recipe DropDown is currently expanded.
    recipe_dropdown_expanded: bool,
    /// Cached snapshot of the todo list, refreshed when the todo tool executes.
    cached_todo_items: Vec<TodoItem>,
    /// Latest version from crates.io when newer than current, shown as a banner.
    update_available: Option<String>,
    /// Cached latest version from the last crates.io check.
    cached_update_version: Option<String>,
}

#[derive(Clone)]
pub(crate) enum Message {
    CursorMoved(Point),
    LeftPressed,
    LeftReleased,
    WindowResized(Size),
    WindowMoved(Point),
    ToggleEnabled(&'static str, bool),
    ToggleExpanded(&'static str),
    EditTextField(&'static str, String),
    EditTextContent(&'static str, text_editor::Action),
    SelectWorkspace(FilepathEntry),
    WorkspaceDialogResult(Option<PathBuf>),
    SelectPreamble(FilepathEntry),
    PreambleFileResult(Result<String, String>),
    SelectRules(FilepathEntry),
    RulesFileResult(Result<String, String>),
    ToggleMcpServer(String, bool),
    ToggleAgentTool(String, bool),
    /// MCP tools discovered from a single server, delivered incrementally.
    McpToolsDiscovered((String, Vec<crabot::tools::mcp::McpTool>)),
    /// An edit action targeting a specific [`TextArea`].
    /// The [`FocusedTarget`] identifies which text area should receive the action.
    EditTextArea(FocusedTarget, textarea::Message),
    /// Global undo/redo shortcut (Ctrl+Z / Ctrl+Y). Routed to whichever
    /// [`TextArea`] currently holds keyboard focus.
    UndoRedo(textarea::Message),
    SelectWorkMode(WorkMode),
    ToggleWorkMode(bool),
    NewSession,
    LoadSession(SessionEntry),
    SessionListLoaded(Vec<SessionEntry>),
    SendPrompt,
    /// Result of the "empty workspace" confirmation dialog shown before sending.
    EmptyWorkspaceConfirm(Option<PathBuf>),
    ToggleTurnExpand(usize, usize),
    AskAction(AskAction),
    AskInputChanged(String),
    ToggleDialogExpand(usize),
    /// Streaming event (tool calls, content, reasoning, lifecycle).
    /// Wraps [`views::session_state::Event`] to delegate all streaming interactions.
    SessionEvent(views::SessionEvent),
    AppClosing,
    Noop,
    CopySessionTitle,
    ResendLastPrompt,
    Restart,
    /// Fires when the center pane scrollable's viewport changes.
    SessionViewScrolled(Viewport),
    /// Toggle a single message between Markdown and selectable plain-text.
    /// `Some(i)` toggles message `i`; `None` clears all (ESC).
    ToggleSelectableMode(Option<usize>),
    /// Track whether the Shift key is currently held.
    ShiftHeld(bool),
    /// Zoom the center pane font scale by a delta.
    Zoom(f32),
    /// Model configuration event (provider/model selection, thinking).
    ModelConfigEvent(views::model_config::Event),
    /// Session pick_list gained focus (e.g. via dropdown open).
    SessionPickerFocused,
    /// Arrow-key navigation for the session pick_list.
    /// `true` = up (previous), `false` = down (next).
    NavigateSession(bool),
    /// Search bar event (toggle, query change, submit, navigate).
    SearchEvent(views::SearchEvent),
    /// Escape key pressed. Closes search bar if visible, otherwise clears selectable-text mode.
    EscapePressed,
    /// Result of measuring turn offsets from the widget tree.
    TurnOffsetsMeasured(u64, Vec<f32>),
    /// Toggle the recipe DropDown expand/collapse.
    ToggleRecipeDropdown,
    /// Select a recipe by index and fill the user prompt text area.
    SelectRecipe(usize),
    /// Dismiss the recipe DropDown without selecting.
    DismissRecipeDropdown,
    /// Result of checking crates.io for a newer version.
    VersionCheckResult(Option<String>),
    /// User dismissed the update-available banner.
    DismissUpdateBanner,
    /// Open the Crabot GitHub releases page in the system browser.
    OpenReleaseNotes,
}

// ── App impl ──────────────────────────────────────────────────────

impl App {
    fn boot(saved: settings::Settings) -> (Self, Task<Message>) {
        let provided_models = model::load_models();
        let provider_entries: Vec<ProviderEntry> = provided_models
            .providers
            .iter()
            .map(|(id, p)| ProviderEntry {
                id: id.clone(),
                name: p.name.clone(),
            })
            .collect();
        let selected_model = provided_models.ensure_valid_name(&saved.selected_model);

        let custom_tool_list = tools::custom::ToolList::load();
        let mcp_list = tools::mcp::McpList::load();

        let mut tool_registry = tools::ToolRegistry::new();
        tool_registry.register_custom(custom_tool_list);
        tool_registry.mcp_servers = mcp_list.servers.clone();

        let enabled_tools: HashSet<String> = tool_registry
            .builtin_names
            .iter()
            .cloned()
            .chain(tool_registry.custom_names.iter().cloned())
            .filter(|name| saved.agent_tools.get(name).copied().unwrap_or(true))
            .collect();

        let (preamble_options, preamble_content) =
            views::load_prompt_options("preamble", &saved.selected_preamble);

        let (rules_options, rules_content) =
            views::load_prompt_options("rules", &saved.selected_rules);

        let workspace_path = saved.workspace;
        let files_tree = workspace::build_files_tree(&workspace_path);
        let (agents_md_exists, agents_md_content) = load_agents_md(&workspace_path);
        let enabled_mcp_servers: HashSet<_> = saved
            .mcp_servers
            .iter()
            .filter(|(_, enabled)| **enabled)
            .map(|(name, _)| name.clone())
            .collect();
        let tools_summary =
            system::tools_summary(&tool_registry, &enabled_tools, &enabled_mcp_servers);
        let files_content = text_editor::Content::with_text(&files_tree);
        let tools_content = text_editor::Content::with_text(&tools_summary);

        let show_restart = !workspace_path.as_os_str().is_empty()
            && env::current_exe()
                .ok()
                .is_some_and(|exe| exe.starts_with(&workspace_path));

        let system_prompt = SystemPrompt {
            preamble: (saved.preamble_enabled, preamble_content),
            rules: (saved.rules_enabled, rules_content),
            tools: (saved.tools_enabled, tools_summary),
            workspace: (saved.workspace_enabled, workspace_path),
            files: (saved.files_enabled, files_tree),
            agents_md: (saved.agents_md_enabled, agents_md_content),
            date: (
                saved.date_enabled,
                chrono::Local::now().format("%Y-%m-%d").to_string(),
            ),
        };

        let cached_update_version = saved.last_update_version;
        let update_available = cached_update_version
            .as_ref()
            .filter(|v| views::update::version_gt(v, views::update::CURRENT_VERSION))
            .cloned();

        let mut app = Self {
            left_pane_width: saved.left_pane_width,
            right_pane_width: saved.right_pane_width,
            window_size: Size::new(saved.window_size.0, saved.window_size.1),
            window_pos: Point::new(saved.window_pos.0, saved.window_pos.1),
            cursor: Point::ORIGIN,
            left_divider: DividerState::default(),
            right_divider: DividerState::default(),
            provided_models,
            provider_entries,
            preamble_options,
            rules_options,
            system_prompt,
            theme: default_theme(),
            selected_model,
            selected_preamble: saved.selected_preamble,
            selected_rules: saved.selected_rules,
            workspace_options: views::build_workspace_options(&saved.recent_workspaces),
            agents_md_exists,
            recent_workspaces: saved.recent_workspaces,
            prompt_section_state: PromptSectionState::default(),
            tool_list_state: ToolListState::default(),
            files_content,
            tools_content,
            tool_registry,
            enabled_tools,
            enabled_mcp_servers: enabled_mcp_servers.clone(),
            saved_agent_tools: saved.agent_tools,
            user_prompt: TextArea::new(),
            workmode: WorkMode::default_mode(),
            workmode_enabled: true,
            session: Session::new(),
            session_options: Vec::new(),
            session_state: views::SessionState::new(),
            expanded_turns: HashSet::new(),
            expanded_dialogs: HashSet::new(),
            last_usage: genai::chat::Usage::default(),
            center_pane_title: "New session".into(),
            show_restart,
            selectable_msgs: HashSet::new(),
            shift_held: false,
            font_scale: saved.font_scale,
            focused: None,
            show_workspace_dialog: false,
            default_workspace_path: setup::default_workspace_path(),
            search: views::search_bar::SearchState::default(),
            prompt_recipe: saved.prompt_recipe,
            recipe_dropdown_expanded: false,
            cached_todo_items: Vec::new(),
            update_available,
            cached_update_version,
        };
        let session_task = app.refresh_session_list();
        let discover_task = mcp_list
            .servers
            .into_iter()
            .map(|s| {
                Task::perform(
                    async move { tools::mcp::discover_mcp_server(s).await },
                    Message::McpToolsDiscovered,
                )
            })
            .fold(Task::none(), Task::chain);
        // Skip the network check when a cached update is already available.
        let update_task = if app.update_available.is_some() {
            Task::none()
        } else {
            Task::perform(
                views::update::check_for_updates(),
                Message::VersionCheckResult,
            )
        };
        // Run session refresh, MCP discovery, and version check in parallel.
        (app, Task::batch([session_task, discover_task, update_task]))
    }

    /// Rebuild the tools summary and refresh all UI fields that depend on it.
    fn refresh_tools_summary(&mut self) {
        let summary = system::tools_summary(
            &self.tool_registry,
            &self.enabled_tools,
            &self.enabled_mcp_servers,
        );
        self.tools_content = text_editor::Content::with_text(&summary);
        self.system_prompt.tools.1 = summary;
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::CursorMoved(pos) => {
                self.cursor = pos;
                self.left_divider.hovered =
                    pos.x >= self.left_pane_width && pos.x <= self.left_pane_width + HANDLE;
                let right_x = self.window_size.width - self.right_pane_width - HANDLE;
                self.right_divider.hovered = pos.x >= right_x && pos.x <= right_x + HANDLE;

                if self.left_divider.dragging {
                    let delta = pos.x - self.left_divider.origin;
                    let gutter = 2.0 * HANDLE;
                    let max = (self.window_size.width - self.right_pane_width - gutter - MIN_W)
                        .max(MIN_W);
                    self.left_pane_width = (self.left_divider.start + delta).clamp(MIN_W, max);
                } else if self.right_divider.dragging {
                    let delta = pos.x - self.right_divider.origin;
                    let gutter = 2.0 * HANDLE;
                    let max =
                        (self.window_size.width - self.left_pane_width - gutter - MIN_W).max(MIN_W);
                    let new = (self.right_divider.start - delta).max(0.0);
                    // Shrink below MIN_W → hide, expand from hidden → snap to MIN_W.
                    self.right_pane_width = if self.right_divider.start == 0.0 {
                        if new > 10.0 {
                            new.clamp(MIN_W, max)
                        } else {
                            0.0
                        }
                    } else if new < MIN_W - 10.0 {
                        0.0
                    } else {
                        new.min(max)
                    };
                }
            }
            Message::LeftPressed => {
                let left_x = self.left_pane_width;
                let right_x = self.window_size.width - self.right_pane_width - HANDLE;

                if self.cursor.x >= left_x && self.cursor.x <= left_x + HANDLE {
                    self.left_divider.dragging = true;
                    self.left_divider.origin = self.cursor.x;
                    self.left_divider.start = self.left_pane_width;
                } else if self.cursor.x >= right_x && self.cursor.x <= right_x + HANDLE {
                    // When the right pane is hidden, a single click on the divider shows it
                    if self.right_pane_width == 0.0 {
                        self.right_pane_width = MIN_W;
                    } else {
                        self.right_divider.dragging = true;
                        self.right_divider.origin = self.cursor.x;
                        self.right_divider.start = self.right_pane_width;
                    }
                }
            }
            Message::LeftReleased => {
                self.left_divider.dragging = false;
                self.right_divider.dragging = false;
            }
            Message::WindowResized(size) => {
                // Ignore zero-size events (e.g. window minimized on Windows).
                if size.width > 0.0 && size.height > 0.0 {
                    self.window_size = size;
                    self.search.invalidate_offsets();
                }
            }
            Message::WindowMoved(pos) => {
                self.window_pos = pos;
            }
            Message::ModelConfigEvent(event) => {
                if views::model_config::update(
                    event,
                    &mut self.provided_models,
                    &mut self.selected_model,
                ) {
                    self.provided_models.save();
                }
            }
            Message::ToggleEnabled(name, enabled) => {
                if name == WORKSPACE {
                    self.system_prompt.workspace.0 = enabled;
                } else if let Some(field) = self.system_prompt.get_mut(name) {
                    field.0 = enabled;
                }
            }
            Message::ToggleMcpServer(server, enabled) => {
                if enabled {
                    self.enabled_mcp_servers.set(server.clone(), true);
                    // When enabling a server without a live connection (e.g.
                    // disabled at boot or just toggled off), trigger discovery.
                    if !tools::mcp::has_connection(&server)
                        && let Some(s) = self
                            .tool_registry
                            .mcp_servers
                            .iter()
                            .find(|s| s.name == server)
                            .cloned()
                    {
                        return Task::perform(
                            async move { tools::mcp::discover_mcp_server(s).await },
                            Message::McpToolsDiscovered,
                        );
                    }
                } else {
                    tools::mcp::drop_connection(&server);
                    self.enabled_mcp_servers.set(server.clone(), false);
                }
                self.refresh_tools_summary();
            }
            Message::ToggleAgentTool(tool_name, enabled) => {
                self.enabled_tools.set(tool_name, enabled);
                self.refresh_tools_summary();
            }
            Message::McpToolsDiscovered((server_name, tools)) => {
                if tools.is_empty() {
                    self.enabled_mcp_servers.remove(&server_name);
                    tools::mcp::drop_connection(&server_name);
                } else {
                    // Drop connection if server is disabled (e.g. disabled at boot).
                    if !self.enabled_mcp_servers.contains(&server_name) {
                        tools::mcp::drop_connection(&server_name);
                    }
                    let new_names: Vec<_> = tools.iter().map(|t| t.name.clone()).collect();
                    self.tool_registry.register_mcp_group(server_name, tools);
                    // Auto-enable tools that were previously saved as enabled.
                    // New tools default to disabled (opt-in).
                    self.enabled_tools.extend(
                        new_names.into_iter().filter(|name| {
                            self.saved_agent_tools.get(name).copied().unwrap_or(false)
                        }),
                    );
                }
                self.refresh_tools_summary();
            }
            Message::ToggleExpanded(name) => {
                if !self.prompt_section_state.update(name) {
                    self.tool_list_state.update(name);
                }
            }
            Message::EditTextField(name, value) => {
                if let Some(field) = self.system_prompt.get_mut(name) {
                    field.1 = value;
                }
            }
            Message::EditTextContent(name, action) => {
                // A click on this text editor claims focus (implicitly clearing
                // focus on the prompt editor and all others).
                if matches!(action, text_editor::Action::Click(_)) {
                    self.focused = Some(FocusedTarget::EditText(name));
                }
                let text = if let Some(content) = self.content_mut(name) {
                    content.perform(action);
                    content.text()
                } else {
                    return Task::none();
                };
                if let Some(field) = self.system_prompt.get_mut(name) {
                    field.1 = text;
                }
            }
            Message::EditTextArea(target, msg) => {
                // A click claims keyboard focus for this text area.
                if msg.is_click() {
                    self.focused = Some(target);
                } else if self.focused != Some(target) {
                    return Task::none();
                }
                if target == FocusedTarget::UserPrompt {
                    // Enter sends the prompt; Shift+Enter inserts a newline.
                    if msg.is_enter() && !self.shift_held {
                        return Task::done(Message::SendPrompt);
                    }
                    self.user_prompt.update(msg, self.shift_held);
                }
            }
            Message::UndoRedo(msg) => {
                // Global Ctrl+Z/Y — route to whichever TextArea holds focus.
                if let Some(FocusedTarget::UserPrompt) = self.focused {
                    self.user_prompt.update(msg, self.shift_held);
                }
            }
            Message::SelectWorkspace(entry) => {
                if entry.path.as_os_str().is_empty() {
                    return Task::perform(
                        async { rfd::FileDialog::new().pick_folder() },
                        Message::WorkspaceDialogResult,
                    );
                }
                self.set_workspace(entry.path);
                return self.refresh_session_list();
            }
            Message::WorkspaceDialogResult(path) => {
                if let Some(path) = path {
                    self.set_workspace(path);
                    return self.refresh_session_list();
                }
            }
            Message::SelectPreamble(entry) => {
                return Self::select_prompt_file(
                    entry,
                    &mut self.selected_preamble,
                    Message::PreambleFileResult,
                );
            }
            Message::PreambleFileResult(result) => {
                if let Ok(content) = result {
                    self.system_prompt.preamble.1 = content;
                }
            }
            Message::SelectRules(entry) => {
                return Self::select_prompt_file(
                    entry,
                    &mut self.selected_rules,
                    Message::RulesFileResult,
                );
            }
            Message::RulesFileResult(result) => {
                if let Ok(content) = result {
                    self.system_prompt.rules.1 = content;
                }
            }
            Message::SelectWorkMode(mode) => {
                self.workmode = mode;
            }
            Message::ToggleWorkMode(enabled) => {
                self.workmode_enabled = enabled;
            }
            Message::NewSession => {
                self.session = Session::new();
                self.session_state = views::SessionState::new();
                self.center_pane_title = "New session".into();
                self.last_usage = genai::chat::Usage::default();
                self.expanded_turns.clear();
                self.expanded_dialogs.clear();
                self.selectable_msgs.clear();
                self.search.reset();
                self.cached_todo_items.clear();
                self.tool_registry.clear_todo();
                // Refresh workspace tree so the system prompt reflects current files.
                self.system_prompt.files.1 =
                    workspace::build_files_tree(&self.system_prompt.workspace.1);
                self.files_content = text_editor::Content::with_text(&self.system_prompt.files.1);
                let (exists, agents_md_content) = load_agents_md(&self.system_prompt.workspace.1);
                self.agents_md_exists = exists;
                self.system_prompt.agents_md.1 = agents_md_content;
                return self.refresh_session_list();
            }
            Message::ToggleTurnExpand(idx, sub) => {
                let key = (idx, sub);
                let present = self.expanded_turns.contains(&key);
                self.expanded_turns.set(key, !present);
                self.search.invalidate_offsets();
            }
            Message::ToggleDialogExpand(idx) => {
                let present = self.expanded_dialogs.contains(&idx);
                self.expanded_dialogs.set(idx, !present);
                self.search.invalidate_offsets();
            }
            Message::LoadSession(entry) => {
                if self.session_state.phase != DialogPhase::Idle {
                    return Task::none();
                }
                match Session::load(&entry.path) {
                    Ok(session) => {
                        self.session = session;
                    }
                    Err(e) => {
                        self.session = Session::new();
                        self.session.id = entry.id;
                        eprintln!("Failed to load session: {e}");
                    }
                }
                self.last_usage = genai::chat::Usage {
                    prompt_tokens: Some(self.session.size),
                    ..Default::default()
                };
                self.center_pane_title = self.session.title.clone();
                self.expanded_turns.clear();
                self.expanded_dialogs.clear();
                self.selectable_msgs.clear();
                self.search.reset();
                // Restore todo list from the last successful todo tool call in history.
                self.cached_todo_items = self.session.last_todo_items();
            }
            Message::SessionListLoaded(entries) => {
                self.session_options = entries;
            }
            Message::SessionPickerFocused => {
                self.focused = Some(FocusedTarget::SessionPicker);
            }
            Message::NavigateSession(up) => {
                if self.focused != Some(FocusedTarget::SessionPicker)
                    || self.session_state.phase != DialogPhase::Idle
                    || self.session_options.is_empty()
                {
                    return Task::none();
                }
                let current_idx = self
                    .session_options
                    .iter()
                    .position(|e| e.id == self.session.id);
                let new_entry = match current_idx {
                    Some(idx) => {
                        let new_idx = if up {
                            idx.checked_sub(1)
                                .unwrap_or(self.session_options.len().saturating_sub(1))
                        } else {
                            let next = idx + 1;
                            if next < self.session_options.len() {
                                next
                            } else {
                                0
                            }
                        };
                        Some(self.session_options[new_idx].clone())
                    }
                    None => {
                        // Current session not in list; select the first (or last) entry.
                        if up {
                            self.session_options.last().cloned()
                        } else {
                            self.session_options.first().cloned()
                        }
                    }
                };
                if let Some(entry) = new_entry {
                    return Task::done(Message::LoadSession(entry));
                }
            }
            Message::SendPrompt => {
                let content_raw = self.user_prompt.text();
                let content = crabot::tools::normalize_newlines(&content_raw).into_owned();
                if content.trim().is_empty() {
                    return Task::none();
                }

                let Some(model) = self
                    .provided_models
                    .get_config(&self.selected_model)
                    .cloned()
                else {
                    return Task::none();
                };

                // If no workspace is set, ask the user whether to continue with
                // the default `~/.crabot` workspace before sending the prompt.
                if self.system_prompt.workspace.1.as_os_str().is_empty() {
                    self.show_workspace_dialog = true;
                    return Task::none();
                }

                let mode = if self.workmode_enabled {
                    Some(self.workmode)
                } else {
                    None
                };
                let user_prompt = UserPrompt::new(mode, content.clone()).get_prompt();
                self.user_prompt.clear();

                // During streaming: stash the prompt for the agent loop to pick up.
                if self.session_state.phase != DialogPhase::Idle {
                    if let Ok(mut pending) = self.session_state.pending_user_prompt.lock() {
                        *pending = Some(user_prompt.clone());
                    }
                    self.session_state.pending_display = Some(content);
                    return Task::none();
                }

                let title = Session::derive_title(&content);
                self.center_pane_title = content;
                // Auto-collapse all previous dialogs; keep the new one expanded.
                let new_dialog_idx = self.session.dialogs.len();
                self.expanded_dialogs.clear();
                self.expanded_dialogs.insert(new_dialog_idx);
                self.session.add_dialog(title);
                self.session.push_turn(Turn::user(user_prompt.clone()));

                return self.start_dialog(&model, Some(user_prompt));
            }
            Message::EmptyWorkspaceConfirm(path) => {
                self.show_workspace_dialog = false;
                let Some(path) = path else {
                    return Task::none();
                };
                self.set_workspace(path);
                // Re-enter the send-prompt flow now that a workspace is set.
                return Task::done(Message::SendPrompt);
            }
            Message::ResendLastPrompt => {
                if self.session_state.phase != DialogPhase::Idle
                    || self.center_pane_title == "New session"
                {
                    return Task::none();
                }
                let Some(model) = self
                    .provided_models
                    .get_config(&self.selected_model)
                    .cloned()
                else {
                    return Task::none();
                };
                // Collapse all other dialogs; keep the last one expanded during resend.
                self.expanded_dialogs.clear();
                if let Some(idx) = self.session.dialogs.len().checked_sub(1) {
                    self.expanded_dialogs.insert(idx);
                }
                return self.start_dialog(&model, None);
            }
            Message::AskInputChanged(input) => {
                self.session_state.ask_input = input;
            }
            Message::AskAction(action) => {
                let result = match action {
                    AskAction::OptionSelected(option) => {
                        self.session_state.ask_input = option;
                        return Task::none();
                    }
                    AskAction::Ok => Ok(self.session_state.ask_input.clone()),
                    AskAction::Skip => Ok("No preference. Use your best judgment.".into()),
                };
                let _ = self.session_state.ask_sender.send(result);
                self.session_state.ask_request = None;
            }
            Message::SessionEvent(event) => {
                // If the todo tool just finished, refresh the cached snapshot.
                if let views::SessionEvent::ToolResult(ref tr) = event
                    && tr.name == "todo"
                {
                    self.cached_todo_items = self.tool_registry.snapshot_todo();
                }
                let cost = self.get_current_model().map(|m| m.cost);
                return views::session_state::update(
                    event,
                    &mut self.session_state,
                    &mut self.session,
                    &mut self.search,
                    &mut self.last_usage,
                    cost,
                    &mut self.user_prompt,
                );
            }
            Message::CopySessionTitle => {
                return iced::clipboard::write(self.center_pane_title.clone());
            }
            Message::Restart => {
                self.save_settings();
                // Spawn release build in background before exiting.
                let _ = Command::new("cargo").args(["run", "--release"]).spawn();
                return iced::exit();
            }
            Message::SessionViewScrolled(viewport) => {
                views::session_state::handle_scroll(&self.session_state, viewport);
            }
            Message::AppClosing => {
                // Signal any in-flight stream and running tools to stop.
                self.session_state
                    .cancel_token
                    .store(true, Ordering::Release);
                self.save_settings();
                return iced::exit();
            }
            Message::Noop => {}
            Message::ShiftHeld(held) => {
                self.shift_held = held;
            }
            Message::Zoom(delta) => {
                self.font_scale = (self.font_scale + delta).clamp(0.5, 2.0);
                self.search.invalidate_offsets();
            }
            Message::EscapePressed => {
                if self.search.visible {
                    self.search.visible = false;
                } else {
                    self.selectable_msgs.clear();
                }
            }
            Message::ToggleSelectableMode(msg_index) => match msg_index {
                Some(i) => {
                    let present = self.selectable_msgs.contains(&i);
                    self.selectable_msgs.set(i, !present);
                }
                None => self.selectable_msgs.clear(),
            },
            Message::SearchEvent(event) => {
                return views::search_bar::update(
                    event,
                    &mut self.search,
                    &self.session,
                    &mut self.expanded_dialogs,
                    &mut self.expanded_turns,
                );
            }
            Message::TurnOffsetsMeasured(generation, offsets) => {
                self.search.handle_offsets(generation, offsets);
            }
            Message::ToggleRecipeDropdown => {
                self.recipe_dropdown_expanded = !self.recipe_dropdown_expanded;
            }
            Message::SelectRecipe(index) => {
                let mode_key = self.workmode.name.to_lowercase();
                if let Some(recipes) = self.prompt_recipe.get(&mode_key)
                    && let Some(recipe) = recipes.get(index)
                {
                    self.user_prompt.replace_text(recipe);
                }
                self.recipe_dropdown_expanded = false;
            }
            Message::DismissRecipeDropdown => {
                self.recipe_dropdown_expanded = false;
            }
            Message::VersionCheckResult(latest) => {
                self.update_available = latest.clone();
            }
            Message::DismissUpdateBanner => {
                self.update_available = None;
            }
            Message::OpenReleaseNotes => {
                if let Err(e) = open::that(views::update::RELEASES_URL) {
                    eprintln!("Failed to open release notes: {e}");
                }
            }
        }
        Task::none()
    }

    fn start_dialog(
        &mut self,
        model_config: &ModelConfig,
        user_prompt: Option<String>,
    ) -> Task<Message> {
        // Update session state with the selected model and workspace.
        let Some(model) = self.provided_models.get_model_info(model_config) else {
            return Task::none();
        };
        self.session.model = Some(model_config.clone());
        self.session.workspace = self.system_prompt.workspace.1.clone();
        self.session.save().ok();

        // Add current session to the dropdown list so it appears immediately.
        if self.session.dialogs.len() == 1
            && let Some(path) = self.session.save_path()
        {
            let entry = SessionEntry {
                id: self.session.id.clone(),
                title: self.session.title.clone(),
                path,
            };
            self.session_options.insert(0, entry);
        }

        // Clear any stale pending prompt from a previous stream.
        if let Ok(mut pending) = self.session_state.pending_user_prompt.lock() {
            *pending = None;
        }
        self.session_state.pending_display = None;
        self.session_state.start_index = self.session.total_turns();
        self.session_state
            .auto_scroll
            .store(true, Ordering::Relaxed);

        // Create a fresh mpsc channel for this stream's ask-tool responses.
        let (ask_tx, ask_rx) = tokio::sync::mpsc::unbounded_channel();
        self.session_state.ask_sender = ask_tx;

        let config = llm::SendConfig {
            model,
            workspace: self.system_prompt.workspace.1.clone(),
            system_prompt: self.system_prompt.get_prompt(self.workmode_enabled),
            user_prompt,
            tools: self
                .tool_registry
                .enabled_tools(&self.enabled_tools, &self.enabled_mcp_servers),
            pending_user_prompt: self.session_state.pending_user_prompt.clone(),
            ask_receiver: ask_rx,
            user_agent: crabot_title().to_string(),
            cancel_token: self.session_state.cancel_token.clone(),
        };

        let history = self.session.history.clone();

        self.session_state.phase = DialogPhase::LlmLoading;
        self.session_state
            .cancel_token
            .store(false, Ordering::Relaxed);
        let cancel_token = self.session_state.cancel_token.clone();

        Task::batch([
            scroll_to_end(),
            Task::stream(iced::stream::channel(128, async move |sender| {
                let cancel = cancel_token.clone();
                let mut callback = {
                    move |msg: views::SessionEvent| {
                        let cancel = cancel.clone();
                        let mut sender = sender.clone();
                        async move {
                            let ok = sender.send(Message::SessionEvent(msg)).await.is_ok();
                            if cancel.load(Ordering::Relaxed) {
                                false
                            } else {
                                ok
                            }
                        }
                        .boxed()
                    }
                };
                llm::send_stream(config, history, &mut callback).await;
            })),
        ])
    }

    /// Bump `path` to top of recents, persist it as current workspace,
    /// restore its agents_md_enabled preference, and rebuild the files tree.
    fn set_workspace(&mut self, path: PathBuf) {
        // Save current workspace preference before switching.
        let cur = &self.system_prompt.workspace.1;
        if !cur.as_os_str().is_empty() {
            let enabled = self.system_prompt.agents_md.0;
            if let Some(entry) = self.recent_workspaces.iter_mut().find(|(p, _)| p == cur) {
                entry.1 = enabled;
            } else {
                self.recent_workspaces.push((cur.clone(), enabled));
            }
        }

        // Move the new workspace to the front of recents.
        let (exists, content) = load_agents_md(&path);
        let enabled = self
            .recent_workspaces
            .iter()
            .find_map(|(p, e)| (p == &path).then_some(*e))
            .unwrap_or(true)
            && exists;
        self.recent_workspaces.retain(|(p, _)| p != &path);
        self.recent_workspaces.insert(0, (path.clone(), enabled));
        self.recent_workspaces.truncate(10);

        // Apply workspace.
        self.system_prompt.files.1 = workspace::build_files_tree(&path);
        self.files_content = text_editor::Content::with_text(&self.system_prompt.files.1);
        self.agents_md_exists = exists;
        self.system_prompt.agents_md = (enabled, content);
        self.show_restart = env::current_exe()
            .ok()
            .is_some_and(|exe| exe.starts_with(&path));
        self.system_prompt.workspace.1 = path;
        self.workspace_options = views::build_workspace_options(&self.recent_workspaces);
    }

    /// Collect current app state into `Settings` and persist to disk.
    fn save_settings(&self) {
        let settings = settings::Settings {
            left_pane_width: self.left_pane_width,
            right_pane_width: self.right_pane_width,
            window_size: (self.window_size.width, self.window_size.height),
            window_pos: (self.window_pos.x.max(0.0), self.window_pos.y.max(0.0)),
            selected_model: self.selected_model.clone(),
            selected_preamble: self.selected_preamble.clone(),
            selected_rules: self.selected_rules.clone(),
            preamble_enabled: self.system_prompt.preamble.0,
            rules_enabled: self.system_prompt.rules.0,
            tools_enabled: self.system_prompt.tools.0,
            workspace_enabled: self.system_prompt.workspace.0,
            agents_md_enabled: self.system_prompt.agents_md.0,
            files_enabled: self.system_prompt.files.0,
            date_enabled: self.system_prompt.date.0,
            workspace: self.system_prompt.workspace.1.clone(),
            recent_workspaces: self.recent_workspaces.clone(),
            font_scale: self.font_scale,
            mcp_servers: self
                .tool_registry
                .mcp_servers
                .iter()
                .map(|s| (s.name.clone(), self.enabled_mcp_servers.contains(&s.name)))
                .collect(),
            agent_tools: self
                .tool_registry
                .all_names()
                .map(|name| {
                    let enabled = self.enabled_tools.contains(name);
                    (name.clone(), enabled)
                })
                .collect(),
            prompt_recipe: self.prompt_recipe.clone(),
            last_update_version: self.cached_update_version.clone(),
        };
        settings.save();
    }

    /// Read a prompt file (preamble or rules) from disk and return a task
    /// that produces the appropriate `FileResult` message.
    fn select_prompt_file(
        entry: FilepathEntry,
        selected: &mut String,
        on_load: fn(Result<String, String>) -> Message,
    ) -> Task<Message> {
        let FilepathEntry { display, path } = entry;
        *selected = display;
        Task::perform(
            async move { std::fs::read_to_string(&path).map_err(|e| e.to_string()) },
            on_load,
        )
    }

    /// Refresh the session list dropdown entries from disk.
    fn refresh_session_list(&mut self) -> Task<Message> {
        let workspace = self.system_prompt.workspace.1.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::views::session_list::list_entries(&workspace)
                })
                .await
                .unwrap_or(Ok(Vec::new()))
            },
            |result| match result {
                Ok(entries) => Message::SessionListLoaded(entries),
                Err(_) => Message::SessionListLoaded(Vec::new()),
            },
        )
    }

    fn content_mut(&mut self, name: &str) -> Option<&mut text_editor::Content> {
        match name {
            TOOLS => Some(&mut self.tools_content),
            WORKSPACE_TREE => Some(&mut self.files_content),
            _ => None,
        }
    }

    fn get_current_model(&self) -> Option<&Model> {
        self.session
            .model
            .as_ref()
            .or_else(|| self.provided_models.get_config(&self.selected_model))
            .and_then(|cfg| self.provided_models.get_model(cfg))
    }

    fn get_status(&self) -> &str {
        self.session_state.status(self.session.is_empty())
    }

    /// Return the recipe list for the currently active work mode (lowercase key).
    fn current_recipe_list(&self) -> &[String] {
        let key = self.workmode.name.to_lowercase();
        self.prompt_recipe
            .get(&key)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn view(&self) -> Element<'_, Message> {
        let main_content: Element<_> = row![
            left_pane(
                self.left_pane_width,
                &self.provided_models,
                &self.provider_entries,
                &self.selected_model,
                &self.system_prompt,
                self.agents_md_exists,
                &self.prompt_section_state,
                &self.tool_list_state,
                &self.selected_preamble,
                &self.preamble_options,
                &self.selected_rules,
                &self.rules_options,
                &self.workspace_options,
                &self.files_content,
                &self.tools_content,
                &self.enabled_tools,
                &self.tool_registry,
                &self.user_prompt,
                self.workmode,
                self.workmode_enabled,
                self.current_recipe_list(),
                self.recipe_dropdown_expanded,
                self.session_state.phase,
                &self.session_options,
                &self.session.id,
                &self.enabled_mcp_servers,
            ),
            divider(&self.left_divider),
            center_pane(
                &self.center_pane_title,
                self.session.dialogs.as_slice(),
                &self.expanded_turns,
                &self.expanded_dialogs,
                self.get_status(),
                &self.theme,
                self.session_state.phase,
                &self.selectable_msgs,
                self.font_scale,
                self.session_state.pending_display.as_deref(),
                self.session_state.ask_request.as_ref(),
                &self.session_state.ask_input,
                &self.search,
                self.session.model.as_ref().map(|m| m.model_id.as_str()),
                &self.session.created_at,
            ),
            divider(&self.right_divider),
            right_pane(
                self.right_pane_width,
                self.get_current_model().map(|model| model.context_window),
                &self.last_usage,
                &self.session.usage,
                self.session.cost,
                &self.session.modified_files,
                self.show_restart,
                &self.cached_todo_items,
            ),
        ]
        .spacing(0)
        .into();

        let body: Element<_> = if self.show_workspace_dialog {
            iced::widget::stack![
                main_content,
                views::workspace_modal(&self.default_workspace_path),
            ]
            .into()
        } else {
            main_content
        };

        if let Some(latest) = &self.update_available {
            column![views::update::update_banner(latest), body]
                .spacing(0)
                .into()
        } else {
            body
        }
    }

    fn subscription(_state: &Self) -> Subscription<Message> {
        Subscription::batch([
            event::listen_with(|event, status, _window| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::CursorMoved(position))
                }
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    Some(Message::LeftPressed)
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::LeftReleased)
                }
                Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
                Event::Window(window::Event::Moved(pos)) => Some(Message::WindowMoved(pos)),
                // Always handle Escape regardless of capture status so that
                // selectable-text mode can be exited in a single keypress.
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::EscapePressed),
                // Skip keyboard shortcuts when a widget already captured the
                // event (e.g. dropdown overlay handling arrow-key navigation).
                Event::Keyboard(_) if status == event::Status::Captured => None,
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
                    ..
                }) => Some(Message::NavigateSession(true)),
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
                    ..
                }) => Some(Message::NavigateSession(false)),
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if modifiers.command() =>
                {
                    match key.as_ref() {
                        keyboard::Key::Character("z") => {
                            Some(Message::UndoRedo(textarea::Message::Undo))
                        }
                        keyboard::Key::Character("y") => {
                            Some(Message::UndoRedo(textarea::Message::Redo))
                        }
                        keyboard::Key::Character("f") => {
                            Some(Message::SearchEvent(views::SearchEvent::ToggleSearch))
                        }
                        keyboard::Key::Character("=") => Some(Message::Zoom(0.05)),
                        keyboard::Key::Character("-") => Some(Message::Zoom(-0.05)),
                        _ => None,
                    }
                }
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Shift),
                    ..
                }) => Some(Message::ShiftHeld(true)),
                Event::Keyboard(keyboard::Event::KeyReleased {
                    key: keyboard::Key::Named(keyboard::key::Named::Shift),
                    ..
                }) => Some(Message::ShiftHeld(false)),
                _ => None,
            }),
            window::close_requests().map(|_id| Message::AppClosing),
        ])
    }
}

/// Read `AGENTS.md` from the workspace root, returning (exists, content).
fn load_agents_md(workspace: &std::path::Path) -> (bool, String) {
    if !workspace.as_os_str().is_empty() {
        let path = workspace.join("AGENTS.md");
        if path.is_file() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            return (true, content);
        }
    }
    (false, String::new())
}
