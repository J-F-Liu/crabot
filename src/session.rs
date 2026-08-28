use genai::chat::{ChatMessage, ChatRole, ContentPart};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, TimeZone};

use crate::chat::{Dialog, ToolResult, Turn, TurnBody, is_enveloped_error, strip_error_envelope};
use crate::model::{Currency, ModelConfig, TokenAmount, currency_symbol};
use crate::tools::todo::TodoItem;
use crate::user::WorkMode;

/// One self-contained line in a `{id}.jsonl` session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionRecord {
    /// Header — first line; session-list scan reads only this.
    Meta {
        id: String,
        parent: String,
        model: Option<ModelConfig>,
        title: String,
        workspace: PathBuf,
        created_at: String,
    },
    /// One history message, appended incrementally.
    Message { message: ChatMessage },
    /// Cumulative usage snapshot — last record wins on load.
    Tally {
        tokens: TokenAmount,
        cost: f64,
        currency: Currency,
        requests: u32,
        updated_at: String,
    },
}

/// A conversation session, persisted to `.agent/sessions/` as `{id}.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub id: String,
    /// Parent session id — set by `renew`/`task` spawns, empty for user tabs.
    pub parent: String,
    pub title: String,
    pub model: Option<ModelConfig>,
    pub workspace: PathBuf,
    /// Raw genai messages — exact history sent to / received from the LLM.
    pub history: Vec<ChatMessage>,
    /// Dialogs — each dialog groups one user prompt with its responses.
    #[serde(skip)]
    pub dialogs: Vec<Dialog>,
    /// History messages already written to the jsonl file.
    /// `#[serde(skip)]` — reset to 0 on fork/compact so the next save writes everything.
    #[serde(skip)]
    pub persisted: usize,
    /// Serialized snapshot of the last-written `Meta` record.
    #[serde(skip)]
    pub saved_meta: Option<String>,
    /// Number of successful LLM requests in the session.
    pub requests: u32,
    /// Accumulated token usage across all turns.
    pub tokens: TokenAmount,
    /// Accumulated token cost.
    pub cost: f64,
    /// Currency for the accumulated cost (ISO 4217 code, e.g. "USD", "CNY").
    pub currency: Currency,
    /// Files modified during this session (write / edit tools).
    /// Derived from history on load; not serialised directly.
    #[serde(skip)]
    pub modified_files: Vec<String>,
    /// Files read during this session (read tool).
    /// Derived from history on load; not serialised directly.
    #[serde(skip)]
    pub accessed_files: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Global session-ID generator state — Unix seconds (UTC epoch).
static LAST_ID_TIME: Mutex<i64> = Mutex::new(0);

/// Generate a unique session ID.  `now` is shared with `created_at`.
fn generate_session_id(now: chrono::DateTime<chrono::Local>) -> String {
    let mut last = LAST_ID_TIME.lock().unwrap();
    let candidate = now.timestamp();

    if candidate > *last {
        *last = candidate;
    } else {
        *last += 1;
    }

    let dt = chrono::Local
        .timestamp_opt(*last, 0)
        .single()
        .expect("timestamp must be in range");
    dt.naive_local().format("%Y%m%d-%H%M%S").to_string()
}

/// Add empty reasoning parts to assistant tool-calls missing them, so
/// deepseek-style APIs accept the history. Operates in place.
pub fn fix_history(history: &mut [ChatMessage]) {
    for message in history {
        if message.role == ChatRole::Assistant
            && message.content.contains_tool_call()
            && !message.content.contains_reasoning_content()
        {
            message
                .content
                .push(ContentPart::ReasoningContent(String::new()));
        }
    }
}

impl Session {
    /// Create a new session.
    pub fn new() -> Self {
        let now = chrono::Local::now();
        let id = generate_session_id(now);
        let time = now.format("%Y-%m-%d %H:%M:%S").to_string();
        Session {
            id,
            parent: String::new(),
            title: String::new(),
            model: None,
            workspace: PathBuf::new(),
            history: Vec::new(),
            dialogs: Vec::new(),
            persisted: 0,
            saved_meta: None,
            requests: 0,
            tokens: TokenAmount::default(),
            cost: 0.0,
            currency: Currency::new(),
            modified_files: Vec::new(),
            accessed_files: Vec::new(),
            created_at: time,
            updated_at: String::new(),
        }
    }

