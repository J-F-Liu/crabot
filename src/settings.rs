use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::model::ModelConfig;
use crate::system::SystemPrompt;
use crate::user::WorkMode;

/// All persistable app-level state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub left_w: f32,
    pub right_w: f32,
    pub window_size: (f32, f32),
    pub window_pos: (f32, f32),
    pub selected_model: Option<ModelConfig>,
    pub system_prompt: SystemPrompt,
    pub rules_expanded: bool,
    pub tools_expanded: bool,
    pub files_expanded: bool,
    pub selected_preamble: String,
    /// Recent workspace paths (most recent first).
    pub recent_workspaces: Vec<PathBuf>,
    pub rules_text: String,
    pub tools_text: String,
    pub files_text: String,
    /// Enabled dev tools by name → bool.
    pub dev_tools: Vec<(String, bool)>,
    pub workmode: WorkMode,
}

impl Default for Settings {
    fn default() -> Self {
        let tools_summary = crate::tool::tools_summary(
            &crate::tools::DevTool::ALL
                .iter()
                .map(|&t| (t, true))
                .collect(),
        );
        Self {
            left_w: 300.0,
            right_w: 280.0,
            window_size: (1200.0, 800.0),
            window_pos: (0.0, 0.0),
            selected_model: None,
            system_prompt: SystemPrompt {
                preamble: (true, String::new()),
                rules: (true, String::new()),
                tools: (true, tools_summary.clone()),
                workspace: (true, PathBuf::new()),
                files: (true, String::new()),
                date: (true, chrono::Local::now().format("%Y-%m-%d").to_string()),
            },
            rules_expanded: false,
            tools_expanded: false,
            files_expanded: false,
            selected_preamble: String::new(),
            recent_workspaces: Vec::new(),
            rules_text: String::new(),
            tools_text: tools_summary,
            files_text: String::new(),
            dev_tools: crate::tools::DevTool::ALL
                .iter()
                .map(|t| (t.name().to_string(), true))
                .collect(),
            workmode: WorkMode::Code,
        }
    }
}

impl Settings {
    /// Path to `~/.crabot/settings.json`.
    pub fn path() -> PathBuf {
        home::home_dir()
            .unwrap_or_default()
            .join(".crabot")
            .join("settings.json")
    }

    /// Load settings from disk, returning defaults if file is missing or malformed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save settings to disk.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}
