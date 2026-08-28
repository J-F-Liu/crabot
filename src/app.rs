//! Application state, message routing, and the Iced Program implementation.
//!
//! This module owns the root [`App`] state, the nested domain [`Message`] enum,
//! and the boot / update / view / subscription methods that drive the GUI.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;

use iced::widget::scrollable::Viewport;
use iced::widget::{column, container, row, text_editor};
use iced::{Element, Length, Point, Size, Subscription, Task, Theme};

use crabot::model::{self, ModelConfig, ModelList};
use crabot::setup;
use crabot::tools;
use crabot::user::{UserPrompt, WorkMode};
use prompt::{FilepathEntry, TOOLS};

use crate::views::{
    DividerState, PaneSection, PaneSections, center_pane, divider, left_pane, right_pane,
    session_list::SessionEntry,
    theme::{self, CRABOT_MODAL_SCRIM},
    tool_list::ToolListState,
};
use crate::widgets::textarea::{self, TextArea};

mod attention;
pub(crate) mod conversation;
mod layout;
mod overlay;
pub(crate) mod prompt;
pub(crate) mod session_state;
mod session_tab;
mod settings;
pub(crate) mod snapshot;
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
    /// Whether the window has OS focus (gates taskbar attention).
    pub(crate) window_focused: bool,
}

/// Collapsible text-editor field with enable/expand state.
#[derive(Debug)]
pub(crate) struct ExpandableEditor {
    pub(crate) expanded: bool,
    pub(crate) enabled: bool,
    pub(crate) content: text_editor::Content,
}

/// Collapsible read-only preview of the workspace files tree.
#[derive(Debug)]
pub(crate) struct FileTreePane {
    pub(crate) enabled: bool,
    pub(crate) expanded: bool,
    /// Latest fetched tree (display-only; rebuilt each time the pane expands).
    pub(crate) tree: String,
}

