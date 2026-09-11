use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::i18n::Lang;
use crate::model::TaskModels;
use crate::tools::{ToolLimits, ToolRegistry};

/// Chat text-size zoom bounds, shared by the settings slider and Ctrl+/-.
pub const FONT_SCALE_MIN: f32 = 0.5;
pub const FONT_SCALE_MAX: f32 = 2.0;
/// Chat text-size zoom step (5%), used by Ctrl+/- and the settings slider.
pub const FONT_SCALE_STEP: f32 = 0.05;

/// Snap a font scale to the 5% step grid within the supported range.
pub fn snap_font_scale(scale: f32) -> f32 {
    ((scale / FONT_SCALE_STEP).round() * FONT_SCALE_STEP).clamp(FONT_SCALE_MIN, FONT_SCALE_MAX)
}

/// All persistable app-level state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub left_pane_width: f32,
    pub right_pane_width: f32,
    pub window_size: (f32, f32),
    pub window_pos: (f32, f32),
    /// Model label used to seed the first session tab at startup.
    pub selected_model: String,
    pub selected_preamble: String,
    /// Skill files selected for the system prompt, in selection order.
    pub selected_skills: Vec<String>,
    /// Enabled status for each system-prompt component.
    pub preamble_enabled: bool,
    /// Whether the selected skills are included in the system prompt.
    pub skills_enabled: bool,
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
    /// Route LLM API traffic through the Windows system proxy.
    pub use_system_proxy_for_llm: bool,
    /// Route tool HTTP (fetch, sandbox curl/wget, spawned processes) through it.
    pub use_system_proxy_for_tools: bool,
    /// Latest version found in the last check, if newer than current.
    pub last_update_version: Option<String>,
    /// Whether the dark color theme is active.
    pub dark_mode: bool,
    /// UI language.
    pub language: Lang,
    /// Renderer backend (`ICED_BACKEND` value); `auto`/empty = wgpu → tiny-skia.
    pub iced_backend: String,
    /// Whether the built-in ACP HTTP server is enabled.
    pub acp_server_enabled: bool,
    /// Loopback port for the ACP HTTP server.
    pub acp_server_port: u16,
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
            selected_skills: Vec::new(),
            preamble_enabled: true,
            skills_enabled: true,
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
            use_system_proxy_for_llm: true,
            use_system_proxy_for_tools: true,
            last_update_version: None,
            dark_mode: false,
            language: Lang::default(),
            iced_backend: String::from("auto"),
            acp_server_enabled: false,
            acp_server_port: 8787,
        }
    }
}

/// Adopt each disk field that `ours` did not change since `baseline`. The list
/// holds every setting not owned by live UI state (`App::synced_settings`); add
/// new persistent fields here or they will never merge across instances.
macro_rules! merge_untouched {
    ($ours:expr, $baseline:expr, $disk:expr, $merged:expr; $($field:ident),* $(,)?) => {
        $(
            if $ours.$field == $baseline.$field {
                $merged.$field = $disk.$field.clone();
            }
        )*
    };
}

impl Settings {
    /// Path to `~/.crabot/settings.ron`.
    pub fn path() -> PathBuf {
        crate::setup::config_dir().join("settings.ron")
    }

    /// Load settings from disk, returning defaults when the file is missing or
    /// unparsable; a malformed file is moved aside and its `.bak` is used when
    /// available.
    pub fn load() -> Self {
        crate::atomic::load_ron(&Self::path()).unwrap_or_default()
    }

    /// Refresh `mcp_servers` / `agent_tools` from the live registry. Unknown
    /// names stay while enabled (MCP tools appear only after discovery); stale
    /// disabled ones are pruned so the file stops growing.
    pub fn sync_tools(
        &mut self,
        registry: &ToolRegistry,
        enabled_tools: &HashSet<String>,
        enabled_mcp_servers: &HashSet<String>,
    ) {
        self.mcp_servers.retain(|name, enabled| {
            *enabled || registry.mcp_servers.iter().any(|s| &s.name == name)
        });
        self.agent_tools
            .retain(|name, enabled| *enabled || registry.all_names().any(|n| n == name));
        for server in &registry.mcp_servers {
            let enabled = enabled_mcp_servers.contains(&server.name);
            self.mcp_servers.insert(server.name.clone(), enabled);
        }
        for name in registry.all_names() {
            let enabled = enabled_tools.contains(name);
            self.agent_tools.insert(name.clone(), enabled);
        }
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

    /// Save, merging concurrent edits from another instance: locally changed
    /// fields win, untouched fields take the on-disk value. On error the file is
    /// untouched, so the caller can retry with the same baseline.
    pub fn save_merged(&self, baseline: &Settings) -> std::io::Result<Settings> {
        let path = Self::path();
        crate::atomic::with_lock(&path, || {
            // `load_ron` moves an unparsable file aside instead of overwriting it.
            let disk = crate::atomic::load_ron::<Settings>(&path);
            let merged = match &disk {
                Some(disk) if disk != baseline => merge_settings(baseline, self, disk),
                _ => self.clone(),
            };
            // Skip the write (and its mtime bump) when the file already holds this.
            if disk.as_ref() != Some(&merged) {
                crate::atomic::save_ron(&path, &merged, ron::ser::PrettyConfig::default())?;
            }
            Ok(merged)
        })
    }
}

/// Three-way merge: `ours` wins for locally changed fields, `disk` fills in the
/// rest.
fn merge_settings(baseline: &Settings, ours: &Settings, disk: &Settings) -> Settings {
    let mut merged = ours.clone();
    merge_untouched!(ours, baseline, disk, merged;
        left_pane_width,
        right_pane_width,
        selected_skills,
        recent_workspaces,
        font_scale,
        prompt_recipes,
        fill_ratio_threshold,
        max_iterations,
        stream_stall_timeout,
        tool_limits,
        task_models,
        auto_check_updates,
        use_system_proxy_for_llm,
        use_system_proxy_for_tools,
        last_update_version,
        language,
        iced_backend,
        // Read only when the ACP listener next starts; never auto-restarted.
        acp_server_port,
    );
    merged
}
