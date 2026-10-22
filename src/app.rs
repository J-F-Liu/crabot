//! Application state, message routing, and the Iced Program implementation.
//!
//! This module owns the root [`App`] state, the nested domain [`Message`] enum,
//! and the boot / update / view / subscription methods that drive the GUI.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;

use iced::widget::scrollable::Viewport;
use iced::widget::{Space, column, container, row, text_editor};
use iced::{Element, Length, Point, Size, Subscription, Task, Theme};

use crabot::model::{self, ModelConfig, ModelList};
use crabot::settings::{Appearance, FONT_SCALE_MAX, FONT_SCALE_MIN};
use crabot::setup;
use crabot::tools;
use crabot::user::{AttachmentKind, UserAttachment, UserPrompt, WorkMode};
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
pub(crate) mod diff;
mod layout;
mod overlay;
pub(crate) mod prompt;
pub(crate) mod prompt_options;
pub(crate) mod session_state;
mod session_tab;
mod settings;
pub(crate) mod snapshot;
mod subscription;
mod tool_state;

pub(crate) use prompt_options::PromptOptions;
pub(crate) use session_tab::{AttachmentPreview, PreviewContent, SessionEndStatus, SessionTab};

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
    /// Last visible width, restored when the right pane is shown again.
    pub(crate) right_pane_restore_width: f32,
    /// Active color theme; the matching dark flag lives in `views::theme`.
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
    /// Skill files picked for the viewing tab, in selection order.
    pub(crate) selected_skills: Vec<String>,
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
    /// Files picked for the next prompt, cleared after sending.
    pub(crate) attachments: Vec<UserAttachment>,
    /// Whether the attachment menu is open.
    pub(crate) attachment_menu_expanded: bool,
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

    /// Concatenate the enabled components of `options`, returning the full
    /// system prompt. `tools` is that tab's tools section; preamble and skills
    /// are read from disk on every call.
    pub(crate) fn get_system_prompt(&self, options: &PromptOptions, tools: &str) -> String {
        let preamble = options
            .preamble_enabled
            .then(|| prompt::load_prompt_file(&self.preamble_options, &options.selected_preamble));
        self.compose_system_prompt(options, preamble.as_deref(), tools)
    }

    /// Like [`get_system_prompt`](Self::get_system_prompt), but with a
    /// caller-provided preamble replacing the configured one (`None` omits it).
    pub(crate) fn compose_system_prompt(
        &self,
        options: &PromptOptions,
        preamble: Option<&str>,
        tools: &str,
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
        if options.workmode_enabled
            && let Some(contents) = crabot::setup::ASSETS
                .get_file("workmode.md")
                .and_then(|file| file.contents_utf8())
        {
            section(&mut prompt, contents);
        }
        if options.skills_enabled {
            section(
                &mut prompt,
                &prompt::load_prompt_files(&self.skills_options, &options.selected_skills),
            );
        }
        if options.tools_enabled {
            section(&mut prompt, tools);
        }
        if options.workspace_enabled && options.workspace.is_dir() {
            let path = crabot::tools::convert_path_to_unix_style(&options.workspace);
            prompt.push_str(&format!("Current Workspace: {}\n", path));
        }
        if options.agents_md_enabled {
            section(&mut prompt, &options.agents_md);
        }
        if options.date_enabled && !self.date.1.is_empty() {
            prompt.push_str(&format!("Current Date: {}\n", self.date.1));
        }
        prompt
    }
}

/// Tool registry, enabled-tool sets.
pub(crate) struct ToolState {
    pub(crate) tool_registry: tools::ToolRegistry,
    pub(crate) enabled_tools: HashSet<String>,
    pub(crate) enabled_mcp_servers: HashSet<String>,
    /// Latest failure per MCP server, shown next to its name in the left pane.
    pub(crate) mcp_errors: HashMap<String, tools::mcp::DiscoverFailure>,
    /// Servers with an in-flight discovery; shown as "Connecting..."/"Reconnect...".
    pub(crate) mcp_pending: HashSet<String>,
    /// Per-server discovery counter; results tagged with an older epoch are stale.
    pub(crate) mcp_discovery_epoch: HashMap<String, u64>,
    /// Pre-reload tool names per server, so a reload only enables genuinely new tools.
    pub(crate) mcp_known_tools: HashMap<String, HashSet<String>>,
    pub(crate) tool_list_state: ToolListState,
}