/// System prompt, workspace, prompt-file options, and user-prompt editor.
pub(crate) struct PromptWorkspaceState {
    pub(crate) preamble_enabled: bool,
    pub(crate) skills_enabled: bool,
    pub(crate) workspace: (bool, PathBuf),
    pub(crate) agents_md: (bool, String),
    pub(crate) date: (bool, String),
    pub(crate) preamble_options: Vec<FilepathEntry>,
    pub(crate) skills_options: Vec<FilepathEntry>,
    pub(crate) workspace_options: Vec<FilepathEntry>,
    pub(crate) agents_md_exists: bool,
    pub(crate) files: FileTreePane,
    pub(crate) tools: ExpandableEditor,
    pub(crate) user_prompt: TextArea,
    pub(crate) workmode: WorkMode,
    pub(crate) workmode_enabled: bool,
    pub(crate) recipe_dropdown_expanded: bool,
    /// Whether the skills multi-select dropdown is open.
    pub(crate) skills_menu_expanded: bool,
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
            _ => None,
        }
    }

    /// Concatenate all enabled components, returning the full system prompt.
    /// Preamble and skills file contents are read from disk on every call.
    pub(crate) fn get_system_prompt(
        &self,
        selected_preamble: &str,
        selected_skills: &[String],
    ) -> String {
        let preamble = self
            .preamble_enabled
            .then(|| prompt::load_prompt_file(&self.preamble_options, selected_preamble));
        self.compose_system_prompt(preamble.as_deref(), selected_skills)
    }

    /// Like [`get_system_prompt`](Self::get_system_prompt), but with a
    /// caller-provided preamble section replacing the configured one
    /// (`None` omits the preamble component entirely).
    pub(crate) fn compose_system_prompt(
        &self,
        preamble: Option<&str>,
        selected_skills: &[String],
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
        if self.skills_enabled {
            section(
                &mut prompt,
                &prompt::load_prompt_files(&self.skills_options, selected_skills),
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
                    .mcp_server_has_enabled_tool(&server.name, &self.enabled_tools)
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
    /// True while the session-header actions popup menu is expanded.
    pub(crate) header_menu_open: bool,
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
            header_menu_open: false,
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
    /// Revert-All confirmation dialog visibility.
    pub(crate) show_revert_all_confirm: bool,
    /// Owning tab number captured when the dialog opened (Ctrl+1..9 still works under the modal).
    pub(crate) revert_all_tab: usize,
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
    /// Right-pane section expand/collapse state, shared across all tabs.
    pub pane_sections: PaneSections,
    /// Running-process snapshot shown in the right pane, refreshed once per
    /// registry change via [`Message::ProcessTick`].
    pub running_processes: Vec<tools::process::RunningProcess>,
    /// ACP HTTP server state (toggle, bind address, shutdown handle).
    pub(crate) acp: crate::acp::AcpState,
    pub overlay: OverlayState,
    /// Per-workspace shared snapshot locks held until exit (see `snapshot::retain_workspace_lock`).
    pub(crate) snapshot_locks: HashMap<PathBuf, File>,
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
    WindowFocusChanged(bool),
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
    /// A freshly built files tree for the expanded preview pane.
    FileTreeReady(String),
    EditTextArea(FocusedTarget, textarea::Message),
    SelectWorkspace(FilepathEntry),
    WorkspaceDialogResult(Option<PathBuf>),
    SelectPreamble(FilepathEntry),
    /// Toggle a skill file in the multi-selection.
    ToggleSkill(String, bool),
    /// Open/close the skills multi-select dropdown.
    ToggleSkillsMenu,
    /// Dismiss the skills dropdown (outside click / Escape).
    DismissSkillsMenu,
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
    /// A finished off-thread AGENTS.md scan, tagged with the workspace it scanned.
    WorkspaceContentReady(Box<prompt::WorkspaceScan>),
    /// A prepared task-tool spawn whose blocking workspace scan finished — the
    /// sub-agent session can now be launched.
    TaskSpawnReady(Box<session_state::SuccessorSpawn>),
    /// A prepared renew spawn whose blocking workspace scan finished — the
    /// continuation session can now be launched.
    RenewSpawnReady(Box<session_state::SuccessorSpawn>),
    /// A validated prompt whose fresh workspace-tree scan finished — it can now
    /// be injected or launched.
    SendWithFreshTree(Box<conversation::PendingSend>),
    ToggleTurnExpand(usize, usize),
    ToggleDialogExpand(usize),
    ToggleAllDialogsExpand,
    SessionPickerFocused,
    NavigateSession(bool),
    DefocusSessionPicker,
    ResendSessionHistory,
    /// Fork the viewing session into a new tab.
    ForkSession,
    /// Compact the viewing session into a new tab.
    CompactSession,
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
    /// Export the current session to an HTML file (opens a save dialog).
    ExportSessionHtml,
    /// Result of the HTML export.
    ExportSessionHtmlDone(crate::views::export::ExportOutcome),
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
    /// Toggle the session-header actions popup menu.
    ToggleHeaderActionsMenu,
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
    /// Streaming download progress: `(downloaded_bytes, total_bytes)`.
    UpdateProgress {
        downloaded: u64,
        total: Option<u64>,
    },
    UpdateReady(Result<PathBuf, String>),
    RestartFromUpdate,
    EmptyWorkspaceConfirm(Option<PathBuf>),
    /// User answered the Revert-All confirmation dialog.
    RevertAllConfirm(bool),
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

/// Events emitted by the right pane.
#[derive(Clone)]
pub(crate) enum RightPaneEvent {
    /// Toggle the ACP HTTP server.
    ToggleAcpServer(bool),
    ToggleTheme(bool),
    Restart,
    /// Restore the file from its snapshot.
    RevertFile(String),
    /// Restore all snapshotted files of the current session.
    RevertAll,
    /// Dismiss the transient revert error message.
    DismissRevertError,
    /// Expand or collapse a right-pane section.
    ToggleSection(PaneSection),
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
    RevertFile(String),
    RevertAll,
    /// Single-file revert finished: (tab number, Ok(raw) | Err(message)).
    RevertDone(usize, Result<String, String>),
    /// Revert All finished: (tab number, per-file outcomes).
    RevertAllDone(usize, Vec<Result<String, String>>),
    DismissRevertError,
    /// Expand or collapse a right-pane section.
    TogglePaneSection(PaneSection),
    /// Managed process started/exited; refresh the cached right-pane list.
    ProcessTick,
    /// ACP HTTP server bridge events.
    Acp(crate::acp::AcpMessage),
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
        let skills_options = crate::views::build_md_file_options("skills");

        // Drop skill selections whose files no longer exist (e.g. a skill
        // file deleted from ~/.crabot/skills).
        saved
            .selected_skills
            .retain(|name| skills_options.iter().any(|entry| &entry.display == name));

        let workspace_path = saved.workspace.clone();
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
        let enabled_tools_count = tools.enabled_tools.len();
        let enabled_mcp_count = tools.enabled_mcp_servers.len();

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
        let acp_server_enabled = saved.acp_server_enabled;
        theme::set_dark_mode(dark_mode);

        let initial_selected_model = saved.selected_model.clone();
        let initial_selected_preamble = if preamble_options.iter().any(|e| e.display == "crabot") {
            "crabot".to_string()
        } else {
            saved.selected_preamble.clone()
        };

        let prompt = PromptWorkspaceState {
            preamble_enabled: saved.preamble_enabled,
            skills_enabled: saved.skills_enabled,
            workspace: (saved.workspace_enabled, workspace_path.clone()),
            agents_md: (saved.agents_md_enabled, String::new()),
            date: (saved.date_enabled, date_str),
            preamble_options,
            skills_options,
            workspace_options,
            agents_md_exists: false,
            // Tree is fetched fresh from disk whenever the pane is expanded.
            files: FileTreePane {
                enabled: true,
                expanded: false,
                tree: String::new(),
            },
            tools: ExpandableEditor {
                expanded: false,
                enabled: tools_enabled,
                content: text_editor::Content::with_text(&tools_summary),
            },
            user_prompt: TextArea::with_text(&saved.user_prompt),
            workmode: WorkMode::default_mode(),
            workmode_enabled: true,
            recipe_dropdown_expanded: false,
            skills_menu_expanded: false,
        };

        let models_count = models.models.len();
        let mut app = Self {
            settings: saved,
            layout: LayoutState {
                window_size,
                window_pos,
                cursor: Point::ORIGIN,
                left_divider: DividerState::default(),
                right_divider: DividerState::default(),
                theme: theme::theme_for(dark_mode),
                shift_held: false,
                ctrl_held: false,
                scroll_viewport_height: 0.0,
                focused: None,
                window_focused: true,
            },
            prompt,
            tools,
            conversation: ConversationState::new(initial_selected_model, initial_selected_preamble),
            models,
            settings_dialog: crate::views::SettingsState::default(),
            pane_sections: PaneSections::default(),
            running_processes: Vec::new(),
            acp: crate::acp::AcpState::new(acp_server_enabled),
            snapshot_locks: HashMap::new(),
            overlay: OverlayState {
                show_workspace_dialog: false,
                default_workspace_path: setup::default_workspace_path(),
                show_revert_all_confirm: false,
                revert_all_tab: 0,
                update_available,
                download_state: crate::views::update::UpdateDownloadState::Idle,
            },
        };
        tracing::info!(
            workspace = %workspace_path.display(),
            models = models_count,
            enabled_tools = enabled_tools_count,
            enabled_mcp_server_count = enabled_mcp_count,
            dark_mode,
            "crabot boot complete"
        );
        // Boot-time session list scan: show the loading placeholder until it lands.
        app.conversation.session_list_loading = !workspace_path.as_os_str().is_empty();
        let session_task = conversation::refresh_session_list(workspace_path.clone());
        let workspace_task = prompt::scan_workspace_content(workspace_path, None);
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
        // Resume the ACP HTTP server when it was enabled at shutdown.
        let acp_task = if app.settings.acp_server_enabled {
            crate::acp::start(&mut app)
        } else {
            Task::none()
        };
        // Run workspace scan, session refresh, MCP discovery, and version check in parallel.
        (
            app,
            Task::batch([
                session_task,
                workspace_task,
                discover_task,
                update_task,
                acp_task,
            ]),
        )
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
                crate::acp::stop(self);
                self.save_settings();
                snapshot::cleanup_snapshots(self);
                tools::process::shutdown();
                let workspace_path = &self.prompt.workspace.1;
                tracing::info!("restarting crabot");
                match std::env::current_exe() {
                    Ok(exe) => {
                        // A workspace-local exe means a dev build — relaunch via
                        // cargo so the latest source changes take effect.
                        if !workspace_path.as_os_str().is_empty() && exe.starts_with(workspace_path)
                        {
                            tracing::info!("relaunching via cargo run --release");
                            if let Err(e) = std::process::Command::new("cargo")
                                .args(["run", "--release"])
                                .env_remove("RUST_RECURSION_COUNT") // repeated `cargo run` relaunches would exhaust it, max value is 20
                                .spawn()
                            {
                                tracing::error!("failed to spawn cargo run --release: {e}");
                            }
                        } else {
                            tracing::info!(exe = %exe.display(), "relaunching current executable");
                            if let Err(e) = spawn_relaunch(&exe, &[]) {
                                tracing::error!("failed to spawn replacement process: {e}");
                            }
                        }
                    }
                    Err(e) => tracing::error!("cannot determine current exe for restart: {e}"),
                }
                iced::exit()
            }
            Message::RevertFile(path) => snapshot::revert(self, path),
            Message::RevertAll => snapshot::request_revert_all(self),
            Message::RevertDone(number, outcome) => snapshot::revert_done(self, number, outcome),
            Message::RevertAllDone(number, outcomes) => {
                snapshot::revert_all_done(self, number, outcomes)
            }
            Message::DismissRevertError => {
                self.conversation.viewing_mut().modified_files_error = None;
                Task::none()
            }
            Message::TogglePaneSection(section) => {
                let expanded = match section {
                    PaneSection::ContextWindow => &mut self.pane_sections.context_window,
                    PaneSection::TokenUsage => &mut self.pane_sections.token_usage,
                    PaneSection::Processes => &mut self.pane_sections.processes,
                    PaneSection::AccessedFiles => &mut self.pane_sections.accessed_files,
                    PaneSection::ModifiedFiles => &mut self.pane_sections.modified_files,
                };
                *expanded = !*expanded;
                Task::none()
            }
            // Refresh the cached list once per registry change; the view
            // only reads the snapshot.
            Message::ProcessTick => {
                self.running_processes = tools::process::running_processes();
                Task::none()
            }
            Message::Acp(event) => crate::acp::update(self, event),
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
        self.settings.skills_enabled = self.prompt.skills_enabled;
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
        self.settings.user_prompt = self.prompt.user_prompt.text();
        self.settings.dark_mode = theme::is_dark();
        self.settings.acp_server_enabled = self.acp.enabled;
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
        } else if self.overlay.show_revert_all_confirm {
            iced::widget::stack![main, crate::views::revert_all_modal().map(Message::Overlay)]
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
                    if self.layout.ctrl_held
                        && let Err(e) = open::that_detached(&url)
                    {
                        tracing::warn!("failed to open link '{url}': {e}");
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
                &self.pane_sections,
                &self.running_processes,
                self.settings.dark_mode,
                &self.acp,
            )
            .map(|event| match event {
                RightPaneEvent::ToggleTheme(dark) =>
                    Message::Layout(LayoutEvent::ToggleTheme(dark)),
                RightPaneEvent::Restart => Message::RestartApp,
                RightPaneEvent::RevertFile(path) => Message::RevertFile(path),
                RightPaneEvent::RevertAll => Message::RevertAll,
                RightPaneEvent::DismissRevertError => Message::DismissRevertError,
                RightPaneEvent::ToggleSection(section) => Message::TogglePaneSection(section),
                RightPaneEvent::ToggleAcpServer(enabled) => {
                    Message::Acp(crate::acp::AcpMessage::Toggle(enabled))
                }
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

/// Spawn a detached replacement for app restart, nulls stdio, and on Unix
/// starts a new session so the child outlives the parent's terminal.
pub(crate) fn spawn_relaunch(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&str],
) -> std::io::Result<std::process::Child> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            // New session: outlives the terminal, immune to SIGHUP.
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    cmd.spawn()
}
