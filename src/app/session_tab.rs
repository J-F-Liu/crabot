use std::collections::HashSet;

use crate::app::session_state::SessionState;
use crate::llm::DialogPhase;
use crate::model::TokenAmount;
use crate::tools::todo::{self, TodoList};
use crate::views::search_bar::SearchState;
use crabot::session::Session;

/// Final state after the session stream finishes — None while running or fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionEndStatus {
    Done,
    Error,
    Cancelled,
}

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
    pub(crate) latest_tokens: TokenAmount,
    /// Expanded (turn, sub-item) keys for tool-call details.
    pub(crate) expanded_turns: HashSet<(usize, usize)>,
    /// Expanded dialog indices.
    pub(crate) expanded_dialogs: HashSet<usize>,
    /// Currently selectable (double-clicked) turn indices.
    pub(crate) selectable_msgs: HashSet<usize>,
    /// Per-tab search state (query, results, offsets, widget ids).
    pub(crate) search: SearchState,
    /// Shared per-tab todo list — the `todo` tool writes to it during this tab's running.
    pub(crate) todo_items: TodoList,
    /// Saved scroll position (absolute y-offset) to restore when switching back to this tab.
    pub(crate) scroll_offset: Option<f32>,
    /// The model selected for this tab (restored when switching back to this tab).
    pub(crate) selected_model: String,
    /// End status indicator — set when a stream finishes, cleared when a new dialog starts.
    pub(crate) end_status: Option<SessionEndStatus>,
}

impl SessionTab {
    /// Create a fresh (unsaved, empty) tab with the given number.
    pub(crate) fn new(number: usize, selected_model: String) -> Self {
        let session = Session::new();
        Self {
            number,
            session,
            session_state: SessionState::new(),
            center_pane_title: "New session".into(),
            latest_tokens: TokenAmount::default(),
            expanded_turns: HashSet::new(),
            expanded_dialogs: HashSet::new(),
            selectable_msgs: HashSet::new(),
            search: SearchState::default(),
            todo_items: TodoList::default(),
            scroll_offset: None,
            selected_model,
            end_status: None,
        }
    }

    /// Build a tab from a previously-saved session loaded from disk.
    pub(crate) fn from_session(number: usize, session: Session, selected_model: String) -> Self {
        let prompt_tokens = session.tokens.prompt;
        let title = session.title.clone();
        let todo_items = todo::create_todo_list(session.last_todo_items());
        Self {
            number,
            session,
            session_state: SessionState::new(),
            center_pane_title: title,
            latest_tokens: TokenAmount {
                prompt: prompt_tokens,
                ..Default::default()
            },
            expanded_turns: HashSet::new(),
            expanded_dialogs: HashSet::new(),
            selectable_msgs: HashSet::new(),
            search: SearchState::default(),
            todo_items,
            scroll_offset: None,
            selected_model,
            end_status: None,
        }
    }

    /// Whether this tab has an active LLM stream.
    pub(crate) fn running(&self) -> bool {
        self.session_state.phase != DialogPhase::Idle
    }
}