    /// Clone with a fresh id/timestamps — the copy gets its own file and
    /// must rewrite everything on the next save.
    fn fresh_copy(&self) -> Self {
        let mut session = self.clone();
        let now = chrono::Local::now();
        session.id = generate_session_id(now);
        session.created_at = now.format("%Y-%m-%d %H:%M:%S").to_string();
        session.updated_at = String::new();
        session.cost = 0.0;
        session.persisted = 0;
        session.saved_meta = None;
        session
    }

    /// Fork: fresh copy with its own usage accounting. System-prompt records
    /// are dropped — the fork starts its own audit trail.
    pub fn fork(&self) -> Self {
        let mut session = self.fresh_copy();
        // Keep only cumulative prompt/output counts.
        session.tokens = TokenAmount {
            prompt: session.tokens.prompt,
            output: session.tokens.output,
            ..Default::default()
        };
        // Save the forked session; failures are logged inside `save`.
        if session.workspace.is_dir() {
            let _ = session.save();
        }
        session
    }

    /// Compact: fresh copy where each dialog keeps only its user prompt and
    /// final text answer; tool activity, partial replies and system-prompt
    /// records are dropped.
    pub fn compact(&self) -> Self {
        let mut session = self.fresh_copy();
        // Keep the last text-only assistant reply per dialog; flush it before
        // the next user prompt and at the end.
        let mut answer: Option<ChatMessage> = None;
        let mut compacted = Vec::with_capacity(session.dialogs.len() * 2);
        for msg in &session.history {
            match msg.role {
                ChatRole::User => {
                    compacted.extend(answer.take());
                    compacted.push(msg.clone());
                }
                ChatRole::Assistant
                    if !msg.content.contains_tool_call()
                        && msg.content.joined_texts().is_some_and(|t| !t.is_empty()) =>
                {
                    answer = Some(msg.clone());
                }
                _ => {}
            }
        }
        compacted.extend(answer.take());
        session.history = compacted;
        session.rebuild_dialogs();
        // Fresh accumulators
        session.tokens = TokenAmount::default();
        session.requests = 0;
        // Save the compacted session; failures are logged inside `save_with_tally`.
        if session.workspace.is_dir() {
            let _ = session.save_with_tally();
        }
        session
    }

    // ── Dialog / turn helpers ────────────────────────────────────────

    /// Add a new empty dialog with the given title and optional work mode.
    pub fn add_dialog(&mut self, title: String, mode: Option<WorkMode>) {
        if self.title.is_empty() {
            self.title = title.clone();
        }
        self.dialogs.push(Dialog {
            title,
            turns: Vec::new(),
            mode,
        });
    }

    /// Push a turn.  A `User` turn starts a new dialog; all other roles
    /// append to the last dialog (creating one if none exists yet).
    pub fn push_turn(&mut self, mut turn: Turn) {
        let now = chrono::Local::now();
        turn.timestamp = now.format("%H:%M:%S").to_string();
        if let Some(last) = self.dialogs.last_mut() {
            last.turns.push(turn);
        } else {
            self.dialogs.push(Dialog {
                title: String::new(),
                turns: vec![turn],
                mode: None,
            });
        }
    }

    /// Mutable reference to the last turn across all dialogs.
    pub fn last_turn_mut(&mut self) -> Option<&mut Turn> {
        self.dialogs.last_mut().and_then(|d| d.turns.last_mut())
    }

