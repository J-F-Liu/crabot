//! A text area component wrapping `iced::widget::text_editor` with undo/redo support.
//!
//! The component follows Iced's widget pattern: `TextArea` holds the editor state
//! and history stacks, `Message` defines its actions, and `update` processes them.
//! `view` renders the component as an `Element`.

use iced::advanced::text::highlighter::PlainText;
use iced::widget::text_editor;
use iced::widget::text_editor::TextEditor;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Messages that the `TextArea` component can process.
#[derive(Debug, Clone)]
pub enum Message {
    /// An edit action from the text editor (insert, delete, paste, etc.).
    Edit(text_editor::Action),
    /// Undo the last edit.
    Undo,
    /// Redo the last undone edit.
    Redo,
}

impl Message {
    /// Returns `true` if this message originates from a mouse click.
    pub fn is_click(&self) -> bool {
        matches!(self, Self::Edit(text_editor::Action::Click(_)))
    }

    /// Returns `true` if this message is an Enter edit (newline insertion).
    pub fn is_enter(&self) -> bool {
        matches!(
            self,
            Self::Edit(text_editor::Action::Edit(text_editor::Edit::Enter))
        )
    }
}

/// A snapshot of the text editor state used for undo/redo.
#[derive(Debug, Clone)]
struct EditSnapshot {
    text: String,
    cursor: text_editor::Cursor,
}

/// Maximum number of undo units retained per stack. When exceeded, the oldest
/// entry is discarded.
const MAX_HISTORY: usize = 100;

/// Maximum idle gap between edits that still coalesce into one undo unit.
const COALESCE_WINDOW: Duration = Duration::from_secs(1);

/// Coalescing class of a [`text_editor::Edit`], used for undo grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// Character typing; inserts coalesce until a word boundary.
    Insert,
    /// Consecutive backspaces coalesce.
    Backspace,
    /// Consecutive forward-deletes coalesce.
    Delete,
    /// Paste, Enter, indent, … — always a standalone undo unit.
    Atomic,
}

impl EditKind {
    fn of(edit: &text_editor::Edit) -> Self {
        match edit {
            text_editor::Edit::Insert(_) => Self::Insert,
            text_editor::Edit::Backspace => Self::Backspace,
            text_editor::Edit::Delete => Self::Delete,
            _ => Self::Atomic,
        }
    }
}

/// The last edit performed, used to coalesce runs into a single undo unit.
#[derive(Debug, Clone, Copy)]
struct LastEdit {
    kind: EditKind,
    at: Instant,
    /// Whether the last inserted character was whitespace (word boundary).
    word_boundary: bool,
}

/// A multi-line text input component with undo/redo history.
///
/// Wraps [`text_editor::Content`] and maintains bounded undo/redo stacks.
/// Consecutive edits of the same kind (typing, backspacing, …) coalesce
/// into one undo unit, so undo reverts a run rather than a single keystroke.
pub struct TextArea {
    content: text_editor::Content,
    undo_stack: VecDeque<EditSnapshot>,
    redo_stack: VecDeque<EditSnapshot>,
    /// The most recent edit; `None` after navigation, undo/redo, or reset.
    last_edit: Option<LastEdit>,
}

impl TextArea {
    /// Creates a new empty `TextArea`.
    pub fn new() -> Self {
        Self {
            content: text_editor::Content::new(),
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            last_edit: None,
        }
    }

    /// Creates a `TextArea` pre-filled with the given text.
    #[allow(dead_code)]
    pub fn with_text(text: &str) -> Self {
        Self {
            content: text_editor::Content::with_text(text),
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            last_edit: None,
        }
    }

    /// Processes a [`Message`] and updates internal state accordingly.
    ///
    /// `shift_held` should be `true` when the Shift key is pressed during
    /// the action — this enables Shift+Click selection extension.
    pub fn update(&mut self, message: Message, shift_held: bool) {
        match message {
            Message::Edit(action) => {
                // Shift+Click extends the selection from the previous cursor position.
                if shift_held && matches!(action, text_editor::Action::Click(_)) {
                    let anchor = self.cursor().position;
                    self.perform(action);
                    let cursor = self.cursor();
                    self.move_to(text_editor::Cursor {
                        position: cursor.position,
                        selection: Some(anchor),
                    });
                } else {
                    self.perform(action);
                }
            }
            Message::Undo => self.undo(),
            Message::Redo => self.redo(),
        }
    }