impl ToolState {
    /// Bump the discovery epoch for `server`, returning it for the spawned task.
    pub(crate) fn next_epoch(&mut self, server: &str) -> u64 {
        let epoch = self
            .mcp_discovery_epoch
            .entry(server.to_string())
            .or_insert(0);
        *epoch += 1;
        *epoch
    }

    /// Drop a server's connection, discovery state, and tools, bumping its epoch
    /// so in-flight discoveries go stale. Returns the removed tool names, keeping
    /// enabled state for names a built-in, custom, or other MCP tool still claims.
    pub(crate) fn drop_mcp_server(&mut self, server: &str) -> Vec<String> {
        tools::mcp::drop_connection(server);
        self.enabled_mcp_servers.remove(server);
        self.mcp_errors.remove(server);
        self.mcp_pending.remove(server);
        self.mcp_known_tools.remove(server);
        self.tool_list_state.forget_server(server);
        self.next_epoch(server);
        let removed: Vec<String> = self
            .tool_registry
            .take_mcp_group(server)
            .into_iter()
            .map(|tool| tool.name)
            .filter(|name| !self.tool_registry.owns_name(name))
            .collect();
        self.enabled_tools.retain(|name| !removed.contains(name));
        removed
    }

    /// Start discovery for `server`, superseding any in-flight one.
    pub(crate) fn start_discovery(&mut self, server: tools::mcp::McpServer) -> Task<Message> {
        let epoch = self.next_epoch(&server.name);
        self.mcp_pending.insert(server.name.clone());
        Task::perform(
            async move { tools::mcp::discover_mcp_server(server, epoch).await },
            move |(name, result)| {
                Message::Tools(ToolEvent::McpToolsDiscovered((name, epoch, result)))
            },
        )
    }

    /// Generate an XML-formatted summary of the live enabled tools.
    pub(crate) fn summary(&self) -> String {
        self.summary_for(&self.enabled_tools, &self.enabled_mcp_servers)
    }

