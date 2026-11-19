use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, TimeZone};

use crate::chat::{
    Dialog, ToolResult, Turn, TurnBody, attachment_marker_items, image_marker_paths,
    is_enveloped_error, strip_error_envelope,
};
use crate::model::{Currency, ModelConfig, TokenAmount, currency_symbol};
use crate::tools::ask::{ask_question, is_ask_failure};
use crate::tools::todo::TodoItem;
use crate::user::{UserPrompt, WorkMode};

/// Prompt injected when the context is nearly full, suggesting `renew`.
pub const RENEW_HINT_PROMPT: &str = "Context fill ratio is near its limit, consider calling the renew tool to continue current task.";

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
    /// UI-only notice turn (e.g. a truncated reply); never part of the history.
    Notice { at: usize, text: String },
    /// Cumulative usage snapshot — last record wins on load.
    Tally {
        tokens: TokenAmount,
        cost: f64,
        currency: Currency,
        requests: u32,
        updated_at: String,
    },
}

/// A UI-only notice turn, anchored to the history length when it appeared.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoticeTurn {
    /// Number of history messages already recorded when the notice was emitted.
    pub at: usize,
    pub text: String,
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
    /// Notice turns (warnings) shown between messages but never sent to the LLM.
    #[serde(default)]
    pub notices: Vec<NoticeTurn>,
    /// Notice turns already written to the jsonl file.
    #[serde(skip)]
    pub persisted_notices: usize,
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
    /// Files modified by `write` / `edit`, in first-touch order (rebuilt from history).
    #[serde(skip)]
    pub modified: TrackedFiles,
    /// Files read by the `read` tool (rebuilt from history).
    #[serde(skip)]
    pub accessed: TrackedFiles,
    /// Current round id — bumped per user message, rebuilt from history on load.
    #[serde(skip)]
    round: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Paths touched by tools, in first-touch order, with the last round each was
/// touched. Derived from history on load; never serialised directly.
#[derive(Debug, Clone, Default)]
pub struct TrackedFiles {
    pub files: Vec<String>,
    rounds: HashMap<String, u32>,
}

impl TrackedFiles {
    /// Record files touched by a completed tool call.
    fn track_result(modified: &mut Self, accessed: &mut Self, round: u32, tr: &ToolResult) {
        if let Some(path) = tr.get_modified_file() {
            modified.stamp(round, path);
        }
        if let Some(path) = tr.get_read_file() {
            accessed.stamp(round, path);
        }
    }

    fn stamp(&mut self, round: u32, path: &str) {
        if let Some(last) = self.rounds.get_mut(path) {
            *last = round;
            return;
        }
        let key = path.to_string();
        self.files.push(key.clone());
        self.rounds.insert(key, round);
    }

    /// True if `path` was touched in `round`.
    fn this_round(&self, round: u32, path: &str) -> bool {
        self.rounds.get(path).is_some_and(|r| *r == round)
    }
}

/// Reconstructed dialogs plus the history/notice records each one owns.
struct DialogBuild {
    dialogs: Vec<Dialog>,
    modified: TrackedFiles,
    accessed: TrackedFiles,
    round: u32,
    /// Ask dialogs and the last history index of their exchange.
    ask_exchanges: HashMap<usize, usize>,
    /// Owner dialog index per history message; `None` for system records.
    history_owners: Vec<Option<usize>>,
    /// Owner dialog index per notice.
    notice_owners: Vec<Option<usize>>,
}

/// Ask dialog waiting for a turn after its tool response.
struct PendingAsk {
    start: usize,
    title: String,
    mode: Option<WorkMode>,
    tool_turn: Turn,
}

