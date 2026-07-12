use std::cell::{Ref, RefCell};
use std::collections::HashSet;

use iced::Task;
use iced::widget::Id;

use crate::Message;
use crate::chat::TurnBody;
use crate::session::Session;
use crate::views::{measure_turn_offsets, scroll_to_turn_at};

/// UI state and widget bookkeeping for center-pane search.
pub(crate) struct SearchState {
    /// Whether the search bar is visible (toggled via Ctrl+F).
    pub(crate) visible: bool,
    /// Current search query text.
    pub(crate) query: String,
    /// Flat turn indices matching the current query.
    pub(crate) results: Vec<usize>,
    /// Index into `results` for the currently-highlighted match.
    pub(crate) current: usize,
    /// Cached y-offsets for each turn in the scrollable content (pixels).
    /// Indexed by flat turn index. Invalidated when the view changes.
    turn_offsets: Vec<f32>,
    /// Pre-built widget IDs for each turn (flat turn index -> widget ID).
    /// Built once when the turn count changes, cloned cheaply in the view.
    turn_ids: RefCell<Vec<Id>>,
    /// Monotonic counter for in-flight `measure_and_scroll` calls.
    /// Stale results (from earlier clicks) are discarded on arrival.
    measure_generation: u64,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            visible: false,
            query: String::new(),
            results: Vec::new(),
            current: 0,
            turn_offsets: Vec::new(),
            turn_ids: RefCell::new(Vec::new()),
            measure_generation: 0,
        }
    }
}

impl SearchState {
    /// Reset all search-related state to its initial (hidden, empty) state.
    pub(crate) fn reset(&mut self) {
        self.visible = false;
        self.query.clear();
        self.results.clear();
        self.current = 0;
        self.invalidate_offsets();
    }

    /// Clear cached layout measurements after content/layout changes.
    pub(crate) fn invalidate_offsets(&mut self) {
        self.turn_offsets.clear();
    }

    /// Replace the query and clear stale search results.
    pub(crate) fn set_query(&mut self, query: String) {
        self.query = query;
        self.results.clear();
        self.current = 0;
    }

    /// Recompute results for the current query and return the first target.
    pub(crate) fn submit(&mut self, session: &Session) -> Option<usize> {
        self.results = session.search(&self.query);
        self.current = 0;
        self.results.first().copied()
    }

    /// Move the current result pointer by `delta`, wrapping around.
    pub(crate) fn navigate(&mut self, delta: i32) -> Option<usize> {
        if self.results.is_empty() {
            return None;
        }
        let len = self.results.len() as i32;
        self.current = (self.current as i32 + delta).rem_euclid(len) as usize;
        Some(self.results[self.current])
    }

    /// Scroll to a turn using stored offsets. Callers must ensure offsets have been measured.
    pub(crate) fn scroll_to_target(&self, target: usize) -> Option<Task<Message>> {
        self.turn_offsets
            .get(target)
            .copied()
            .map(scroll_to_turn_at)
    }

    /// Store measured offsets if they are from the latest measurement task.
    pub(crate) fn handle_offsets(&mut self, generation: u64, offsets: Vec<f32>) {
        if generation == self.measure_generation {
            self.turn_offsets = offsets;
        }
    }

    /// Ensure `turn_ids` matches the current turn count, rebuilding if needed.
    pub(crate) fn ensure_turn_ids(&self, total: usize) {
        let mut ids = self.turn_ids.borrow_mut();
        if ids.len() < total {
            *ids = (0..total).map(|_| Id::unique()).collect();
        }
    }

    /// Borrow current turn IDs for view construction.
    pub(crate) fn turn_ids(&self) -> Ref<'_, Vec<Id>> {
        self.turn_ids.borrow()
    }

    /// Measure all turn offsets, cache them, and scroll to `target`.
    pub(crate) fn measure_and_scroll(&mut self, total: usize, target: usize) -> Task<Message> {
        self.ensure_turn_ids(total);
        self.measure_generation += 1;
        let generation = self.measure_generation;
        let turn_ids = self.turn_ids.borrow().clone();
        measure_turn_offsets(turn_ids).then(move |offsets| {
            let y = offsets.get(target).copied();
            Task::batch([
                Task::done(Message::TurnOffsetsMeasured(generation, offsets)),
                y.map_or(Task::none(), scroll_to_turn_at),
            ])
        })
    }
}

/// Expand the dialog and turn body for the given flat turn index so the user can
/// see the matching content. Only matching tool items are expanded.
///
/// Returns `true` if any expansion state changed and offsets should be remeasured.
pub(crate) fn expand_result(
    session: &Session,
    expanded_dialogs: &mut HashSet<usize>,
    expanded_turns: &mut HashSet<(usize, usize)>,
    flat_idx: usize,
    query: &str,
) -> bool {
    let q = query.to_lowercase();
    let mut remaining = flat_idx;
    for (di, dialog) in session.dialogs.iter().enumerate() {
        if remaining < dialog.turns.len() {
            let dialog_changed = expanded_dialogs.insert(di);
            let turn = &dialog.turns[remaining];
            let turn_changed = match &turn.body {
                TurnBody::Text(tc) => {
                    if tc.reasoning.is_some() {
                        // `expanded_turns` tracks collapsed reasoning; remove to expand.
                        expanded_turns.remove(&(flat_idx, 0))
                    } else {
                        false
                    }
                }
                TurnBody::Tool(trs) => {
                    // `expanded_turns` tracks expanded tool items; insert matching items.
                    let mut changed = false;
                    for (idx, tr) in trs.iter().enumerate() {
                        let matches = tr.name.to_lowercase().contains(&q)
                            || tr.args.to_string().to_lowercase().contains(&q)
                            || match &tr.result {
                                Ok(s) => s.to_lowercase().contains(&q),
                                Err(e) => e.to_lowercase().contains(&q),
                            };
                        if matches && expanded_turns.insert((flat_idx, idx)) {
                            changed = true;
                        }
                    }
                    changed
                }
                TurnBody::Temp(_) => false,
            };
            return dialog_changed || turn_changed;
        }
        remaining -= dialog.turns.len();
    }
    false
}
