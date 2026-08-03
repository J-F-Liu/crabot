//! Application state, message routing, and the Iced Program implementation.
//!
//! This module owns the root [`App`] state, the nested domain [`Message`] enum,
//! and the boot / update / view / subscription methods that drive the GUI.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use iced::widget::scrollable::Viewport;
use iced::widget::{column, container, row, text_editor};
use iced::{Element, Length, Point, Size, Subscription, Task, Theme};

use crabot::model::{self, ModelConfig, ModelList};
use crabot::tools;
use crabot::user::{UserPrompt, WorkMode};
use crabot::{setup, workspace};
use prompt::{FilepathEntry, TOOLS, WORKSPACE_TREE};

use crate::views::{
    DividerState, center_pane, divider, left_pane, right_pane,
    session_list::SessionEntry,
    theme::{CRABOT_MODAL_SCRIM, set_dark_mode, theme_for},
    tool_list::ToolListState,
};
use crate::widgets::textarea::{self, TextArea};

pub(crate) mod conversation;
mod layout;
mod overlay;
pub(crate) mod prompt;
pub(crate) mod session_state;
mod session_tab;
mod settings;
mod subscription;
mod tool_state;

pub(crate) use session_tab::{SessionEndStatus, SessionTab};

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
    pub(crate) preamble_enabled: bool,
    pub(crate) rules_enabled: bool,
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

    /// Concatenate all enabled components, returning the full system prompt.
    /// Preamble and rules file contents are read from disk on every call.
    pub(crate) fn get_system_prompt(
        &self,
        selected_preamble: &str,
        selected_rules: &str,
    ) -> String {
        let preamble = self
            .preamble_enabled
            .then(|| prompt::load_prompt_file(&self.preamble_options, selected_preamble));
        self.compose_system_prompt(preamble.as_deref(), selected_rules)
    }

    /// Like [`get_system_prompt`](Self::get_system_prompt), but with a
    /// caller-provided preamble section replacing the configured one
    /// (`None` omits the preamble component entirely).
    pub(crate) fn compose_system_prompt(
        &self,
        preamble: Option<&str>,
        selected_rules: &str,
    ) -> String {
        fn section(prompt: &mut String, content: &str) {
            if !content.is_empty() {
                prompt.push_str(content);
                prompt.push('\n');
            }
        }
        let mut prompt = String::new();
        if let Some(content) = preamble {
            section(&mut prompt, content);
        }
        if self.workmode_enabled
            && let Some(contents) = crabot::setup::ASSETS
                .get_file("workmode.md")
                .and_then(|file| file.contents_utf8())
        {
            section(&mut prompt, contents);
        }
        if self.rules_enabled {
            section(
                &mut prompt,
                &prompt::load_prompt_file(&self.rules_options, selected_rules),
            );
        }
        if self.tools.enabled {
            section(&mut prompt, &self.tools.content.text());
        }
        if let (true, workspace) = &self.workspace
            && workspace.is_dir()
        {
            let path = crabot::tools::convert_path_to_unix_style(workspace);
            prompt.push_str(&format!("Current Workspace: {}\n", path));
        }
        if self.agents_md.0 {
            section(&mut prompt, &self.agents_md.1);
        }
        if let (true, date) = &self.date
            && !date.is_empty()
        {
            prompt.push_str(&format!("Current Date: {}\n", date));
        }
        prompt
    }
}

/// Tool registry, enabled-tool sets.
pub(crate) struct ToolState {
    pub(crate) tool_registry: tools::ToolRegistry,
    pub(crate) enabled_tools: HashSet<String>,
    pub(crate) enabled_mcp_servers: HashSet<String>,
    pub(crate) tool_list_state: ToolListState,
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

/// Compact scroll state for the tab-bar scrollable.
///
/// The `Viewport` type has private fields, so we track the three values we need
/// separately.  The struct is updated both by `on_scroll` (user scroll / layout)
/// and eagerly when arrow buttons are clicked, keeping arrow visibility and
/// scroll-target clamping in sync.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TabBarScrollState {
    /// Current absolute horizontal scroll offset.
    pub offset: f32,
    /// Width of the viewport (visible area).
    pub viewport_w: f32,
    /// Total width of the scrollable content.
    pub content_w: f32,
}