    /// Remove and return the last turn, dropping any dialog it empties.
    pub fn pop_last_turn(&mut self) -> Option<Turn> {
        while let Some(dialog) = self.dialogs.last_mut() {
            if let Some(turn) = dialog.turns.pop() {
                return Some(turn);
            }
            self.dialogs.pop();
        }
        None
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
    pub fn accumulate_tokens(&mut self, tokens: &TokenAmount, cost: Option<crate::model::Cost>) {
        self.requests += 1;
        self.tokens.accumulate(tokens);
        if let Some(c) = cost {
            self.cost += c.calculate(tokens);
            if self.currency != c.currency {
                self.currency = c.currency;
            }
        }
    }

    /// Time part of `updated_at` ("%H:%M:%S"), for compact display.
    pub fn updated_at_time(&self) -> String {
        self.updated_at
            .split_whitespace()
            .nth(1)
            .unwrap_or(&self.updated_at)
            .to_string()
    }

    /// Record the time of the last received assistant response.
    pub fn stamp_response(&mut self) {
        self.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    }

    /// Format session cost as a string with the currency symbol prefix.
    /// Small amounts get 4 decimal places, larger amounts get 2 decimal places.
    pub fn formatted_cost(&self) -> String {
        let sym = currency_symbol(&self.currency);
        if self.cost > 0.0 && self.cost < 0.01 {
            format!("{sym}{:.4}", self.cost)
        } else {
            format!("{sym}{:.2}", self.cost)
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

    /// First user message, skipping system-prompt audit records.
    pub fn first_user_message(&self) -> Option<&ChatMessage> {
        self.history.iter().find(|m| m.role == ChatRole::User)
    }

    /// Whether the conversation contains an assistant reply.
    pub fn has_reply(&self) -> bool {
        self.history.iter().any(|m| m.role == ChatRole::Assistant)
    }

    /// Case-insensitive search across dialog headers and turn headers/content.
    /// `flat_idx` matches `center_pane` numbering; dialog-header hits land on
    /// the dialog's first turn so navigation can scroll to them.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let q = query.to_lowercase();
        let matches = |s: &str| s.to_lowercase().contains(&q);
        let mut results = Vec::new();
        let mut flat_idx: usize = 0;
        for (di, dialog) in self.dialogs.iter().enumerate() {
            let dialog_start = flat_idx;
            // Dialog header: displayed title + work-mode badge.
            let header_hit = matches(&dialog.display_title(di))
                || dialog.mode.as_ref().is_some_and(|m| matches(&m.name));
            let mut dialog_hits = Vec::new();
            for turn in &dialog.turns {
                // Plain role labels ("User"/"Assistant") are uniform across
                // turn kinds, hence not searchable; tool badges embed the tool
                // name ("Tool - read") and are searchable. Timestamps, tool
                // names/args and message content are searchable too.
                let hit = match &turn.body {
                    TurnBody::Text(tc) => {
                        matches(&turn.timestamp)
                            || matches(&tc.content)
                            || tc.reasoning.as_deref().is_some_and(matches)
                    }
                    TurnBody::Tool(trs) => trs.iter().any(|tr| {
                        matches(&format!("Tool - {}", tr.name))
                            || matches(&tr.timestamp)
                            || matches(&tr.args.to_string())
                            || match &tr.result {
                                Ok(s) => matches(s),
                                Err(e) => matches(e),
                            }
                    }),
                    TurnBody::Temp(calls) => {
                        matches(&turn.timestamp)
                            || calls.iter().any(|c| {
                                matches(&format!("Tool - {}", c.name))
                                    || matches(&c.args.to_string())
                            })
                    }
                };
                if hit {
                    dialog_hits.push(SearchHit {
                        flat_idx,
                        kind: SearchHitKind::Turn,
                    });
                }
                flat_idx += 1;
            }
            // Header hits jump to the dialog's first turn; skip when that turn
            // is itself a hit so Next/Prev never lands on the same spot twice.
            let first_turn_hit = dialog_hits
                .first()
                .is_some_and(|h| h.flat_idx == dialog_start);
            if header_hit && !dialog.turns.is_empty() && !first_turn_hit {
                results.push(SearchHit {
                    flat_idx: dialog_start,
                    kind: SearchHitKind::DialogHeader,
                });
            }
            results.extend(dialog_hits);
        }
        results
    }

    /// Reconstruct `dialogs` from raw `history` (called after load, since
    /// `dialogs` is `#[serde(skip)]`).
    pub fn rebuild_dialogs(&mut self) {
        // Index tool responses by call_id to pair with their tool calls.
        let mut results: HashMap<String, String> = HashMap::new();
        for msg in &self.history {
            if msg.role == ChatRole::Tool {
                for tr in msg.content.tool_responses() {
                    results.insert(tr.call_id.clone(), tr.content.clone());
                }
            }
        }

        let mut accessed: Vec<String> = Vec::new();
        let mut modified: Vec<String> = Vec::new();

        let mut dialogs: Vec<Dialog> = Vec::new();

        /// Append `turn` to the last dialog, or start a new one if none exists.
        fn push_or_new(dialogs: &mut Vec<Dialog>, turn: Turn) {
            match dialogs.last_mut() {
                Some(d) => d.turns.push(turn),
                None => dialogs.push(Dialog {
                    title: String::new(),
                    turns: vec![turn],
                    mode: None,
                }),
            }
        }

        for msg in &self.history {
            match msg.role {
                ChatRole::System => {}
                ChatRole::User => {
                    let parts = msg.content.parts();
                    // Extract work mode from the first part if present (e.g. "work-mode: code").
                    let mode = parts
                        .first()
                        .and_then(|p| p.as_text())
                        .and_then(|t| t.strip_prefix("work-mode: "))
                        .filter(|s| !s.is_empty())
                        .map(WorkMode::from);
                    let text = parts.last().and_then(|p| p.as_text()).unwrap_or_default();
                    let title = Self::derive_title(text);
                    let turn = Turn::user(text);
                    dialogs.push(Dialog {
                        title,
                        turns: vec![turn],
                        mode,
                    });
                }
                ChatRole::Assistant => {
                    let text = msg.content.joined_texts().unwrap_or_default();
                    let reasoning = msg.content.first_reasoning_content().map(|s| s.to_string());

                    if !text.is_empty() || reasoning.is_some() {
                        push_or_new(&mut dialogs, Turn::assistant(text, reasoning));
                    }

                    // Group all tool calls from this assistant message into one Turn.
                    let mut trs: Vec<ToolResult> = Vec::new();
                    for tc in msg.content.tool_calls() {
                        let content = results.remove(&tc.call_id).unwrap_or_default();
                        // Strip the "Error: " envelope to match live-stream display.
                        let result = if is_enveloped_error(&content) {
                            Err(strip_error_envelope(&content).to_string())
                        } else {
                            Ok(content)
                        };
                        let tr = ToolResult {
                            name: tc.fn_name.clone(),
                            call_id: Some(tc.call_id.clone()),
                            args: tc.fn_arguments.clone(),
                            result,
                            timestamp: String::new(),
                            streaming: false,
                        };
                        // Track files touched by write / edit / read tools.
                        tr.track_modified_file(&mut modified);
                        tr.track_read_file(&mut accessed);
                        trs.push(tr);
                    }
                    if !trs.is_empty() {
                        let turn = Turn::from_tool_results(trs);
                        push_or_new(&mut dialogs, turn);
                    }
                }
                ChatRole::Tool => {
                    // Tool responses already paired with calls above; skip.
                }
            }
        }

        self.modified_files = modified;
        self.accessed_files = accessed;
        self.dialogs = dialogs;
    }

    /// Return the items of the last successful `todo` tool call.
    pub fn last_todo_items(&self) -> Vec<TodoItem> {
        // Reverse pass: collect successful todo call-ids from Tool messages,
        // then match against the preceding Assistant's todo tool call.
        static SUCCESS_RE: OnceLock<Regex> = OnceLock::new();
        let success_re = SUCCESS_RE.get_or_init(|| Regex::new(r"^Updated \d+ todo").unwrap());
        let mut successful: HashSet<&str> = HashSet::new();
        for msg in self.history.iter().rev() {
            match msg.role {
                ChatRole::Tool => {
                    for tr in msg.content.tool_responses() {
                        if success_re.is_match(&tr.content) {
                            successful.insert(tr.call_id.as_str());
                        }
                    }
                }
                ChatRole::Assistant => {
                    for tc in msg.content.tool_calls().iter().rev() {
                        if tc.fn_name == "todo" && successful.contains(tc.call_id.as_str()) {
                            return tc
                                .fn_arguments
                                .get("items")
                                .and_then(|v| {
                                    serde_json::from_value::<Vec<TodoItem>>(v.clone()).ok()
                                })
                                .unwrap_or_default();
                        }
                    }
                }
                _ => {}
            }
        }
        Vec::new()
    }

    // ── Persistence ─────────────────────────────────────────────────

    /// Compute the save path for this session.
    /// Sessions are stored under `.agent/sessions/{YYYY-MM}/{id}.jsonl`.
    pub fn save_path(&self) -> Option<PathBuf> {
        if !self.workspace.is_dir() {
            return None;
        }
        let base = self.workspace.join(".agent").join("sessions");
        let year_month = year_month_from_id(&self.id);
        Some(base.join(&year_month).join(format!("{}.jsonl", self.id)))
    }

    /// Append new `Message` lines; re-append `Meta` whenever it changed (last one wins on load).
    /// No `Tally` — use [`Session::save_with_tally`] on terminal stream events.
    pub fn save(&mut self) -> Result<(), String> {
        // All callers discard the result, so failures are logged here once.
        self.save_inner(false).inspect_err(|e| {
            tracing::warn!(session = %self.id, "failed to save session: {e}");
        })
    }

    /// Like [`Session::save`], plus a [`SessionRecord::Tally`] line so counters
    /// persist on terminal stream events even without new history.
    pub fn save_with_tally(&mut self) -> Result<(), String> {
        self.save_inner(true).inspect_err(|e| {
            tracing::warn!(session = %self.id, "failed to save session: {e}");
        })
    }

    fn save_inner(&mut self, with_tally: bool) -> Result<(), String> {
        let path = self.save_path().ok_or("No workspace set")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create session dir: {e}"))?;
        }

        let is_new = !path.exists();
        let start = if is_new {
            0
        } else {
            self.persisted.min(self.history.len())
        };
        let new_messages = self.history.len() > start;

        // Build a Meta line when writing a new file or when the meta snapshot
        // has changed since the last write.
        let meta_json = serde_json::to_string(&self.meta_record())
            .map_err(|e| format!("Failed to serialize meta: {e}"))?;
        let write_meta = is_new || self.saved_meta.as_deref() != Some(&meta_json);

        let mut buf = String::new();
        if write_meta {
            buf.push_str(&meta_json);
            buf.push('\n');
        }
        Self::push_messages(&mut buf, &self.history, start)?;
        // Tally only on terminal stream events — avoids a redundant line per save.
        if with_tally {
            self.push_tally(&mut buf)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("Failed to open session file: {e}"))?;
        file.write_all(buf.as_bytes())
            .map_err(|e| format!("Failed to write session: {e}"))?;

        self.persisted = self.history.len();
        self.saved_meta = Some(meta_json);

        // Remove a stale legacy `.json` sibling — the jsonl now holds everything.
        if is_new {
            let legacy = path.with_extension("json");
            if legacy.exists() {
                let _ = std::fs::remove_file(&legacy);
            }
        }
        tracing::debug!(
            session = %self.id,
            path = %path.display(),
            new_messages = if new_messages { self.history.len() - start } else { 0 },
            "session saved"
        );
        Ok(())
    }

    /// Load a session from disk. Prefers the `.jsonl` sibling when given a
    /// legacy `.json` path.
    pub fn load(path: &Path) -> Result<Self, String> {
        tracing::debug!(path = %path.display(), "loading session");
        let path = match path.extension().and_then(|e| e.to_str()) {
            Some("json") if path.with_extension("jsonl").exists() => path.with_extension("jsonl"),
            _ => path.to_path_buf(),
        };
        match path.extension().and_then(|e| e.to_str()) {
            Some("jsonl") => Self::load_jsonl(&path),
            _ => Self::load_legacy_json(&path),
        }
    }

    /// Load from a jsonl file — merge all records line-by-line.
    fn load_jsonl(path: &Path) -> Result<Self, String> {
        let file =
            std::fs::File::open(path).map_err(|e| format!("Failed to open session file: {e}"))?;
        let reader = BufReader::new(file);

        let mut session = Session::new();
        let mut saw_meta = false;

        for (line_no, line) in reader.lines().enumerate() {
            let line = line
                .map_err(|e| format!("Failed to read session file at line {}: {e}", line_no + 1))?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionRecord>(&line) {
                Ok(SessionRecord::Meta {
                    id,
                    parent,
                    title,
                    model,
                    workspace,
                    created_at,
                }) => {
                    saw_meta = true;
                    session.id = id;
                    session.parent = parent;
                    session.title = title;
                    session.model = model;
                    session.workspace = workspace;
                    session.created_at = created_at;
                }
                Ok(SessionRecord::Message { message }) => session.history.push(message),
                Ok(SessionRecord::Tally {
                    requests,
                    tokens,
                    cost,
                    currency,
                    updated_at,
                }) => {
                    session.requests = requests;
                    session.tokens = tokens;
                    session.cost = cost;
                    session.currency = currency;
                    session.updated_at = updated_at;
                }
                Err(e) => tracing::warn!(
                    "Skipping unparseable line {} in {}: {e}",
                    line_no + 1,
                    path.display()
                ),
            }
        }

        // Fall back to the file stem when the Meta header is missing/corrupt.
        if !saw_meta {
            session.id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
        }

        session.persisted = session.history.len();
        session.saved_meta = serde_json::to_string(&session.meta_record()).ok();
        session.rebuild_dialogs();
        Ok(session)
    }

    /// Load from a legacy whole-document `.json` file.
    fn load_legacy_json(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read session file: {e}"))?;
        let mut session: Self =
            serde_json::from_str(&json).map_err(|e| format!("Failed to parse session: {e}"))?;
        // `persisted` / `saved_meta` are `#[serde(skip)]` → next save writes a full jsonl.
        session.rebuild_dialogs();
        Ok(session)
    }

    /// Audit record of the system prompt, deduped against the last record.
    pub fn record_system_prompt(&mut self, prompt: &str) -> bool {
        let changed = !prompt.trim().is_empty()
            && self
                .history
                .iter()
                .rev()
                .find(|m| m.role == ChatRole::System)
                .and_then(|m| m.content.joined_texts())
                .as_deref()
                != Some(prompt);
        if changed {
            self.history.push(ChatMessage::system(prompt));
        }
        changed
    }

    /// Append `history[start..]` as one `Message` record line per message.
    fn push_messages(
        buf: &mut String,
        history: &[ChatMessage],
        start: usize,
    ) -> Result<(), String> {
        for msg in &history[start..] {
            let line = serde_json::to_string(&SessionRecord::Message {
                message: msg.clone(),
            })
            .map_err(|e| format!("Failed to serialize message: {e}"))?;
            buf.push_str(&line);
            buf.push('\n');
        }
        Ok(())
    }

    /// Append the cumulative usage snapshot as one `Tally` record line.
    fn push_tally(&self, buf: &mut String) -> Result<(), String> {
        let line = serde_json::to_string(&SessionRecord::Tally {
            requests: self.requests,
            tokens: self.tokens,
            cost: self.cost,
            currency: self.currency,
            updated_at: self.updated_at.clone(),
        })
        .map_err(|e| format!("Failed to serialize tally: {e}"))?;
        buf.push_str(&line);
        buf.push('\n');
        Ok(())
    }

    /// Build the current `Meta` record from this session's fields.
    fn meta_record(&self) -> SessionRecord {
        SessionRecord::Meta {
            id: self.id.clone(),
            parent: self.parent.clone(),
            model: self.model.clone(),
            title: self.title.clone(),
            workspace: self.workspace.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

/// Where a [`SearchHit`] matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchHitKind {
    /// Dialog header text; jumps to the dialog's first turn. Not emitted when
    /// that turn is also a hit, since both would share the same jump target.
    DialogHeader,
    /// Turn header or content.
    Turn,
}

/// One match from [`Session::search`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchHit {
    /// Flat turn index to jump to, matching `center_pane` numbering.
    pub flat_idx: usize,
    /// What part of the conversation matched.
    pub kind: SearchHitKind,
}

/// Extract YYYY-MM from a session id (format: YYYYMMDD-HHMMSS).
pub fn year_month_from_id(id: &str) -> String {
    if id.len() >= 6 {
        format!("{}-{}", &id[..4], &id[4..6])
    } else {
        chrono::Local::now().format("%Y-%m").to_string()
    }
}

/// List saved session file paths for a workspace (newest first). When `month`
/// is given (`YYYY-MM`), only that year-month subdirectory is scanned;
/// otherwise the last 3 months are scanned. The legacy flat base directory is
/// always scanned. Prefers `.jsonl` over `.json` for the same id.
pub fn list_session_paths(workspace: &Path, month: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let base = workspace.join(".agent").join("sessions");
    if !base.exists() {
        return Ok(Vec::new());
    }

    // Year-month subdirectories to scan (None = last 3 months).
    let year_months: Vec<String> = match month {
        Some(month) => vec![month.to_string()],
        None => {
            let now = chrono::Local::now();
            let (year, current_month) = (now.year(), now.month());
            (0..3i32)
                .map(|i| {
                    let mut m = current_month as i32 - i;
                    let mut y = year;
                    while m <= 0 {
                        m += 12;
                        y -= 1;
                    }
                    format!("{:04}-{:02}", y, m)
                })
                .collect()
        }
    };

    // Collect paths, preferring .jsonl over .json for the same session id.
    let mut seen: HashMap<String, PathBuf> = HashMap::new();

    fn collect_into(dir: &Path, seen: &mut HashMap<String, PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "jsonl" | "json") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            // Prefer .jsonl over .json for the same stem.
            let replacing_jsonl_with_json = ext == "json"
                && seen
                    .get(stem)
                    .is_some_and(|p| p.extension().is_some_and(|e| e == "jsonl"));
            if !replacing_jsonl_with_json {
                seen.insert(stem.to_string(), path);
            }
        }
    }

    for ym in &year_months {
        let dir = base.join(ym);
        if dir.is_dir() {
            collect_into(&dir, &mut seen);
        }
    }
    // Also scan the base directory for legacy flat session files.
    collect_into(&base, &mut seen);

    let mut paths: Vec<PathBuf> = seen.into_values().collect();
    paths.sort_by(|a, b| b.file_stem().cmp(&a.file_stem())); // newest first by id
    Ok(paths)
}
