use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::system::SystemPrompt;
use crate::tools::ToolRegistry;

/// All persistable app-level state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub left_pane_width: f32,
    pub right_pane_width: f32,
    pub window_size: (f32, f32),
    pub window_pos: (f32, f32),
    pub selected_model: String,
    pub selected_preamble: String,
    pub selected_rules: String,
    /// Enabled status for each system-prompt component.
    pub preamble_enabled: bool,
    pub rules_enabled: bool,
    pub tools_enabled: bool,
    pub workspace_enabled: bool,
    pub agents_md_enabled: bool,
    pub files_enabled: bool,
    pub date_enabled: bool,
    /// Current workspace path.
    pub workspace: PathBuf,
    /// Recent workspaces as `(path, agents_md_enabled)` tuples, most recent first.
    pub recent_workspaces: Vec<(PathBuf, bool)>,
    /// Font scale factor for center pane dialog blocks (0.5 .. 2.0).
    pub font_scale: f32,
    /// Enabled MCP servers: server name → enabled.
    pub mcp_servers: IndexMap<String, bool>,
    /// Enabled agent tools: tool name → enabled.
    pub agent_tools: IndexMap<String, bool>,
    /// Prompt recipes: work-mode name (lowercase) → list of prompt templates.
    pub prompt_recipe: IndexMap<String, Vec<String>>,
    /// Latest version found in the last check, if newer than current.
    pub last_update_version: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            left_pane_width: 300.0,
            right_pane_width: 280.0,
            window_size: (1200.0, 800.0),
            window_pos: (0.0, 0.0),
            selected_model: String::new(),
            selected_preamble: String::new(),
            selected_rules: String::new(),
            preamble_enabled: true,
            rules_enabled: true,
            tools_enabled: true,
            workspace_enabled: true,
            agents_md_enabled: true,
            files_enabled: true,
            date_enabled: true,
            workspace: PathBuf::new(),
            recent_workspaces: Vec::new(),
            font_scale: 1.0,
            mcp_servers: IndexMap::new(),
            agent_tools: IndexMap::new(),
            prompt_recipe: IndexMap::new(),
            last_update_version: None,
        }
    }
}

impl Settings {
    /// Path to `~/.crabot/settings.ron`.
    pub fn path() -> PathBuf {
        home::home_dir()
            .unwrap_or_default()
            .join(".crabot")
            .join("settings.ron")
    }

    /// Load settings from disk, returning defaults if file is missing or malformed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => ron::from_str::<Settings>(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Copy enabled flags and workspace path from a `SystemPrompt`.
    pub fn sync_system_prompt(&mut self, prompt: &SystemPrompt) {
        self.preamble_enabled = prompt.preamble.0;
        self.rules_enabled = prompt.rules.0;
        self.tools_enabled = prompt.tools.0;
        self.workspace_enabled = prompt.workspace.0;
        self.agents_md_enabled = prompt.agents_md.0;
        self.date_enabled = prompt.date.0;
        self.workspace = prompt.workspace.1.clone();
    }

    /// Rebuild `mcp_servers` and `agent_tools` from live registry state.
    pub fn sync_tools(
        &mut self,
        registry: &ToolRegistry,
        enabled_tools: &HashSet<String>,
        enabled_mcp_servers: &HashSet<String>,
    ) {
        self.mcp_servers = registry
            .mcp_servers
            .iter()
            .map(|s| (s.name.clone(), enabled_mcp_servers.contains(&s.name)))
            .collect();
        self.agent_tools = registry
            .all_names()
            .map(|name| {
                let enabled = enabled_tools.contains(name);
                (name.clone(), enabled)
            })
            .collect();
    }

    /// Look up whether a tool is enabled in saved agent-tool preferences.
    pub fn is_tool_enabled(&self, name: &str) -> bool {
        self.agent_tools.get(name).copied().unwrap_or(false)
    }

    /// Set `agents_md_enabled` for a workspace path in recents.
    pub fn set_recent_workspace_enabled(&mut self, path: &PathBuf, enabled: bool) {
        if let Some(entry) = self.recent_workspaces.iter_mut().find(|(p, _)| p == path) {
            entry.1 = enabled;
        } else {
            self.recent_workspaces.push((path.clone(), enabled));
        }
    }

    /// Save settings to disk as RON text.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()) {
            let _ = std::fs::write(&path, text);
        }
    }
}
