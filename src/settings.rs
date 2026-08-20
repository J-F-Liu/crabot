use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::model::TaskModels;
use crate::tools::{ToolLimits, ToolRegistry};

/// All persistable app-level state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub left_pane_width: f32,
    pub right_pane_width: f32,
    pub window_size: (f32, f32),
    pub window_pos: (f32, f32),
    /// Model label used to seed the first session tab at startup.
    pub selected_model: String,
    pub selected_preamble: String,
    pub selected_rules: String,
    /// Enabled status for each system-prompt component.
    pub preamble_enabled: bool,
    pub rules_enabled: bool,
    pub tools_enabled: bool,
    pub workspace_enabled: bool,
    pub agents_md_enabled: bool,
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
    /// Leftover text in the user prompt input box, restored on startup.
    pub user_prompt: String,
    /// Prompt recipes: work-mode name (lowercase) → list of prompt templates.
    pub prompt_recipes: IndexMap<String, Vec<String>>,
    /// Context-window fill ratio threshold (%) that triggers a renew reminder.
    pub fill_ratio_threshold: f32,
    /// Max agent-loop iterations (tool-calling rounds) before giving up.
    pub max_iterations: usize,
    /// Seconds of LLM stream silence before giving up (0 = off).
    pub stream_stall_timeout: u64,
    /// Configurable limits for the built-in tools (timeouts, output caps, …).
    pub tool_limits: ToolLimits,
    /// Sub-agent model per difficulty tier used by the `task` tool.
    pub task_models: TaskModels,
    /// Whether to automatically check for new versions on startup.
    pub auto_check_updates: bool,
    /// Latest version found in the last check, if newer than current.
    pub last_update_version: Option<String>,
    /// Whether the dark color theme is active.
    pub dark_mode: bool,
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
            date_enabled: true,
            workspace: PathBuf::new(),
            recent_workspaces: Vec::new(),
            font_scale: 1.0,
            mcp_servers: IndexMap::new(),
            agent_tools: IndexMap::new(),
            user_prompt: String::new(),
            prompt_recipes: IndexMap::new(),
            fill_ratio_threshold: 25.0,
            max_iterations: 100,
            // Anthropic heartbeats every ~15-30s; 120s of silence means a dead stream.
            stream_stall_timeout: 120,
            tool_limits: ToolLimits::default(),
            task_models: TaskModels::default(),
            auto_check_updates: true,
            last_update_version: None,
            dark_mode: false,
        }
    }
}

impl Settings {
    /// Path to `~/.crabot/settings.ron`.
    pub fn path() -> PathBuf {
        crate::setup::config_dir().join("settings.ron")
    }

    /// Load settings from disk, returning defaults if file is missing or malformed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match ron::from_str::<Settings>(&text) {
                Ok(settings) => settings,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to parse settings, using defaults: {e}");
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!(path = %path.display(), "failed to read settings, using defaults: {e}");
                Self::default()
            }
        }
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
        match ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    tracing::error!(path = %path.display(), "failed to save settings: {e}");
                }
            }
            Err(e) => tracing::error!("failed to serialize settings: {e}"),
        }
    }
}