/// Incremental dialog reconstruction that tracks which records each dialog owns.
struct DialogBuilder<'a> {
    history: &'a [ChatMessage],
    notices: &'a [NoticeTurn],
    dialogs: Vec<Dialog>,
    modified: TrackedFiles,
    accessed: TrackedFiles,
    round: u32,
    results: HashMap<String, String>,
    ask_exchanges: HashMap<usize, usize>,
    history_owners: Vec<Option<usize>>,
    notice_owners: Vec<Option<usize>>,
    notice_index: usize,
    pending_ask: Option<PendingAsk>,
}

impl<'a> DialogBuilder<'a> {
    /// Rebuild dialogs from `history`, recording the records each one owns.
    fn build(history: &'a [ChatMessage], notices: &'a [NoticeTurn]) -> DialogBuild {
        let results = history
            .iter()
            .filter(|msg| msg.role == ChatRole::Tool)
            .flat_map(|msg| msg.content.tool_responses())
            .map(|tr| (tr.call_id.clone(), tr.content.clone()))
            .collect();
        let mut builder = Self {
            history,
            notices,
            dialogs: Vec::new(),
            modified: TrackedFiles::default(),
            accessed: TrackedFiles::default(),
            round: 0,
            results,
            ask_exchanges: HashMap::new(),
            history_owners: vec![None; history.len()],
            notice_owners: vec![None; notices.len()],
            notice_index: 0,
            pending_ask: None,
        };
        for index in 0..history.len() {
            builder.push_notices(index);
            builder.process(index);
        }
        // Tail notices have no following message to anchor to.
        builder.push_notices(usize::MAX);
        builder.close_pending();
        builder.finish()
    }

    /// Append `turn` to the last dialog, creating one when none exists.
    fn push(&mut self, turn: Turn) -> usize {
        match self.dialogs.last_mut() {
            Some(dialog) => {
                dialog.turns.push(turn);
                self.dialogs.len() - 1
            }
            None => {
                self.dialogs
                    .push(Dialog::new(String::new(), vec![turn], None));
                0
            }
        }
    }

    /// Emit notices anchored at or before `upto`, before the following message.
    fn push_notices(&mut self, upto: usize) {
        while self
            .notices
            .get(self.notice_index)
            .is_some_and(|notice| notice.at <= upto)
        {
            self.open_pending(upto);
            let text = self.notices[self.notice_index].text.clone();
            let owner = self.push(Turn::assistant(text, None));
            self.notice_owners[self.notice_index] = Some(owner);
            self.notice_index += 1;
        }
    }

    /// Open the pending ask dialog before a turn that follows its response.
    fn open_pending(&mut self, upto: usize) {
        let Some(ask) = self.pending_ask.take() else {
            return;
        };
        let index = self.dialogs.len();
        self.dialogs
            .push(Dialog::new(ask.title, vec![ask.tool_turn], ask.mode));
        let end = upto.min(self.history.len());
        let cutoff = (ask.start + 1..end)
            .rfind(|&i| self.history[i].role == ChatRole::Tool)
            .unwrap_or(ask.start);
        self.ask_exchanges.insert(index, cutoff);
        for i in ask.start..end {
            if self.history[i].role != ChatRole::System {
                self.history_owners[i] = Some(index);
            }
        }
    }

    /// Merge a pending ask into the previous dialog when no turn follows.
    fn close_pending(&mut self) {
        let Some(ask) = self.pending_ask.take() else {
            return;
        };
        let Some(index) = self.dialogs.len().checked_sub(1) else {
            return;
        };
        self.dialogs[index].turns.push(ask.tool_turn);
        self.history_owners[ask.start] = Some(index);
    }

    fn process(&mut self, index: usize) {
        match self.history[index].role {
            ChatRole::System => {}
            ChatRole::User => self.user(index),
            ChatRole::Assistant => self.assistant(index),
            ChatRole::Tool => self.history_owners[index] = self.dialogs.len().checked_sub(1),
        }
    }