    /// Renders the text area as a [`text_editor`] widget.
    ///
    /// Text editor [`Action`](text_editor::Action)s are wrapped into
    /// [`Message::Edit`] and forwarded via the provided `on_action` callback.
    /// Callers can chain widget methods (e.g. `.height()`) before converting
    /// to [`Element`] via `.into()`.
    pub fn view<'a>(
        &'a self,
        on_action: impl Fn(Message) -> crate::Message + 'a,
    ) -> TextEditor<'a, PlainText, crate::Message> {
        text_editor(&self.content).on_action(move |action| on_action(Message::Edit(action)))
    }

    /// Returns the text content.
    pub fn text(&self) -> String {
        self.content.text()
    }

    /// Returns the current cursor position.
    pub fn cursor(&self) -> text_editor::Cursor {
        self.content.cursor()
    }

    /// Moves the cursor to the given position.
    pub fn move_to(&mut self, cursor: text_editor::Cursor) {
        self.content.move_to(cursor);
    }

    /// Clears all text content and resets the undo/redo history.
    pub fn clear(&mut self) {
        self.content = text_editor::Content::new();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit = None;
    }

    /// Replaces all text content and resets the undo/redo history.
    pub fn set_text(&mut self, text: &str) {
        self.content = text_editor::Content::with_text(text);
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit = None;
    }

    /// Replaces all text content while preserving undo history.
    ///
    /// The previous content is pushed onto the undo stack so the user can
    /// revert via undo, unlike [`set_text`](Self::set_text) which wipes history.
    pub fn replace_text(&mut self, text: &str) {
        self.push_undo();
        self.redo_stack.clear();
        self.last_edit = None;
        self.content = text_editor::Content::with_text(text);
    }

    // ── private helpers ────────────────────────────────────────────────

    fn perform(&mut self, action: text_editor::Action) {
        if let text_editor::Action::Edit(edit) = &action {
            let kind = EditKind::of(edit);
            // Continue the current undo unit only when the same kind of edit
            // follows within the coalescing window (and, for typing, doesn't
            // cross a word boundary); otherwise start a new unit.
            let coalesces = self.last_edit.is_some_and(|last| {
                last.kind == kind
                    && kind != EditKind::Atomic
                    && last.at.elapsed() < COALESCE_WINDOW
                    && !(kind == EditKind::Insert && last.word_boundary)
            });
            if !coalesces {
                self.push_undo();
                self.redo_stack.clear();
            }
            self.last_edit = Some(LastEdit {
                kind,
                at: Instant::now(),
                word_boundary: matches!(edit, text_editor::Edit::Insert(c) if c.is_whitespace()),
            });
        } else {
            // Navigation, selection, and scrolling break the coalescing run.
            self.last_edit = None;
        }
        self.content.perform(action);
    }

    fn undo(&mut self) {
        if let Some(snapshot) = self.undo_stack.pop_back() {
            self.push_redo();
            self.restore_snapshot(&snapshot);
            self.last_edit = None;
        }
    }

    fn redo(&mut self) {
        if let Some(snapshot) = self.redo_stack.pop_back() {
            self.push_undo();
            self.restore_snapshot(&snapshot);
            self.last_edit = None;
        }
    }

    fn push_undo(&mut self) {
        self.undo_stack.push_back(self.snapshot());
        if self.undo_stack.len() > MAX_HISTORY {
            self.undo_stack.pop_front();
        }
    }

    fn push_redo(&mut self) {
        self.redo_stack.push_back(self.snapshot());
        if self.redo_stack.len() > MAX_HISTORY {
            self.redo_stack.pop_front();
        }
    }

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            text: self.content.text(),
            cursor: self.content.cursor(),
        }
    }

    fn restore_snapshot(&mut self, snapshot: &EditSnapshot) {
        self.content = text_editor::Content::with_text(&snapshot.text);
        self.content.move_to(snapshot.cursor);
    }
}

impl Default for TextArea {
    fn default() -> Self {
        Self::new()
    }
}
