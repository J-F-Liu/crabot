use std::collections::HashSet;

use genai::chat::Usage;

use crate::app::session_state::SessionState;
use crate::llm::DialogPhase;
use crate::tools::todo::TodoItem;
use crate::views::search_bar::SearchState;
use crabot::session::Session;

/// A single session tab, owning the in-memory session and all UI state.
#[derive(Debug)]
pub(crate) struct SessionTab {
    /// 1-based monotonic label number — never reused within a run, resets on restart.
    pub(crate) number: usize,
    /// The session proper (provides the implicit `number → session.id` mapping).
    pub(crate) session: Session,
    /// Per-tab streaming lifecycle.
    pub(crate) session_state: SessionState,
    /// Heading displayed in the center-pane header.
    pub(crate) center_pane_title: String,
    /// Token usage for the most recent request in this tab.
    pub(crate) last_usage: Usage,
    /// Expanded (turn, sub-item) keys for tool-call details.
    pub(crate) expanded_turns: HashSet<(usize, usize)>,
    /// Expanded dialog indices.
    pub(crate) expanded_dialogs: HashSet<usize>,
    /// Currently selectable (double-clicked) turn indices.
    pub(crate) selectable_msgs: HashSet<usize>,
    /// Per-tab search state (query, results, offsets, widget ids).
    pub(crate) search: SearchState,
    /// Snapshot of the last successful `todo` tool call for right-pane display.
    pub(crate) todo_items: Vec<TodoItem>,
    /// Saved scroll position (absolute y-offset) to restore when switching back to this tab.
    pub(crate) scroll_offset: Option<f32>,
}

impl SessionTab {
    /// Create a fresh (unsaved, empty) tab with the given number.
    pub(crate) fn new(number: usize) -> Self {
        let session = Session::new();
        Self {
            number,
            session,
            session_state: SessionState::new(),
            center_pane_title: "New session".into(),
            last_usage: genai::chat::Usage::default(),
            expanded_turns: HashSet::new(),
            expanded_dialogs: HashSet::new(),
            selectable_msgs: HashSet::new(),
            search: SearchState::default(),
            todo_items: Vec::new(),
            scroll_offset: None,
        }
    }

    /// Build a tab from a previously-saved session loaded from disk.
    pub(crate) fn from_session(number: usize, session: Session) -> Self {
        let prompt_tokens = session.tokens.prompt;
        let title = session.title.clone();
        let todo_items = session.last_todo_items();
        Self {
            number,
            session,
            session_state: SessionState::new(),
            center_pane_title: title,
            last_usage: Usage {
                prompt_tokens: Some(prompt_tokens),
                ..Default::default()
            },
            expanded_turns: HashSet::new(),
            expanded_dialogs: HashSet::new(),
            selectable_msgs: HashSet::new(),
            search: SearchState::default(),
            todo_items,
            scroll_offset: None,
        }
    }

    /// Whether this tab has an active LLM stream.
    pub(crate) fn running(&self) -> bool {
        self.session_state.phase != DialogPhase::Idle
    }
}
