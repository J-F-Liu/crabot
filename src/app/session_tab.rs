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
    /// The preamble file selected for this tab (restored when switching back to this tab).
    pub(crate) selected_preamble: String,
    /// End status indicator — set when a stream finishes, cleared when a new dialog starts.
    pub(crate) end_status: Option<SessionEndStatus>,
    /// Hierarchical label path, e.g. `Some([1, 2])` → "Session 1-2" (task
    /// subtask of tab 1); `None` → plain "Session N" for user-created tabs.
    /// Stored as a full path so the label survives the parent tab closing.
    pub(crate) task_path: Option<Vec<usize>>,
    /// Task-tool call_id this tab was spawned for — tags the final report to the parent.
    pub(crate) task_call_id: Option<String>,
    /// Raw file paths whose original content was snapshotted this run.
    pub(crate) snapshot_files: HashSet<String>,
    /// Transient error from the last Revert action.
    pub(crate) modified_files_error: Option<String>,
}

impl SessionTab {
    /// Create a fresh (unsaved, empty) tab with the given number.
    pub(crate) fn new(number: usize, selected_model: String, selected_preamble: String) -> Self {
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
            selected_preamble,
            end_status: None,
            task_path: None,
            task_call_id: None,
            snapshot_files: HashSet::new(),
            modified_files_error: None,
        }
    }

    /// Build a tab from a previously-saved session loaded from disk.
    pub(crate) fn from_session(
        number: usize,
        session: Session,
        selected_model: String,
        selected_preamble: String,
    ) -> Self {
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
            selected_preamble,
            end_status: None,
            task_path: None,
            task_call_id: None,
            snapshot_files: HashSet::new(),
            modified_files_error: None,
        }
    }

    /// Whether this tab has an active LLM stream.
    pub(crate) fn running(&self) -> bool {
        self.session_state.phase != DialogPhase::Idle
    }

    /// "Session 1-2-3" for task subtasks, "Session N" for user-created tabs.
    pub(crate) fn tab_label(&self) -> String {
        match &self.task_path {
            Some(path) => format!(
                "Session {}",
                path.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("-")
            ),
            None => format!("Session {}", self.number),
        }
    }

    /// Direct parent tab number when this tab was spawned by the task tool —
    /// this tab's final report is delivered to the parent's waiting stream.
    /// Derived from [`Self::task_path`]: the second-to-last element.
    pub(crate) fn task_parent(&self) -> Option<usize> {
        self.task_path
            .as_ref()
            .and_then(|p| p.iter().nth_back(1).copied())
    }
}
