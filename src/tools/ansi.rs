//! ANSI escape stripping and control-character cleanup for shell output.
//!
//! What a terminal renders as styling or screen control — colors, cursor moves,
//! progress redraws, window titles — is noise to the model and the GUI.
//!
//! [`PlainText`] filters incrementally: escape state lives in the filter, so a
//! sequence split across reads is still consumed whole, and a held `\r` lets
//! progress frames collapse to the line a terminal would leave on screen. The
//! escape states come from [`anstyle_parse`]; the printable/control policy is
//! ours.

use anstyle_parse::state::{Action, State, state_change};

/// Byte that introduces every escape sequence.
const ESC: u8 = 0x1B;

/// Incremental terminal-text filter.
pub(crate) struct PlainText {
    /// Escape-parser state, kept across chunks.
    state: State,
    /// Line built since the last line end; between calls it stays non-empty
    /// only while `cr_pending` holds (see [`Self::is_plain`]).
    line: String,
    /// The last character was `\r`: the next printable one redraws the line,
    /// `ESC[K` erases it, and a `\n` turns it into a `\r\n` line end.
    cr_pending: bool,
    /// The open CSI sequence carries only parameter digits/separators (no
    /// private marker or intermediate); only then does `ESC[K` erase here.
    csi_params_only: bool,
    /// An `ESC[K` erase-in-line was dispatched: the visible frame is gone.
    erase_line: bool,
}

impl PlainText {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Ground,
            line: String::new(),
            cr_pending: false,
            csi_params_only: true,
            erase_line: false,
        }
    }

    /// Strip `text`, returning the readable remainder (`None` when nothing is
    /// left to emit). Plain text comes back untouched, without a copy; a chunk
    /// ending on a bare `\r` holds its line for the next call (or
    /// [`Self::finish`]).
    ///
    /// A redraw (`\r` then new text) collapses only within the buffered line:
    /// plain chunks are emitted as they arrive, so a frame whose prefix was
    /// emitted by an earlier call can't be taken back.
    pub(crate) fn push(&mut self, text: String) -> Option<String> {
        if text.is_empty() {
            return None;
        }
        if self.is_plain(&text) {
            return Some(text);
        }
        let mut clean = String::with_capacity(text.len());
        for (idx, ch) in text.char_indices() {
            if self.state != State::Ground {
                // Escape sequence (or its string payload): consumed whole.
                for &byte in &text.as_bytes()[idx..idx + ch.len_utf8()] {
                    self.advance(byte);
                }
                // Erase-in-line wipes from the cursor to end of line; the
                // held frame is only gone because its `\r` put the cursor at
                // column 0. With the cursor at end of line (`ESC[K` right
                // after printed text) nothing visible is erased.
                if std::mem::take(&mut self.erase_line) && self.cr_pending {
                    self.line.clear();
                    self.cr_pending = false;
                }
                continue;
            }
            match ch {
                '\u{1b}' => self.advance(ESC),
                '\n' => {
                    self.cr_pending = false;
                    emit_line(&mut self.line, &mut clean);
                    clean.push('\n');
                }
                '\r' => self.cr_pending = true,
                // BEL, backspace, DEL, C1, form feed, … carry nothing.
                c if !is_kept(c) => {}
                // The redraw shows itself only when text is printed over the
                // held line; escapes and tabs leave it on screen (`ESC[K`
                // erases it above). A tab moves the cursor off column 0, so it
                // settles the frame: the tabbed text follows it instead of
                // overwriting it.
                c => {
                    if self.cr_pending {
                        self.cr_pending = false;
                        if c != '\t' {
                            self.line.clear();
                        }
                    }
                    self.line.push(c);
                }
            }
        }
        if !self.cr_pending {
            emit_line(&mut self.line, &mut clean); // nothing held for the next chunk
        }
        if clean.is_empty() { None } else { Some(clean) }
    }

    /// Flush at the end of a stream: a trailing `\r` left its frame drawn.
    pub(crate) fn finish(&mut self) -> Option<String> {
        self.cr_pending = false;
        if self.line.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.line))
    }

    /// `true` when `text` passes through untouched: ground state, nothing held
    /// for the next chunk, no escape sequence, and no control character but
    /// `\t`/`\n`.
    fn is_plain(&self, text: &str) -> bool {
        self.state == State::Ground
            && !self.cr_pending
            && self.line.is_empty()
            && memchr::memchr(ESC, text.as_bytes()).is_none()
            && text.chars().all(is_kept)
    }

    /// Feed one byte to the escape state machine.
    fn advance(&mut self, byte: u8) {
        let (state, action) = state_change(self.state, byte);
        if state != State::Anywhere {
            // `Anywhere` means "keep the current state".
            self.state = state;
        }
        match self.state {
            State::CsiEntry => self.csi_params_only = true, // `[` just opened
            State::CsiParam | State::CsiIntermediate | State::CsiIgnore
                if !matches!(byte, b'0'..=b'9' | b';' | b':') =>
            {
                self.csi_params_only = false;
            }
            _ => {}
        }
        // Erase-in-line (`ESC[K`, `ESC[2K`, …) wipes the frame a held `\r`
        // would leave; sequences with private markers/intermediates (`ESC[?25l`)
        // dispatch here too but aren't erasures.
        if action == Action::CsiDispatch && byte == b'K' && self.csi_params_only {
            self.erase_line = true;
        }
    }
}

/// `true` for characters the filter keeps: content, plus `\t` and `\n`.
fn is_kept(c: char) -> bool {
    !c.is_control() || c == '\t' || c == '\n'
}

/// Append the buffered line to `out` and reuse its capacity.
fn emit_line(line: &mut String, out: &mut String) {
    out.push_str(line);
    line.clear();
}