    fn user(&mut self, index: usize) {
        if image_marker_paths(&self.history[index]).is_some() {
            // Image markers ride along in history but are not user turns.
            self.history_owners[index] = self.dialogs.len().checked_sub(1);
            return;
        }
        if let Some(items) = attachment_marker_items(&self.history[index]) {
            // Attachment markers become their own display block.
            self.open_pending(index);
            let owner = self.push(Turn::attachment(items));
            self.history_owners[index] = Some(owner);
            return;
        }
        let mode = mode_from_user_message(&self.history[index]);
        let text = self.history[index]
            .content
            .parts()
            .last()
            .and_then(|part| part.as_text())
            .unwrap_or_default()
            .to_string();
        self.close_pending();
        let title = Session::derive_title(&text);
        self.dialogs
            .push(Dialog::new(title, vec![Turn::user(text)], mode));
        self.history_owners[index] = Some(self.dialogs.len() - 1);
        self.round += 1;
    }

    fn assistant(&mut self, index: usize) {
        let (text, reasoning) = {
            let msg = &self.history[index];
            (
                msg.content.joined_texts().unwrap_or_default(),
                msg.content.first_reasoning_content().map(str::to_string),
            )
        };
        let mut owner = None;
        if !text.is_empty() || reasoning.is_some() {
            self.open_pending(index);
            owner = Some(self.push(Turn::assistant(text, reasoning)));
        }

        let mut trs = Vec::new();
        for tc in self.history[index].content.tool_calls() {
            let content = self.results.remove(&tc.call_id).unwrap_or_default();
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
            TrackedFiles::track_result(&mut self.modified, &mut self.accessed, self.round, &tr);
            trs.push(tr);
        }
        if !trs.is_empty() {
            if let Some(title) = trs
                .iter()
                .find_map(|tr| ask_dialog_title(&tr.name, &tr.args))
            {
                // A new ask starts its own dialog; text above may have opened one.
                self.close_pending();
                let mode = self.dialogs.last().and_then(|dialog| dialog.mode);
                self.pending_ask = Some(PendingAsk {
                    start: index,
                    title,
                    mode,
                    tool_turn: Turn::from_tool_results(trs),
                });
            } else {
                self.open_pending(index);
                owner = Some(self.push(Turn::from_tool_results(trs)));
            }
        }
        self.history_owners[index] = owner;
    }

