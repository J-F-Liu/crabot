//! Per-tab left-pane prompt options.
//!
//! The live left-pane state mirrors the viewing tab; each tab keeps its own
//! [`PromptOptions`] snapshot. Prompt draft, attachments, and MCP connections
//! stay shared.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use iced::Task;

use crabot::tools;
use crabot::user::WorkMode;

use crate::app::{App, Message, PromptWorkspaceState, ToolState};

/// Left-pane options owned by a single session tab.
#[derive(Debug, Clone)]
pub(crate) struct PromptOptions {
    /// Model label picked in the model-config tab bar.
    pub(crate) selected_model: String,
    /// Preamble file picked next to the Preamble checkbox.
    pub(crate) selected_preamble: String,
    /// Whether the preamble section joins the system prompt.
    pub(crate) preamble_enabled: bool,
    /// Whether the selected skills join the system prompt.
    pub(crate) skills_enabled: bool,
    /// Skill files picked for this tab, in selection order.
    pub(crate) selected_skills: Vec<String>,
    /// Whether the generated tools summary joins the system prompt.
    pub(crate) tools_enabled: bool,
    /// Whether the workspace line joins the system prompt.
    pub(crate) workspace_enabled: bool,
    /// Workspace picked for this tab.
    pub(crate) workspace: PathBuf,
    /// AGENTS.md content of `workspace` when the tab was captured.
    pub(crate) agents_md: String,
    /// Whether the workspace's AGENTS.md joins the system prompt.
    pub(crate) agents_md_enabled: bool,
    /// Whether the current date joins the system prompt.
    pub(crate) date_enabled: bool,
    /// Whether the next prompt carries the workspace tree.
    pub(crate) files_enabled: bool,
    /// Selected work mode and whether it is active.
    pub(crate) workmode: WorkMode,
    pub(crate) workmode_enabled: bool,
    /// Enabled built-in, custom, and MCP tools for this tab.
    pub(crate) enabled_tools: HashSet<String>,
    /// Enabled MCP servers for this tab (connections themselves are shared).
    pub(crate) enabled_mcp_servers: HashSet<String>,
}

impl PromptOptions {
    /// The workspace this tab runs in: its own pick when that still exists,
    /// else `fallback`.
    pub(crate) fn workspace_or(&self, fallback: &Path) -> PathBuf {
        if self.workspace.as_os_str().is_empty() || !self.workspace.is_dir() {
            fallback.to_path_buf()
        } else {
            self.workspace.clone()
        }
    }

    /// Snapshot the live left-pane state, keeping the given model/preamble picks.
    pub(crate) fn from_live(
        prompt: &PromptWorkspaceState,
        tools: &ToolState,
        selected_model: String,
        selected_preamble: String,
    ) -> Self {
        Self {
            selected_model,
            selected_preamble,
            preamble_enabled: prompt.preamble_enabled,
            skills_enabled: prompt.skills_enabled,
            selected_skills: prompt.selected_skills.clone(),
            tools_enabled: prompt.tools.enabled,
            workspace_enabled: prompt.workspace.0,
            workspace: prompt.workspace.1.clone(),
            agents_md: prompt.agents_md.1.clone(),
            agents_md_enabled: prompt.agents_md.0,
            date_enabled: prompt.date.0,
            files_enabled: prompt.files.enabled,
            workmode: prompt.workmode,
            workmode_enabled: prompt.workmode_enabled,
            enabled_tools: tools.enabled_tools.clone(),
            enabled_mcp_servers: tools.enabled_mcp_servers.clone(),
        }
    }
}

impl App {
    /// Snapshot of the live left-pane options (viewing tab's model/preamble picks).
    pub(crate) fn capture_options(&self) -> PromptOptions {
        let tab = self.conversation.viewing();
        PromptOptions::from_live(
            &self.prompt,
            &self.tools,
            tab.pane.selected_model.clone(),
            tab.pane.selected_preamble.clone(),
        )
    }

    /// Options of the tab at `pos`: live state when it is viewing, else its snapshot.
    pub(crate) fn tab_options(&self, pos: usize) -> PromptOptions {
        if pos == self.conversation.viewing {
            self.capture_options()
        } else {
            self.conversation.session_tabs[pos].pane.clone()
        }
    }