    /// Same summary for an arbitrary tool set (a tab's own).
    pub(crate) fn summary_for(
        &self,
        enabled: &HashSet<String>,
        enabled_mcp_servers: &HashSet<String>,
    ) -> String {
        let all_tools = self
            .tool_registry
            .enabled_tools(enabled, enabled_mcp_servers);
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
            if enabled_mcp_servers.contains(&server.name)
                && !server.prompt.is_empty()
                && self
                    .tool_registry
                    .mcp_server_has_enabled_tool(&server.name, enabled)
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
    pub(crate) fn new(options: PromptOptions) -> Self {
        Self {
            session_tabs: vec![SessionTab::new(1, options)],
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
    pub(crate) fn status(&self, lang: crabot::i18n::Lang) -> Cow<'static, str> {
        self.viewing()
            .session_state
            .status(lang, self.viewing().session.is_empty())
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
    /// Settings as last written to disk; baseline for the save merge.
    pub settings_saved: crabot::settings::Settings,
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
    /// Show or hide the right pane.
    ToggleRightPane,
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
    /// Open/close the attachment type menu.
    ToggleAttachmentMenu,
    /// Dismiss the attachment menu (outside click / Escape).
    DismissAttachmentMenu,
    /// Open the file picker for the given attachment type.
    AttachFile(AttachmentKind),
    /// Returned by the attachment file picker (None when cancelled).
    AttachmentDialogResult(AttachmentKind, Option<Vec<PathBuf>>),
    /// Returned by the attachment validator: accepted files and rejection reasons.
    AttachmentsValidated(Vec<UserAttachment>, Vec<String>),
    /// Remove the selected attachment at the given index.
    RemoveAttachment(usize),
    SendPrompt,
}

/// Events related to tools and MCP server management.
#[derive(Clone)]
pub(crate) enum ToolEvent {
    ToggleMcpServer(String, bool),
    ToggleAgentTool(String, bool),
    /// Select or clear every tool of one MCP server.
    SetMcpServerTools(String, bool),
    /// Cycle one MCP server's tool grid between 1, 2, and 3 columns.
    CycleMcpColumns(String),
    /// Expand or collapse one MCP server's tool grid.
    ToggleMcpServerExpanded(String),
    McpToolsDiscovered((String, u64, crabot::tools::mcp::DiscoverResult)),
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
    /// Toggle the inline preview for a file attachment.
    ToggleAttachmentPreview(String, AttachmentKind),
    /// A decoded preview for (tab number, attachment path, request id).
    AttachmentPreviewLoaded(usize, String, u64, Result<PreviewContent, String>),
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
    /// Show or hide the right pane.
    ToggleRightPane,
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
    /// Open the snapshot ↔ current diff of a modified file in VS Code.
    OpenDiff(String),
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
    /// Open the snapshot ↔ current diff of a modified file.
    OpenDiff(String),
    /// Diff launch finished: (tab number, Ok | Err(message)).
    DiffDone(usize, Result<(), String>),
    DismissRevertError,
    /// Expand or collapse a right-pane section.
    TogglePaneSection(PaneSection),
    /// Managed process started/exited; refresh the cached right-pane list.
    ProcessTick,
    /// Emitted at each local midnight to refresh date-dependent state.
    DateTick,
    /// The OS switched between light and dark appearance.
    SystemThemeChanged(bool),
    /// ACP HTTP server bridge events.
    Acp(crate::acp::AcpMessage),
}

// ── App impl ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn boot(mut saved: crabot::settings::Settings) -> (Self, Task<Message>) {
        let models = model::load_models();
        // Baseline before boot-time fixups, so those count as local changes.
        let settings_saved = saved.clone();
        saved.selected_model = models.ensure_valid_label(&saved.selected_model);

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
            mcp_errors: HashMap::new(),
            mcp_pending: HashSet::new(),
            mcp_discovery_epoch: HashMap::new(),
            mcp_known_tools: HashMap::new(),
            tool_list_state: ToolListState::default(),
        };
        let tools_summary = tools.summary();
        let enabled_tools_count = tools.enabled_tools.len();
        let enabled_mcp_count = tools.enabled_mcp_servers.len();

        let date_str = prompt::current_date();
        let update_available = saved
            .last_update_version
            .as_ref()
            .filter(|v| crate::views::update::version_gt(v, crate::views::update::CURRENT_VERSION))
            .cloned();

        let workspace_options = crate::views::build_workspace_options(&saved.recent_workspaces);
        let window_size = Size::new(saved.window_size.0, saved.window_size.1);
        let window_pos = Point::new(saved.window_pos.0, saved.window_pos.1);
        let tools_enabled = saved.tools_enabled;
        let dark = saved.appearance.is_dark();
        let acp_server_enabled = saved.acp_server_enabled;
        theme::set_dark_mode(dark);

        let initial_selected_model = saved.selected_model.clone();
        let initial_selected_preamble = if preamble_options.iter().any(|e| e.display == "crabot") {
            "crabot".to_string()
        } else {
            saved.selected_preamble.clone()
        };

        let prompt = PromptWorkspaceState {
            preamble_enabled: saved.preamble_enabled,
            skills_enabled: saved.skills_enabled,
            selected_skills: saved.selected_skills.clone(),
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
            attachments: Vec::new(),
            attachment_menu_expanded: false,
            skills_menu_expanded: false,
        };

        // The first tab starts from the persisted left-pane state.
        let initial_options = PromptOptions::from_live(
            &prompt,
            &tools,
            initial_selected_model,
            initial_selected_preamble,
        );

        let models_count = models.models.len();
        let right_pane_restore_width = saved.right_pane_width.max(theme::MIN_W);
        let mut app = Self {
            settings: saved,
            layout: LayoutState {
                window_size,
                window_pos,
                cursor: Point::ORIGIN,
                left_divider: DividerState::default(),
                right_divider: DividerState::default(),
                right_pane_restore_width,
                theme: theme::theme_for(dark),
                shift_held: false,
                ctrl_held: false,
                scroll_viewport_height: 0.0,
                focused: None,
                window_focused: true,
            },
            prompt,
            tools,
            conversation: ConversationState::new(initial_options),
            models,
            settings_saved,
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
            dark_mode = dark,
            "crabot boot complete"
        );
        // Boot-time session list scan: show the loading placeholder until it lands.
        app.conversation.session_list_loading = !workspace_path.as_os_str().is_empty();
        let session_task = conversation::refresh_session_list(workspace_path.clone());
        let workspace_task = prompt::scan_workspace_content(workspace_path, None);
        let discover_task = mcp_list
            .servers
            .into_iter()
            .map(|server| app.tools.start_discovery(server))
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
        // HTTP mode resumes from settings; stdio mode starts its own transport.
        let acp_task = if app.acp.stdio {
            crate::acp::start_stdio(&mut app)
        } else if app.settings.acp_server_enabled {
            crate::acp::start_http(&mut app)
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

    /// App title plus the viewing tab's workspace folder name, when set.
    pub(crate) fn window_title(&self) -> String {
        let app = crabot::app_title();
        match crabot::workspace::folder_name(&self.prompt.workspace.1) {
            Some(name) => format!("{app} - {name}"),
            None => app.to_string(),
        }
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
                            if let Err(e) = spawn_detached(&exe, &[]) {
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
            Message::OpenDiff(path) => diff::open_diff(self, path),
            Message::DiffDone(number, outcome) => diff::diff_done(self, number, outcome),
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
            Message::DateTick => {
                prompt::refresh_date(self);
                Task::none()
            }
            Message::SystemThemeChanged(dark) => {
                // Only "follow system" tracks the OS; an explicit choice wins.
                if self.settings.appearance == Appearance::System {
                    self.set_dark(dark);
                }
                Task::none()
            }
            Message::Acp(event) => crate::acp::update(self, event),
        }
    }

    // ── Settings helpers ──────────────────────────────────────────

    /// Confirm a pending new-label input (Enter or focus loss).
    pub(crate) fn confirm_pending_label(&mut self) {
        settings::confirm_pending_label(self);
    }

    /// Apply a chat font scale; cached search highlight offsets depend on text sizes.
    pub(crate) fn set_font_scale(&mut self, scale: f32) {
        self.settings.font_scale = scale.clamp(FONT_SCALE_MIN, FONT_SCALE_MAX);
        for tab in &mut self.conversation.session_tabs {
            tab.search.invalidate_offsets();
        }
    }

    /// Switch the live theme; the persisted appearance preference is unchanged.
    pub(crate) fn set_dark(&mut self, dark: bool) {
        theme::set_dark_mode(dark);
        self.layout.theme = theme::theme_for(dark);
    }

    /// Apply a persisted appearance preference (`System` asks the OS now).
    pub(crate) fn apply_appearance(&mut self, appearance: Appearance) {
        self.set_dark(appearance.is_dark());
    }

    /// `settings` with live UI values (window geometry, prompt text, tool
    /// toggles, …) written back; these always win over the on-disk copy.
    fn synced_settings(&self) -> crabot::settings::Settings {
        let mut settings = self.settings.clone();
        settings.selected_model = self.conversation.viewing().pane.selected_model.clone();
        settings.selected_preamble = self.conversation.viewing().pane.selected_preamble.clone();
        settings.selected_skills = self.prompt.selected_skills.clone();
        settings.window_size = (
            self.layout.window_size.width,
            self.layout.window_size.height,
        );
        settings.window_pos = (
            self.layout.window_pos.x.max(0.0),
            self.layout.window_pos.y.max(0.0),
        );
        settings.preamble_enabled = self.prompt.preamble_enabled;
        settings.skills_enabled = self.prompt.skills_enabled;
        settings.workspace_enabled = self.prompt.workspace.0;
        settings.agents_md_enabled = self.prompt.agents_md.0;
        settings.date_enabled = self.prompt.date.0;
        settings.workspace = self.prompt.workspace.1.clone();
        settings.tools_enabled = self.prompt.tools.enabled;
        settings.sync_tools(
            &self.tools.tool_registry,
            &self.tools.enabled_tools,
            &self.tools.enabled_mcp_servers,
        );
        settings.user_prompt = self.prompt.user_prompt.text();
        // stdio mode is host-driven; don't persist the toggle into settings.
        if !self.acp.stdio {
            settings.acp_server_enabled = self.acp.enabled;
        }
        settings
    }

    /// Persist settings, merging other instances' concurrent edits, then
    /// re-apply runtime state for values the merge adopted from disk. Called on
    /// exit/restart and by explicit saves; there is no periodic auto-save.
    pub(crate) fn save_settings(&mut self) {
        self.settings = self.synced_settings();
        // In-memory state already matches the last write.
        if self.settings == self.settings_saved {
            return;
        }
        let ours = self.settings.clone();
        match self.settings.save_merged(&self.settings_saved) {
            Ok(merged) => {
                self.settings = merged.clone();
                self.apply_merged_settings(&ours, &merged);
                self.settings_saved = merged;
            }
            Err(e) => tracing::warn!("failed to save settings: {e}"),
        }
    }

    /// Re-apply runtime state for settings `merged` adopted from disk, i.e. the
    /// ones `ours` had not changed; live UI state stays per-instance. Runs once
    /// `self.settings` already holds `merged`, so clamps applied here (font
    /// scale) are what the next save persists. An adopted `acp_server_port`
    /// takes effect only on the next manual ACP start.
    fn apply_merged_settings(
        &mut self,
        ours: &crabot::settings::Settings,
        merged: &crabot::settings::Settings,
    ) {
        if merged.tool_limits != ours.tool_limits {
            tools::init_tool_limits(merged.tool_limits);
        }
        if merged.font_scale != ours.font_scale {
            self.set_font_scale(merged.font_scale);
        }
        if merged.appearance != ours.appearance {
            self.apply_appearance(merged.appearance);
        }
    }

    fn get_current_model(&self) -> Option<&ModelConfig> {
        self.conversation
            .viewing()
            .session
            .model
            .as_ref()
            .or_else(|| {
                self.models
                    .get_config(&self.conversation.viewing().pane.selected_model)
            })
    }

    /// Look up the currently selected model's config, cloned for ownership.
    pub(crate) fn selected_model_config(&self) -> Option<ModelConfig> {
        self.models
            .get_config(&self.conversation.viewing().pane.selected_model)
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
            .unwrap_or_else(|| self.conversation.viewing().pane.selected_model.clone())
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
            let backdrop = container(Space::new().width(Length::Fill).height(Length::Fill))
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
                crate::views::workspace_modal(
                    &self.overlay.default_workspace_path,
                    self.settings.language,
                )
                .map(Message::Overlay),
            ]
            .into()
        } else if self.overlay.show_revert_all_confirm {
            iced::widget::stack![
                main,
                crate::views::revert_all_modal(self.settings.language).map(Message::Overlay)
            ]
            .into()
        } else {
            iced::widget::stack![main].into()
        }
    }