impl TabBarScrollState {
    pub fn from_viewport(vp: &Viewport) -> Self {
        Self {
            offset: vp.absolute_offset().x,
            viewport_w: vp.bounds().width,
            content_w: vp.content_bounds().width,
        }
    }

    /// Whether the content overflows the viewport horizontally.
    pub fn has_overflow(&self) -> bool {
        self.content_w > self.viewport_w
    }

    /// Maximum possible horizontal scroll offset.
    pub fn max_offset(&self) -> f32 {
        (self.content_w - self.viewport_w).max(0.0)
    }

    /// Whether there is room to scroll further left.
    pub fn can_scroll_left(&self) -> bool {
        self.offset > 1.0
    }

    /// Whether there is room to scroll further right.
    pub fn can_scroll_right(&self) -> bool {
        self.offset < self.max_offset() - 1.0
    }
}

/// Session tabbed conversation state.
pub(crate) struct ConversationState {
    pub(crate) session_tabs: Vec<SessionTab>,
    /// The *viewing* tab index, tabs list is never empty.
    pub(crate) viewing: usize,
    /// Monotonic counter for the next tab's `number`; reset on restart.
    next_tab_number: usize,
    pub(crate) session_list: Vec<SessionEntry>,
    /// True while a workspace's session list is being re-scanned off-thread
    /// after a workspace switch; the picker shows a loading placeholder meanwhile.
    pub(crate) session_list_loading: bool,
    /// Per-workspace cached session lists, keyed by workspace path.
    /// Refreshed on explicit workspace switch; reused on tab-switch workspace sync.
    pub(crate) session_list_cache: HashMap<PathBuf, Vec<SessionEntry>>,
    /// Queue of tab numbers whose ask-tool requests arrived while the viewing tab already had an unanswered ask.
    pub(crate) pending_ask_queue: std::collections::VecDeque<usize>,
    /// Current scroll state of the tab bar: offset, viewport width, content width.
    pub(crate) tab_bar_scroll: TabBarScrollState,
    /// Direction being held for tab-bar scroll auto-repeat, if any.
    pub(crate) tab_bar_held_direction: Option<conversation::TabBarDirection>,
    /// Which arrow is currently hovered, for visual feedback.
    pub(crate) tab_bar_hovered_direction: Option<conversation::TabBarDirection>,
}

impl ConversationState {
    pub(crate) fn new(selected_model: String, selected_preamble: String) -> Self {
        Self {
            session_tabs: vec![SessionTab::new(1, selected_model, selected_preamble)],
            viewing: 0,
            next_tab_number: 2,
            session_list: Vec::new(),
            session_list_loading: false,
            session_list_cache: HashMap::new(),
            pending_ask_queue: std::collections::VecDeque::new(),
            tab_bar_scroll: TabBarScrollState::default(),
            tab_bar_held_direction: None,
            tab_bar_hovered_direction: None,
        }
    }

    /// Immutable reference to the currently-viewed tab.
    pub(crate) fn viewing(&self) -> &SessionTab {
        &self.session_tabs[self.viewing]
    }

    /// Mutable reference to the currently-viewed tab.
    pub(crate) fn viewing_mut(&mut self) -> &mut SessionTab {
        &mut self.session_tabs[self.viewing]
    }

    /// The 1-based number of the viewing tab.
    pub(crate) fn viewing_tab_number(&self) -> usize {
        self.viewing().number
    }

    /// Positions (indices into `tabs`) of all currently-running tabs.
    pub(crate) fn running_positions(&self) -> impl Iterator<Item = usize> + '_ {
        self.session_tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.running().then_some(i))
    }

    /// Position of a tab by its stable number.
    pub(crate) fn tab_pos(&self, number: usize) -> Option<usize> {
        self.session_tabs.iter().position(|t| t.number == number)
    }

    /// Human-readable status for the viewing tab.
    pub(crate) fn status(&self) -> Cow<'static, str> {
        self.viewing()
            .session_state
            .status(self.viewing().session.is_empty())
    }

    /// Whether the viewing tab has an active stream.
    pub(crate) fn viewing_is_streaming(&self) -> bool {
        self.viewing().running()
    }

    /// Take the next tab number and advance the counter.
    pub(crate) fn next_tab_number(&mut self) -> usize {
        let n = self.next_tab_number;
        self.next_tab_number += 1;
        n
    }

    /// Take and clear the pending prompt, returns `None` if there was no pending prompt.
    pub(crate) fn take_pending_prompt(&mut self, tab_pos: usize) -> Option<UserPrompt> {
        let state = &mut self.session_tabs[tab_pos].session_state;
        let prompt = state.pending_prompt.take()?;
        if let Ok(mut pending) = state.injected_prompt.lock() {
            *pending = None;
        }
        Some(prompt)
    }

    /// Request all running tabs to stop streaming.
    pub(crate) fn stop(&mut self) {
        self.pending_ask_queue.clear();
        for tab in &mut self.session_tabs {
            if tab.running() {
                tab.session_state.stop();
            }
        }
    }
}

