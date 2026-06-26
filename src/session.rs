use genai::chat::{ChatMessage, ChatRole};
use iced::{
    Alignment, Element, Font, Length, font,
    widget::{button, row, text},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::Message;
use crate::chat::{Dialog, ToolResult, Turn, TurnBody};
use crate::llm::StreamState;
use crate::model::{ModelConfig, TokenAmount};

// ── Session ──────────────────────────────────────────────────────────

/// A conversation session, persisted to `.agent/sessions/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub title: String,
    pub model: Option<ModelConfig>,
    pub workspace: PathBuf,
    /// Dialogs — each dialog groups one user prompt with its responses.
    #[serde(skip)]
    pub dialogs: Vec<Dialog>,
    /// Raw genai messages — exact history sent to / received from the LLM.
    pub history: Vec<ChatMessage>,
    /// Accumulated token usage across all turns.
    #[serde(default)]
    pub usage: TokenAmount,
    /// Accumulated cost in USD.
    #[serde(default)]
    pub cost: f64,
    pub created_at: String,
    pub updated_at: String,
}

impl Session {
    /// Create a new session.
    pub fn new(model: Option<ModelConfig>, workspace: PathBuf) -> Self {
        let now = chrono::Local::now();
        let id = now.format("%Y%m%d-%H%M%S").to_string();
        Session {
            id,
            title: String::new(),
            model,
            workspace,
            history: Vec::new(),
            dialogs: Vec::new(),
            usage: TokenAmount::default(),
            cost: 0.0,
            created_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
            updated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }

    // ── Dialog / turn helpers ────────────────────────────────────────

    /// Push a turn.  A `User` turn starts a new dialog; all other roles
    /// append to the last dialog (creating one if none exists yet).
    pub fn push_turn(&mut self, turn: Turn) {
        self.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        if turn.role == ChatRole::User {
            let title = Self::derive_title(&turn);
            if self.title.is_empty() {
                self.title = title.clone();
            }
            self.dialogs.push(Dialog {
                title,
                turns: vec![turn],
            });
        } else if let Some(last) = self.dialogs.last_mut() {
            last.turns.push(turn);
        } else {
            self.dialogs.push(Dialog {
                title: String::new(),
                turns: vec![turn],
            });
        }
    }

    /// Reference to the last turn across all dialogs.
    pub fn last_turn(&self) -> Option<&Turn> {
        self.dialogs.last().and_then(|d| d.turns.last())
    }

    /// Mutable reference to the last turn across all dialogs.
    pub fn last_turn_mut(&mut self) -> Option<&mut Turn> {
        self.dialogs.last_mut().and_then(|d| d.turns.last_mut())
    }

    /// Total number of turns across all dialogs.
    pub fn total_turns(&self) -> usize {
        self.dialogs.iter().map(|d| d.turns.len()).sum()
    }

    /// Iterate mutably over turns, skipping the first `skip` turns.
    pub fn turns_from_mut(&mut self, skip: usize) -> impl Iterator<Item = &mut Turn> {
        self.dialogs
            .iter_mut()
            .flat_map(|d| d.turns.iter_mut())
            .skip(skip)
    }

    /// Accumulate token usage and recalculate cost from the model's pricing.
    pub fn accumulate_usage(&mut self, tokens: &TokenAmount, cost: Option<&crate::model::Cost>) {
        self.usage.accumulate(tokens);
        if let Some(c) = cost {
            self.cost += c.calculate(tokens);
        }
    }

    /// Derive a short title from a user turn's text.
    fn derive_title(turn: &Turn) -> String {
        if let TurnBody::Text(tc) = &turn.body {
            let trimmed = tc.content.trim();
            // Take up to the first newline, or first 72 chars.
            let first_line = trimmed.lines().next().unwrap_or("");
            if let Some((idx, _)) = first_line.char_indices().nth(72) {
                format!("{}…", &first_line[..idx])
            } else {
                first_line.to_string()
            }
        } else {
            String::new()
        }
    }

    /// Whether the session has any dialogs.
    pub fn is_empty(&self) -> bool {
        self.dialogs.is_empty()
    }

    /// Reference to all dialogs (for UI display).
    pub fn dialogs_ref(&self) -> &[Dialog] {
        &self.dialogs
    }

    /// Reconstruct the `dialogs` Vec from the raw `history`.
    /// Called after loading a session from disk (since `dialogs` is `#[serde(skip)]`).
    pub fn rebuild_dialogs(&mut self) {
        self.dialogs.clear();

        // First pass: collect tool responses indexed by call_id so we can
        // pair them with their tool calls (matching the live-stream behaviour
        // in llm.rs where each tool call+result is a single Turn).
        let mut response_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for msg in &self.history {
            if msg.role == ChatRole::Tool {
                for tr in msg.content.tool_responses() {
                    response_map.insert(tr.call_id.clone(), tr.content.clone());
                }
            }
        }

        enum Draft {
            User(String),
            Assistant(String, Option<String>),
            Tool(ToolResult),
        }
        let mut drafts: Vec<Draft> = Vec::new();
        for msg in &self.history {
            match msg.role {
                ChatRole::System => {}
                ChatRole::User => {
                    let text = msg.content.joined_texts().unwrap_or_default();
                    drafts.push(Draft::User(text));
                }
                ChatRole::Assistant => {
                    let text = msg.content.joined_texts().unwrap_or_default();
                    let reasoning = msg.content.first_reasoning_content().map(|s| s.to_string());

                    if !text.is_empty() || reasoning.is_some() {
                        drafts.push(Draft::Assistant(text, reasoning));
                    }

                    for tc in msg.content.tool_calls() {
                        // Pair tool call with its response; unmatched calls
                        // (shouldn't happen) still get a Turn with an empty result.
                        let result = response_map
                            .remove(&tc.call_id)
                            .map(Ok)
                            .unwrap_or_else(|| Ok(String::new()));
                        drafts.push(Draft::Tool(ToolResult {
                            name: tc.fn_name.clone(),
                            call_id: Some(tc.call_id.clone()),
                            args: tc.fn_arguments.clone(),
                            result,
                        }));
                    }
                }
                ChatRole::Tool => {
                    // Tool responses already paired with calls above; skip.
                }
            }
        }

        // Any unmatched tool responses (shouldn't happen, but be defensive).
        for (call_id, content) in response_map {
            drafts.push(Draft::Tool(ToolResult {
                name: String::new(),
                call_id: Some(call_id),
                args: serde_json::Value::Null,
                result: Ok(content),
            }));
        }

        for draft in drafts {
            match draft {
                Draft::User(text) => self.push_turn(Turn::user(text)),
                Draft::Assistant(text, reasoning) => {
                    self.push_turn(Turn::assistant(text, reasoning))
                }
                Draft::Tool(tr) => {
                    let mut turn = Turn::from_tool_result(tr);
                    turn.timestamp = String::new();
                    self.push_turn(turn);
                }
            }
        }
    }

    // ── Persistence ─────────────────────────────────────────────────

    /// Compute the save path for this session.
    pub fn save_path(&self) -> Option<PathBuf> {
        if !self.workspace.is_dir() {
            return None;
        }
        let base = self.workspace.join(".agent").join("sessions");
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
        let mut session: Self =
            serde_json::from_str(&json).map_err(|e| format!("Failed to parse session: {e}"))?;
        session.rebuild_dialogs();
        Ok(session)
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

pub fn session_view<'a>(streaming: StreamState) -> Element<'a, Message> {
    row![
        text("Session").size(14).font(Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        }),
        iced::widget::Space::new().width(Length::Fill),
        button(text("New").align_x(Alignment::Center))
            .on_press_maybe(if streaming != StreamState::Idle {
                None
            } else {
                Some(Message::NewSession)
            })
            .style(crate::primary_button),
    ]
    .align_y(Alignment::Center)
    .spacing(8)
    .into()
}
