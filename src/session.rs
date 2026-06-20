use genai::chat::ChatMessage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::chat::DisplayMessage;
use crate::model::ModelConfig;

// ── Session ──────────────────────────────────────────────────────────

/// A conversation session, persisted to `.agent/sessions/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub model: Option<ModelConfig>,
    pub workspace: String,
    /// App-level messages for UI display.
    pub messages: Vec<DisplayMessage>,
    /// Raw genai messages — exact history sent to / received from the LLM.
    /// Used directly in subsequent turns to avoid fragile reconstruction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<ChatMessage>,
    pub created_at: String,
    pub updated_at: String,
}

impl Session {
    /// Create a new session.
    pub fn new(model: Option<ModelConfig>, workspace: Option<&Path>) -> Self {
        let now = chrono::Local::now();
        let id = now.format("%Y%m%d-%H%M%S").to_string();
        Session {
            id,
            model,
            workspace: workspace
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            history: Vec::new(),
            messages: Vec::new(),
            created_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
            updated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }

    /// Push a message and bump the updated_at timestamp.
    pub fn push(&mut self, msg: DisplayMessage) {
        self.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.messages.push(msg);
    }

    /// Push multiple messages.
    pub fn extend(&mut self, msgs: impl IntoIterator<Item = DisplayMessage>) {
        for msg in msgs {
            self.push(msg);
        }
    }

    /// Compute the save path for this session.
    pub fn save_path(&self) -> Option<PathBuf> {
        if self.workspace.is_empty() {
            return None;
        }
        let base = Path::new(&self.workspace).join(".agent").join("sessions");
        Some(base.join(format!("{}.json", self.id)))
    }

    /// Save the session to disk.
    pub fn save(&self) -> Result<(), String> {
        let path = self.save_path().ok_or("No workspace set")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create session dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize session: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("Failed to write session: {e}"))
    }

    /// Load a session from disk.
    #[allow(dead_code)]
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read session file: {e}"))?;
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse session: {e}"))
    }

    /// List all saved sessions for a workspace.
    #[allow(dead_code)]
    pub fn list(workspace: &Path) -> Result<Vec<PathBuf>, String> {
        let dir = workspace.join(".agent").join("sessions");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| format!("Failed to read sessions dir: {e}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort_by(|a, b| b.cmp(a)); // newest first
        Ok(paths)
    }
}