/// Overlays: empty-workspace confirmation, restart button, update banner.
pub(crate) struct OverlayState {
    pub(crate) show_workspace_dialog: bool,
    pub(crate) default_workspace_path: PathBuf,
    pub(crate) update_available: Option<String>,
    pub(crate) download_state: crate::views::update::UpdateDownloadState,
}

pub(crate) struct App {
    /// Persisted model/provider list (source of truth for model selection).
    pub models: ModelList,
    /// Persisted configuration shared across domains.
    pub settings: crabot::settings::Settings,
    pub layout: LayoutState,
    pub prompt: PromptWorkspaceState,
    pub tools: ToolState,
    pub conversation: ConversationState,
    /// Settings-dialog visibility and working state.
    pub settings_dialog: crate::views::SettingsState,
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
    TabBarScrolled(Viewport),
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
    SelectRules(FilepathEntry),
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
    /// A finished workspace session-list scan, tagged with the workspace it scanned.
    SessionListLoaded(PathBuf, Vec<SessionEntry>),
    /// A finished off-thread workspace scan (files tree + AGENTS.md).
    WorkspaceContentReady(Box<prompt::WorkspaceScan>),
    /// A prepared task-tool spawn whose blocking workspace scan finished — the
    /// sub-agent session can now be launched.
    TaskSpawnReady(Box<conversation::SuccessorSpawn>),
    /// A prepared renew spawn whose blocking workspace scan finished — the
    /// continuation session can now be launched.
    RenewSpawnReady(Box<conversation::SuccessorSpawn>),
    ToggleTurnExpand(usize, usize),
    ToggleDialogExpand(usize),
    ToggleAllDialogsExpand,
    SessionPickerFocused,
    NavigateSession(bool),
    DefocusSessionPicker,
    ResendSessionHistory,
    AskInputChanged(String),
    AskAction(session_state::AskAction),
    /// A session-streaming event tagged with the tab number that owns the stream.
    SessionEvent(usize, session_state::SessionEvent),
    /// Switch the viewing tab to the one with the given number.
    SwitchTab(usize),
    /// Switch to the Nth tab (1-based position); 0 means the last tab.
    SwitchTabByDigit(usize),
    /// Close the tab with the given number.
    CloseTab(usize),
    /// Close the currently-viewed tab (Ctrl+W).
    CloseCurrentTab,
    CopySessionTitle,
    AppClosing,
    ToggleSelectableMode(Option<usize>),
    SearchEvent(crate::views::SearchEvent),
    /// (tab_number, generation, offsets, target_y) — target_y scrolls only if that tab is still viewing.
    TurnOffsetsMeasured(usize, u64, Vec<f32>, Option<f32>),
    /// Mouse pressed on left scroll arrow — starts press-and-hold auto-repeat.
    TabBarScrollLeftHold,
    /// Mouse pressed on right scroll arrow — starts press-and-hold auto-repeat.
    TabBarScrollRightHold,
    /// Timer tick for auto-repeat scrolling while an arrow is held.
    TabBarScrollTick,
    /// Cursor entered a tab-bar arrow.
    TabBarArrowEnter(conversation::TabBarDirection),
    /// Cursor left a tab-bar arrow.
    TabBarArrowExit,
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
    InstallUpdate,
    UpdateReady(Result<PathBuf, String>),
    RestartFromUpdate,
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
    /// Tab bar scrollable viewport changed.
    TabBarScrolled(Viewport),
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
        let models = model::load_models();
        saved.selected_model = models.ensure_valid_name(&saved.selected_model);

        tools::init_tool_limits(saved.tool_limits);

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

