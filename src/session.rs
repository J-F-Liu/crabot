use genai::chat::{ChatMessage, ChatRole};
use json_escape::unescape;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::chat::{Dialog, ToolResult, Turn};
use crate::model::{ModelConfig, TokenAmount};

// ── SessionEntry ────────────────────────────────────────────────────

/// Lightweight session metadata for dropdown listing.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
}

impl std::fmt::Display for SessionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.title.is_empty() {
            write!(f, "{}", self.id)
        } else {
            write!(f, "{} — {}", self.id, self.title)
        }
    }
}

impl PartialEq for SessionEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

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
    /// Size of the last prompt in tokens.
    #[serde(default)]
    pub size: i32,
    /// Files modified during this session (write / edit tools).
    /// Derived from history on load; not serialised directly.
    #[serde(skip, default)]
    pub modified_files: Vec<String>,
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
            size: 0,
            modified_files: Vec::new(),
            created_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
            updated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }

    // ── Dialog / turn helpers ────────────────────────────────────────

    /// Add a new empty dialog with the given title.
    pub fn add_dialog(&mut self, title: String) {
        if self.title.is_empty() {
            self.title = title.clone();
        }
        self.dialogs.push(Dialog {
            title,
            turns: Vec::new(),
        });
    }

    /// Push a turn.  A `User` turn starts a new dialog; all other roles
    /// append to the last dialog (creating one if none exists yet).
    pub fn push_turn(&mut self, mut turn: Turn) {
        let now = chrono::Local::now();
        turn.timestamp = now.format("%H:%M:%S").to_string();
        self.updated_at = now.format("%Y-%m-%d %H:%M:%S").to_string();
        if let Some(last) = self.dialogs.last_mut() {
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

    /// Derive a short title from text content.
    pub fn derive_title(text: &str) -> String {
        let trimmed = text.trim();
        // Take up to the first newline, or first 144 chars.
        let first_line = trimmed.lines().next().unwrap_or("");
        if let Some((idx, _)) = first_line.char_indices().nth(144) {
            format!("{}…", &first_line[..idx])
        } else {
            first_line.to_string()
        }
    }

    /// Whether the session has any dialogs.
    pub fn is_empty(&self) -> bool {
        self.dialogs.is_empty()
    }

    /// Reconstruct the `dialogs` Vec from the raw `history`.
    /// Called after loading a session from disk (since `dialogs` is `#[serde(skip)]`).
    pub fn rebuild_dialogs(&mut self) {
        // First pass: collect tool responses indexed by call_id so we can
        // pair them with their tool calls (matching the live-stream behaviour
        // in llm.rs where each tool call+result is a single Turn).
        let mut results: HashMap<String, String> = HashMap::new();
        for msg in &self.history {
            if msg.role == ChatRole::Tool {
                for tr in msg.content.tool_responses() {
                    results.insert(tr.call_id.clone(), tr.content.clone());
                }
            }
        }

        let mut dialogs: Vec<Dialog> = Vec::new();

        /// Append `turn` to the last dialog, or start a new one if none exists.
        fn push_or_new(dialogs: &mut Vec<Dialog>, turn: Turn) {
            match dialogs.last_mut() {
                Some(d) => d.turns.push(turn),
                None => dialogs.push(Dialog {
                    title: String::new(),
                    turns: vec![turn],
                }),
            }
        }

        let mut modified: Vec<String> = Vec::new();

        for msg in &self.history {
            match msg.role {
                ChatRole::System => {}
                ChatRole::User => {
                    let text = msg.content.joined_texts().unwrap_or_default();
                    // Strip the leading <work-mode>…</work-mode>\n tag so the
                    // title reflects the actual user message, not the mode annotation.
                    let text_for_title = text
                        .find("</work-mode>\n")
                        .map(|idx| &text[idx + "</work-mode>\n".len()..])
                        .unwrap_or(&text);
                    let title = Self::derive_title(text_for_title);
                    let turn = Turn::user(text);
                    dialogs.push(Dialog {
                        title,
                        turns: vec![turn],
                    });
                }
                ChatRole::Assistant => {
                    let text = msg.content.joined_texts().unwrap_or_default();
                    let reasoning = msg.content.first_reasoning_content().map(|s| s.to_string());

                    if !text.is_empty() || reasoning.is_some() {
                        push_or_new(&mut dialogs, Turn::assistant(text, reasoning));
                    }

                    for tc in msg.content.tool_calls() {
                        let result = results.remove(&tc.call_id).unwrap_or_default();
                        let tr = ToolResult {
                            name: tc.fn_name.clone(),
                            call_id: Some(tc.call_id.clone()),
                            args: tc.fn_arguments.clone(),
                            result: Ok(result),
                        };
                        // Track files modified by write / edit tools.
                        if let Some(path_str) = tr.get_modified_file()
                            && !modified.iter().any(|p| p == path_str)
                        {
                            modified.push(path_str.to_string());
                        }
                        let turn = Turn::from_tool_result(tr);
                        push_or_new(&mut dialogs, turn);
                    }
                }
                ChatRole::Tool => {
                    // Tool responses already paired with calls above; skip.
                }
            }
        }

        // todo: if results is not empty, log warning

        self.modified_files = modified;
        self.dialogs = dialogs;
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
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read session file: {e}"))?;
        let mut session: Self =
            serde_json::from_str(&json).map_err(|e| format!("Failed to parse session: {e}"))?;
        session.rebuild_dialogs();
        Ok(session)
    }

    /// List session metadata for a workspace (reads only first 8 KiB per file).
    pub fn list_entries(workspace: &Path) -> Result<Vec<SessionEntry>, String> {
        let paths = Self::list(workspace)?;
        let mut entries = Vec::with_capacity(paths.len());
        let mut buf = vec![0u8; 8192];
        for path in paths {
            let (id, title) = match std::fs::File::open(&path) {
                Ok(mut file) => match file.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        (
                            extract_json_string(&text, "id").unwrap_or_default(),
                            extract_json_string(&text, "title").unwrap_or_default(),
                        )
                    }
                    _ => (String::new(), String::new()),
                },
                Err(_) => (String::new(), String::new()),
            };
            entries.push(SessionEntry { id, title, path });
        }
        Ok(entries)
    }

    /// List all saved sessions for a workspace.
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

/// Extract a top-level JSON string value for `key` from partial JSON text.
/// Unescaping (incl. `\uXXXX` surrogate pairs) is handled by `json_escape`;
/// truncated input yields the portion decoded so far.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let search = format!("\"{}\"", key);
    let rest = json.split_once(&search)?.1;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    // Isolate the quoted string: `unescape` won't stop at a closing quote,
    // so scan to the first unescaped `"` ourselves.
    let content = rest.strip_prefix('"')?;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => break,
            _ => i += 1,
        }
    }
    let inner = &content[..i.min(content.len())];
    Some(unescape(inner).display_utf8_lossy().to_string())
}