    /// Options for a freshly opened tab: the viewing tab's, with the tree
    /// toggle back on (a new session has not sent a tree yet).
    pub(crate) fn new_tab_options(&self) -> PromptOptions {
        PromptOptions {
            files_enabled: true,
            ..self.capture_options()
        }
    }

    /// Make `pos` the viewing tab: save the outgoing options, restore the target's.
    pub(crate) fn set_viewing(&mut self, pos: usize) -> Task<Message> {
        let current = self.conversation.viewing;
        if current != pos {
            let live = self.capture_options();
            self.conversation.session_tabs[current].pane = live;
        }
        self.conversation.viewing = pos;
        self.apply_prompt_options()
    }

    /// Load the viewing tab's options into the live left-pane state.
    fn apply_prompt_options(&mut self) -> Task<Message> {
        let options = self.conversation.viewing().pane.clone();
        // The pane workspace falls back to where the session actually ran.
        let session_ws = self.conversation.viewing().session.workspace.clone();
        let workspace = options.workspace_or(&session_ws);
        // Switch first, then apply the toggles so the scans cannot win.
        let switch = workspace.is_dir() && workspace != self.prompt.workspace.1;
        let workspace_task = if switch {
            crate::app::prompt::restore_workspace(self, workspace)
        } else if workspace == self.prompt.workspace.1 {
            // No switch: the restored snapshot's AGENTS.md may be stale —
            // re-read it (toggle left alone).
            crate::app::prompt::scan_workspace_content(workspace, None)
        } else {
            Task::none()
        };

        self.prompt.restore(&options);
        self.tools.enabled_tools = options.enabled_tools;
        self.tools.enabled_mcp_servers = options.enabled_mcp_servers;
        self.refresh_tools_summary();

        // A workspace switch clears the tree; refill it when the pane is open.
        let tree_task = if self.prompt.files.expanded && self.prompt.files.tree.is_empty() {
            crate::app::prompt::request_files_tree(self)
        } else {
            Task::none()
        };
        workspace_task
            .chain(tree_task)
            .chain(connect_missing_mcp_servers(self))
    }
    /// Tools prompt section for a request on `pos`: the live text when it is
    /// the viewing tab, a summary of the tab's own tools otherwise.
    pub(crate) fn tools_section(&self, pos: usize, options: &PromptOptions) -> String {
        if pos == self.conversation.viewing {
            self.prompt.tools.content.text()
        } else {
            self.tools
                .summary_for(&options.enabled_tools, &options.enabled_mcp_servers)
        }
    }
}

impl PromptWorkspaceState {
    /// Load the per-tab components and toggles from `options`.
    fn restore(&mut self, options: &PromptOptions) {
        self.preamble_enabled = options.preamble_enabled;
        self.skills_enabled = options.skills_enabled;
        self.selected_skills.clone_from(&options.selected_skills);
        self.tools.enabled = options.tools_enabled;
        self.workspace.0 = options.workspace_enabled;
        // Keep the cached AGENTS.md; a workspace switch rescans it off-thread.
        self.agents_md = (options.agents_md_enabled, options.agents_md.clone());
        self.date.0 = options.date_enabled;
        self.files.enabled = options.files_enabled;
        self.workmode = options.workmode;
        self.workmode_enabled = options.workmode_enabled;
        // Dropdowns never stay open across tabs.
        self.skills_menu_expanded = false;
        self.recipe_dropdown_expanded = false;
        self.attachment_menu_expanded = false;
    }
}

/// Start discovery for enabled MCP servers that have no live connection.
fn connect_missing_mcp_servers(app: &mut App) -> Task<Message> {
    let missing: Vec<tools::mcp::McpServer> = app
        .tools
        .enabled_mcp_servers
        .iter()
        .filter(|server| {
            !app.tools.mcp_pending.contains(*server)
                && !app.tools.mcp_errors.contains_key(*server)
                && !tools::mcp::has_connection(server)
        })
        .filter_map(|server| app.tools.tool_registry.find_mcp_server(server))
        .collect();
    missing
        .into_iter()
        .map(|server| app.tools.start_discovery(server))
        .fold(Task::none(), Task::chain)
}