    fn finish(self) -> DialogBuild {
        DialogBuild {
            dialogs: self.dialogs,
            modified: self.modified,
            accessed: self.accessed,
            round: self.round,
            ask_exchanges: self.ask_exchanges,
            history_owners: self.history_owners,
            notice_owners: self.notice_owners,
        }
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

/// First non-blank line of `text`, truncated to `max` chars with an ellipsis.
pub fn first_line_label(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_string(),
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
            notices: Vec::new(),
            persisted_notices: 0,
            dialogs: Vec::new(),
            persisted: 0,
            saved_meta: None,
            requests: 0,
            tokens: TokenAmount::default(),
            cost: 0.0,
            currency: Currency::new(),
            modified: TrackedFiles::default(),
            accessed: TrackedFiles::default(),
            round: 0,
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
        // Notice turns are dialog-only; the copy rebuilds dialogs from history.
        session.notices.clear();
        session.persisted_notices = 0;
        session.saved_meta = None;
        session
    }

    /// Fork: fresh copy with its own usage accounting and audit trail.
    pub fn fork(&self) -> Self {
        let mut session = self.fresh_copy();
        // Keep only cumulative prompt/output counts.
        session.tokens = TokenAmount {
            prompt: session.tokens.prompt,
            output: session.tokens.output,
            ..Default::default()
        };
        // Drop system-prompt audit records, then rebuild dialogs from history
        // so dialog-only turns (error/notice messages) don't carry over.
        session.history.retain(|m| m.role != ChatRole::System);
        session.rebuild_dialogs();
        // Save the forked session; failures are logged inside `save`.
        if session.workspace.is_dir() {
            let _ = session.save();
        }
        session
    }

    /// Keep user prompts, ask exchanges (answers and timeouts), and final replies.
    pub fn compact(&self) -> Self {
        let mut session = self.fresh_copy();
        let asks = ask_answers(&self.history);
        // Keep the last text-only assistant reply per dialog; flush it before
        // the next user prompt and at the end.
        let mut answer: Option<ChatMessage> = None;
        // Work mode of the latest user prompt, carried into ask prompts.
        let mut mode: Option<WorkMode> = None;
        let mut compacted = Vec::with_capacity(self.dialogs.len() * 2);
        for msg in &self.history {
            match msg.role {
                ChatRole::User => {
                    // Image markers, attachment markers, and auto-injected hints are plumbing.
                    if image_marker_paths(msg).is_some()
                        || attachment_marker_items(msg).is_some()
                        || msg.content.parts().last().and_then(|p| p.as_text())
                            == Some(RENEW_HINT_PROMPT)
                    {
                        continue;
                    }
                    mode = mode_from_user_message(msg);
                    compacted.extend(answer.take());
                    compacted.push(msg.clone());
                }
                ChatRole::Assistant
                    if !msg.content.contains_tool_call()
                        && msg.content.joined_texts().is_some_and(|t| !t.is_empty()) =>
                {
                    answer = Some(msg.clone());
                }
                ChatRole::Assistant => {
                    // Preserve the text and reasoning that introduced an ask exchange.
                    if msg
                        .content
                        .tool_calls()
                        .iter()
                        .any(|tc| tc.fn_name == "ask" && asks.contains_key(tc.call_id.as_str()))
                        && let Some(preamble) = ask_preamble(msg)
                    {
                        compacted.extend(answer.take());
                        compacted.push(preamble);
                    }
                }
                ChatRole::Tool => {
                    for tr in msg.content.tool_responses() {
                        let Some((question, response)) = asks.get(tr.call_id.as_str()) else {
                            continue;
                        };
                        compacted.extend(answer.take());
                        compacted.push(ask_user_message(question, response, mode));
                    }
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

    // ── Round tracking ───────────────────────────────────────────────

    /// Start a new round — called for every user message recorded in history.
    pub fn begin_round(&mut self) {
        self.round += 1;
    }

    /// Record files touched by a completed tool call.
    pub fn track_tool_result(&mut self, tr: &ToolResult) {
        TrackedFiles::track_result(&mut self.modified, &mut self.accessed, self.round, tr);
    }

    /// True if `path` was modified in the current round.
    pub fn modified_this_round(&self, path: &str) -> bool {
        self.modified.this_round(self.round, path)
    }

    /// True if `path` was read in the current round.
    pub fn accessed_this_round(&self, path: &str) -> bool {
        self.accessed.this_round(self.round, path)
    }

    // ── Dialog / turn helpers ────────────────────────────────────────

    /// Add a new empty dialog with the given title and optional work mode;
    /// the title becomes the session title when the session has none yet.
    pub fn add_dialog(&mut self, title: String, mode: Option<WorkMode>) {
        if self.title.is_empty() {
            self.title = title.clone();
        }
        self.dialogs.push(Dialog::new(title, Vec::new(), mode));
    }

    /// Open a user-prompt-like dialog for an `ask` call; returns its index.
    pub fn open_ask_dialog(&mut self, name: &str, args: &serde_json::Value) -> Option<usize> {
        let title = ask_dialog_title(name, args)?;
        let mode = self.dialogs.last().and_then(|d| d.mode);
        self.add_dialog(title, mode);
        Some(self.dialogs.len() - 1)
    }

    /// Push a turn.  A `User` turn starts a new dialog; all other roles
    /// append to the last dialog (creating one if none exists yet).
    pub fn push_turn(&mut self, mut turn: Turn) {
        let now = chrono::Local::now();
        turn.timestamp = now.format("%H:%M:%S").to_string();
        if let Some(last) = self.dialogs.last_mut() {
            last.turns.push(turn);
        } else {
            self.dialogs
                .push(Dialog::new(String::new(), vec![turn], None));
        }
    }

    /// Append a notice turn (warning shown between messages) and persist it as
    /// its own record so it survives a reload; it never enters `history`.
    pub fn push_notice(&mut self, text: String) {
        self.notices.push(NoticeTurn {
            at: self.history.len(),
            text: text.clone(),
        });
        self.push_turn(Turn::assistant(text, None));
        let _ = self.save();
    }

    /// Reference to the last turn across all dialogs.
    pub fn last_turn(&self) -> Option<&Turn> {
        self.dialogs.last().and_then(|d| d.turns.last())
    }

    /// Whether the last turn still has a pending call to `name`.
    pub fn has_pending_call(&self, name: &str) -> bool {
        matches!(
            self.last_turn().map(|turn| &turn.body),
            Some(TurnBody::Temp(calls)) if calls.iter().any(|call| call.name == name)
        )
    }

    /// Whether the last turn still has a pending call with the given id.
    pub fn has_pending_call_id(&self, call_id: &str) -> bool {
        matches!(
            self.last_turn().map(|turn| &turn.body),
            Some(TurnBody::Temp(calls))
                if calls.iter().any(|call| call.call_id.as_deref() == Some(call_id))
        )
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

    /// Derive a short title from text content: the first non-blank line,
    /// truncated to 144 characters.
    pub fn derive_title(text: &str) -> String {
        first_line_label(text, 144)
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
                    TurnBody::Attachment(items) => {
                        matches(&turn.timestamp)
                            || items
                                .iter()
                                .any(|item| matches(item.display_name()) || matches(&item.path))
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
        let built = DialogBuilder::build(&self.history, &self.notices);
        self.modified = built.modified;
        self.accessed = built.accessed;
        self.round = built.round;
        self.dialogs = built.dialogs;
    }

    /// Delete a dialog and its turns; ask dialogs keep the ask call/response.
    pub fn delete_dialog(&mut self, index: usize) -> bool {
        if index >= self.dialogs.len() {
            return false;
        }
        let built = DialogBuilder::build(&self.history, &self.notices);
        let owns_history = built.history_owners.contains(&Some(index));
        if !owns_history && !built.notice_owners.contains(&Some(index)) {
            // UI-only dialog (e.g. a launch that failed before recording history).
            self.dialogs.remove(index);
            return true;
        }

        // An ask dialog keeps its exchange and drops only later owned records.
        let cutoff = built.ask_exchanges.get(&index).copied();
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.history.len());
        for (history_index, (msg, owner)) in std::mem::take(&mut self.history)
            .into_iter()
            .zip(built.history_owners)
            .enumerate()
        {
            if owner == Some(index) && cutoff.is_none_or(|cutoff| history_index > cutoff) {
                removed.push(history_index);
            } else {
                kept.push(msg);
            }
        }
        self.history = kept;
        self.notices = std::mem::take(&mut self.notices)
            .into_iter()
            .zip(built.notice_owners)
            .filter_map(|(mut notice, owner)| {
                if owner == Some(index) {
                    return None;
                }
                // Shift the anchor past the records removed before it.
                notice.at = notice
                    .at
                    .saturating_sub(removed.partition_point(|&index| index < notice.at));
                Some(notice)
            })
            .collect();

        self.rebuild_dialogs();
        if self.save_path().is_some() {
            let _ = self.rewrite();
        }
        true
    }

    /// Whether the dialog is an ask exchange.
    pub fn dialog_is_ask(&self, index: usize) -> bool {
        DialogBuilder::build(&self.history, &self.notices)
            .ask_exchanges
            .contains_key(&index)
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
        self.persist(false, false).inspect_err(|e| {
            tracing::warn!(session = %self.id, "failed to save session: {e}");
        })
    }

    /// Like [`Session::save`], plus a [`SessionRecord::Tally`] line so counters
    /// persist on terminal stream events even without new history.
    pub fn save_with_tally(&mut self) -> Result<(), String> {
        self.persist(false, true).inspect_err(|e| {
            tracing::warn!(session = %self.id, "failed to save session: {e}");
        })
    }

    /// Rewrite the whole session file; normal saves only append new records.
    fn rewrite(&mut self) -> Result<(), String> {
        self.persist(true, true).inspect_err(|e| {
            tracing::warn!(session = %self.id, "failed to rewrite session: {e}");
        })
    }

    /// Append new records, or rewrite the whole file when `rewrite` is set.
    fn persist(&mut self, rewrite: bool, with_tally: bool) -> Result<(), String> {
        let path = self.prepared_save_path()?;
        let is_new = !path.exists();
        let start = if rewrite || is_new {
            0
        } else {
            self.persisted.min(self.history.len())
        };
        let new_messages = self.history.len() - start;

        let meta_json = self.meta_json()?;
        let write_meta = rewrite || is_new || self.saved_meta.as_deref() != Some(&meta_json);
        let notices_from = if rewrite {
            0
        } else {
            self.persisted_notices.min(self.notices.len())
        };
        let buf = self.serialize_records(
            start,
            notices_from,
            write_meta.then_some(meta_json.as_str()),
            with_tally,
        )?;

        let mut file = if rewrite {
            std::fs::File::create(&path)
        } else {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        }
        .map_err(|e| format!("Failed to open session file: {e}"))?;
        file.write_all(buf.as_bytes())
            .map_err(|e| format!("Failed to write session: {e}"))?;

        self.persisted = self.history.len();
        self.persisted_notices = self.notices.len();
        self.saved_meta = Some(meta_json);

        if rewrite || is_new {
            Self::remove_legacy_json(&path);
        }
        tracing::debug!(
            session = %self.id,
            path = %path.display(),
            new_messages,
            rewritten = rewrite,
            "session persisted"
        );
        Ok(())
    }

    /// Session file path, creating its parent directory when needed.
    fn prepared_save_path(&self) -> Result<PathBuf, String> {
        let path = self.save_path().ok_or("No workspace set")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create session dir: {e}"))?;
        }
        Ok(path)
    }

    fn meta_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.meta_record())
            .map_err(|e| format!("Failed to serialize meta: {e}"))
    }

    /// Serialize the optional Meta line plus records from the given offsets.
    fn serialize_records(
        &self,
        messages_from: usize,
        notices_from: usize,
        meta: Option<&str>,
        with_tally: bool,
    ) -> Result<String, String> {
        let mut buf = String::new();
        if let Some(meta) = meta {
            buf.push_str(meta);
            buf.push('\n');
        }
        Self::push_messages(&mut buf, &self.history, messages_from)?;
        Self::push_notices(&mut buf, &self.notices, notices_from)?;
        if with_tally {
            self.push_tally(&mut buf)?;
        }
        Ok(buf)
    }

    fn remove_legacy_json(path: &Path) {
        let legacy = path.with_extension("json");
        if legacy.exists() {
            let _ = std::fs::remove_file(&legacy);
        }
    }

    /// Load a session from disk, preferring the `.jsonl` sibling of a legacy
    /// `.json` path.
    ///
    /// The workspace is re-resolved from the file's own location — not its
    /// `Meta` record — so sessions survive the project being moved or renamed;
    /// the stale `saved_meta` makes the next save rewrite the `Meta` line.
    pub fn load(path: &Path) -> Result<Self, String> {
        tracing::debug!(path = %path.display(), "loading session");
        let path = match path.extension().and_then(|e| e.to_str()) {
            Some("json") if path.with_extension("jsonl").exists() => path.with_extension("jsonl"),
            _ => path.to_path_buf(),
        };
        let mut session = match path.extension().and_then(|e| e.to_str()) {
            Some("jsonl") => Self::load_jsonl(&path),
            _ => Self::load_legacy_json(&path),
        }?;
        if let Some(workspace) = workspace_from_path(&path) {
            session.workspace = workspace;
        }
        Ok(session)
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
                Ok(SessionRecord::Notice { at, text }) => {
                    session.notices.push(NoticeTurn { at, text });
                }
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
        session.persisted_notices = session.notices.len();
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

    /// Append `notices[start..]` as one `Notice` record line each.
    fn push_notices(buf: &mut String, notices: &[NoticeTurn], start: usize) -> Result<(), String> {
        for notice in &notices[start..] {
            let line = serde_json::to_string(&SessionRecord::Notice {
                at: notice.at,
                text: notice.text.clone(),
            })
            .map_err(|e| format!("Failed to serialize notice: {e}"))?;
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

// ── Ask tool helpers ────────────────────────────────────────────────

/// Title for an `ask` call's dialog; `None` for other tools.
fn ask_dialog_title(name: &str, args: &serde_json::Value) -> Option<String> {
    (name == "ask").then(|| Session::derive_title(ask_question(args)))
}

/// Work mode declared by a user message's leading `work-mode: …` part.
fn mode_from_user_message(msg: &ChatMessage) -> Option<WorkMode> {
    msg.content
        .parts()
        .first()
        .and_then(|p| p.as_text())
        .and_then(|t| t.strip_prefix("work-mode: "))
        .filter(|s| !s.is_empty())
        .map(WorkMode::from)
}

/// Completed, non-empty ask exchanges (answers and timeouts) with their questions.
fn ask_answers(history: &[ChatMessage]) -> HashMap<&str, (&str, &str)> {
    let mut questions: HashMap<&str, &str> = HashMap::new();
    let mut answers = HashMap::new();
    for msg in history {
        match msg.role {
            ChatRole::Assistant => {
                for tc in msg.content.tool_calls() {
                    if tc.fn_name == "ask" {
                        questions.insert(tc.call_id.as_str(), ask_question(&tc.fn_arguments));
                    }
                }
            }
            ChatRole::Tool => {
                for tr in msg.content.tool_responses() {
                    if let Some(question) = questions.remove(tr.call_id.as_str())
                        && !tr.content.trim().is_empty()
                        && !is_ask_failure(&tr.content)
                    {
                        answers.insert(tr.call_id.as_str(), (question, tr.content.as_str()));
                    }
                }
            }
            _ => {}
        }
    }
    answers
}

/// Keep a message's text and reasoning parts, dropping its tool calls.
fn ask_preamble(msg: &ChatMessage) -> Option<ChatMessage> {
    let parts: Vec<_> = msg
        .content
        .parts()
        .iter()
        .filter(|part| match part {
            ContentPart::Text(text) => !text.trim().is_empty(),
            ContentPart::ReasoningContent(reasoning) => !reasoning.trim().is_empty(),
            _ => false,
        })
        .cloned()
        .collect();
    (!parts.is_empty()).then(|| ChatMessage::assistant(MessageContent::from_parts(parts)))
}

/// Render an ask exchange (answer or timeout) as a user prompt for compacted history.
fn ask_user_message(question: &str, answer: &str, mode: Option<WorkMode>) -> ChatMessage {
    let prompt = UserPrompt::new(mode, format!("{question}\n\nAnswer: {answer}"), None);
    ChatMessage::user(MessageContent::from_parts(prompt.to_content_parts()))
}

/// Workspace root of a session file — the grandparent of the innermost
/// `.agent`/`sessions` ancestor (the layout written by [`Session::save_path`]).
/// `None` keeps the workspace recorded in the file.
fn workspace_from_path(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1) // the file itself
        .find(|dir| {
            dir.file_name().is_some_and(|n| n == "sessions")
                && dir
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == ".agent")
        })
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
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