        let preamble_options = crate::views::build_md_file_options("preamble");
        let rules_options = crate::views::build_md_file_options("rules");

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
        };
        let tools_summary = tools.summary();
        let files_content = text_editor::Content::with_text(&files_tree);
        let tools_content = text_editor::Content::with_text(&tools_summary);

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

        let initial_selected_model = saved.selected_model.clone();
        let initial_selected_preamble = if preamble_options.iter().any(|e| e.display == "crabot") {
            "crabot".to_string()
        } else {
            saved.selected_preamble.clone()
        };

        let prompt = PromptWorkspaceState {
            preamble_enabled: saved.preamble_enabled,
            rules_enabled: saved.rules_enabled,
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
            conversation: ConversationState::new(initial_selected_model, initial_selected_preamble),
            models,
            settings_dialog: crate::views::SettingsState::default(),
            overlay: OverlayState {
                show_workspace_dialog: false,
                default_workspace_path: setup::default_workspace_path(),
                update_available,
                download_state: crate::views::update::UpdateDownloadState::Idle,
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
        // Skip the network check when auto-check is disabled or a cached update is already available.
        let update_task =
            if app.settings.auto_check_updates && app.overlay.update_available.is_none() {
                Task::perform(crate::views::update::check_for_updates(), |result| {
                    Message::Overlay(OverlayEvent::VersionCheckResult(result))
                })
            } else {
                Task::none()
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
                self.conversation.stop();
                self.save_settings();
                let workspace_path = &self.prompt.workspace.1;
                if let Ok(exe) = std::env::current_exe() {
                    if !workspace_path.as_os_str().is_empty() && exe.starts_with(workspace_path) {
                        let _ = std::process::Command::new("cargo")
                            .args(["run", "--release"])
                            .spawn();
                    } else {
                        let _ = std::process::Command::new(&exe).spawn();
                    }
                }
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
        self.settings.selected_model = self.conversation.viewing().selected_model.clone();
        self.settings.selected_preamble = self.conversation.viewing().selected_preamble.clone();
        self.settings.window_size = (
            self.layout.window_size.width,
            self.layout.window_size.height,
        );
        self.settings.window_pos = (
            self.layout.window_pos.x.max(0.0),
            self.layout.window_pos.y.max(0.0),
        );
        self.settings.preamble_enabled = self.prompt.preamble_enabled;
        self.settings.rules_enabled = self.prompt.rules_enabled;
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

    fn get_current_model(&self) -> Option<&ModelConfig> {
        self.conversation
            .viewing()
            .session
            .model
            .as_ref()
            .or_else(|| {
                self.models
                    .get_config(&self.conversation.viewing().selected_model)
            })
    }

    /// Look up the currently selected model's config, cloned for ownership.
    pub(crate) fn selected_model_config(&self) -> Option<ModelConfig> {
        self.models
            .get_config(&self.conversation.viewing().selected_model)
            .cloned()
    }

    /// Find the model label matching the given config, falling back to the viewing tab's label.
    pub(crate) fn find_model_label(&self, model_config: &ModelConfig) -> String {
        self.models
            .models
            .iter()
            .find(|(_, cfg)| {
                cfg.provider_id == model_config.provider_id && cfg.model_id == model_config.model_id
            })
            .map(|(label, _)| label.clone())
            .unwrap_or_else(|| self.conversation.viewing().selected_model.clone())
    }

    // ── View composition ──────────────────────────────────────────

    pub(crate) fn view(&self) -> Element<'_, Message> {
        let body = self.view_body();
        self.view_with_banner(body)
    }

    /// The main three-pane layout with optional overlays.
    fn view_body(&self) -> Element<'_, Message> {
        let main = self.view_main_content();
        if self.settings_dialog.open {
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
            let dialog = crate::views::settings_dialog(&self.settings_dialog)
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
                &self.models,
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
                CenterPaneEvent::TabBarScrolled(vp) => {
                    Message::Layout(LayoutEvent::TabBarScrolled(vp))
                }
            }),
            divider(&self.layout.right_divider),
            right_pane(
                self.settings.right_pane_width,
                self.get_current_model(),
                self.conversation.viewing(),
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
                crate::views::update::update_banner(latest, &self.overlay.download_state)
                    .map(Message::Overlay),
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