    /// The three-pane layout (left, center, right) with dividers.
    fn view_main_content(&self) -> Element<'_, Message> {
        let model = self.get_current_model();
        let right_pane_collapsed = self.settings.right_pane_width <= 0.0;
        let right_side: Element<'_, Message> = if right_pane_collapsed {
            // Collapsed: skip laying out the pane content entirely.
            Space::new().into()
        } else {
            row![
                divider(&self.layout.right_divider),
                right_pane(
                    self.settings.right_pane_width,
                    model,
                    self.conversation.viewing(),
                    &self.pane_sections,
                    &self.running_processes,
                    theme::is_dark(),
                    &self.acp,
                    self.settings.language,
                )
                .map(|event| match event {
                    RightPaneEvent::ToggleTheme(dark) => {
                        Message::Layout(LayoutEvent::ToggleTheme(dark))
                    }
                    RightPaneEvent::Restart => Message::RestartApp,
                    RightPaneEvent::RevertFile(path) => Message::RevertFile(path),
                    RightPaneEvent::RevertAll => Message::RevertAll,
                    RightPaneEvent::OpenDiff(path) => Message::OpenDiff(path),
                    RightPaneEvent::DismissRevertError => Message::DismissRevertError,
                    RightPaneEvent::ToggleSection(section) => Message::TogglePaneSection(section),
                    RightPaneEvent::ToggleAcpServer(enabled) => {
                        Message::Acp(crate::acp::AcpMessage::Toggle(enabled))
                    }
                }),
            ]
            .into()
        };
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
                self.settings.language,
                model,
                right_pane_collapsed,
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
                CenterPaneEvent::ToggleRightPane => {
                    Message::Layout(LayoutEvent::ToggleRightPane)
                }
            }),
            right_side,
        ]
        .spacing(0)
        .into()
    }

    /// Optionally prepend the update-available banner.
    fn view_with_banner<'a>(&'a self, body: Element<'a, Message>) -> Element<'a, Message> {
        if let Some(latest) = &self.overlay.update_available {
            column![
                crate::views::update::update_banner(
                    latest,
                    &self.overlay.download_state,
                    self.settings.language,
                )
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

/// Spawn a child that outlives crabot: nulled stdio, no console window, a new
/// Unix session, and a reaper thread so it never becomes a zombie.
pub(crate) fn spawn_detached(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&std::ffi::OsStr],
) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
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
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
