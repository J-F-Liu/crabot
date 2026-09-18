use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;
use std::sync::LazyLock;

use genai::chat::{ChatMessage, ChatRole};
use gh_emoji::Replacer;
use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde_json::Value;

use crate::user::WorkMode;

// ── TextContent ──────────────────────────────────────────────────────

/// Plain-text message content (User or Assistant role).
#[derive(Debug, Default)]
pub struct TextContent {
    pub content: String,
    pub reasoning: Option<String>,
    /// Cached parsed Markdown for the text content.
    pub content_md: Option<Box<iced::widget::markdown::Content>>,
    /// Cached parsed Markdown for the reasoning text (if any).
    pub reasoning_md: Option<Box<iced::widget::markdown::Content>>,
    /// True if the content has a bare URL (rendered as a clickable link).
    pub has_url: bool,
    /// True if the reasoning text has a bare URL.
    pub reasoning_has_url: bool,
}

impl Clone for TextContent {
    fn clone(&self) -> Self {
        let mut cloned = Self {
            content: self.content.clone(),
            reasoning: self.reasoning.clone(),
            ..Default::default()
        };
        cloned.refresh_md_cache();
        cloned
    }
}

/// Escape math pipes, linkify URLs, and parse the result as markdown.
fn markdown_content(text: &str) -> (Box<iced::widget::markdown::Content>, bool) {
    let (source, has_url) = markdown_source(text);
    (
        Box::new(iced::widget::markdown::Content::parse(&source)),
        has_url,
    )
}

/// Byte ranges of the GFM table blocks in `text`.
fn table_ranges(text: &str) -> Vec<Range<usize>> {
    Parser::new_ext(text, markdown_options())
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::Start(Tag::Table(_))).then_some(range))
        .collect()
}

/// Escape `|` inside the code and math spans on GFM table lines. A row is split
/// on *every* unescaped bar before inline parsing, so the bar in `` `a|b` ``
/// would truncate the row; `\|` renders as a plain bar inside the cell.
fn escape_table_pipes(text: &str) -> Cow<'_, str> {
    let mut tables = table_ranges(text).into_iter().peekable();
    if tables.peek().is_none() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let (mut offset, mut changed) = (0, false);
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        while tables.peek().is_some_and(|table| table.end <= start) {
            let _ = tables.next();
        }
        // Overlap, not containment: a table in a blockquote/list starts after
        // its `> ` / `- ` marker, mid-line.
        let in_table = tables.peek().is_some_and(|table| table.start < offset);
        if in_table && let Some(escaped) = escape_span_pipes(line) {
            out.push_str(&escaped);
            changed = true;
        } else {
            out.push_str(line);
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// Escape unescaped `|` inside the code and math spans of one line; `None` when
/// the line holds no such bar.
fn escape_span_pipes(line: &str) -> Option<String> {
    let mut out = String::new();
    let (mut written, mut pos) = (0, 0);
    while pos < line.len() {
        let span = match line.as_bytes()[pos] {
            b'`' if !is_escaped(line, pos) => code_span(line, pos),
            b'$' if !is_escaped(line, pos) => math_span(line, pos).body(),
            _ => None,
        };
        let Some((body, after)) = span else {
            pos += unmatched_advance(line, pos);
            continue;
        };
        if let Some(escaped) = escape_pipes(&line[body.start..body.end]) {
            out.push_str(&line[written..body.start]);
            out.push_str(&escaped);
            written = body.end;
        }
        pos = after;
    }
    if out.is_empty() {
        return None;
    }
    out.push_str(&line[written..]);
    Some(out)
}

/// Bytes to advance when no span opens at `pos`. An unescaped `` ` `` or `$$` run
/// that fails to close is kept literal as a whole (CommonMark skips the run);
/// every other byte advances by one character.
fn unmatched_advance(line: &str, pos: usize) -> usize {
    match line.as_bytes()[pos] {
        b'`' if !is_escaped(line, pos) => line[pos..].bytes().take_while(|&b| b == b'`').count(),
        b'$' if !is_escaped(line, pos) && line[pos..].starts_with("$$") => 2,
        _ => line[pos..].chars().next().map_or(1, char::len_utf8),
    }
}

/// Content range of the inline code span opened by the backtick run at `open`,
/// plus the offset just past its matching closing run.
fn code_span(line: &str, open: usize) -> Option<(Range<usize>, usize)> {
    let ticks = line[open..].bytes().take_while(|&b| b == b'`').count();
    let mut pos = open + ticks;
    loop {
        let close = pos + line[pos..].find('`')?;
        let run = line[close..].bytes().take_while(|&b| b == b'`').count();
        if run == ticks {
            return Some((open + ticks..close, close + ticks));
        }
        pos = close + run;
    }
}

/// A math span opened by an unescaped `$`.
enum MathSpan {
    /// Body range plus the offset just past the closing delimiter.
    Body(Range<usize>, usize),
    /// Literal `$` next to whitespace, e.g. currency `$ 5`.
    Literal,
    /// No closing delimiter follows; no later `$` can close either.
    Unclosed,
}

impl MathSpan {
    /// Body range and end offset, if the span is closed.
    fn body(self) -> Option<(Range<usize>, usize)> {
        match self {
            Self::Body(body, after) => Some((body, after)),
            _ => None,
        }
    }
}

/// Opening delimiter of the math span at `open`: `("$$", true)` for display math.
fn math_delim(text: &str, open: usize) -> (&'static str, bool) {
    if text[open..].starts_with("$$") {
        ("$$", true)
    } else {
        ("$", false)
    }
}

/// Scan the math span opened at the unescaped `$` at `open`.
fn math_span(text: &str, open: usize) -> MathSpan {
    let (delim, display) = math_delim(text, open);
    let start = open + delim.len();
    if !display && text[start..].starts_with(char::is_whitespace) {
        return MathSpan::Literal; // `$ 5` stays literal
    }
    match math_close(text, open, display) {
        Some(close) => MathSpan::Body(start..close, close + delim.len()),
        None => MathSpan::Unclosed,
    }
}

/// Escape `|` inside `$...$` / `$$...$$` math spans, so formula bars are not read
/// as GFM table column separators. `\$` never opens a span and `$` next to
/// whitespace stays literal (currency like `$ 5`).
fn escape_math_pipes(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let (mut written, mut search) = (0, 0);
    while let Some(open) = find_unescaped(text, '$', search) {
        match math_span(text, open) {
            MathSpan::Body(body, after) => {
                if let Some(escaped) = escape_pipes(&text[body.start..body.end]) {
                    out.push_str(&text[written..body.start]);
                    out.push_str(&escaped);
                    written = body.end;
                }
                search = after;
            }
            MathSpan::Literal => search = open + 1, // `$ 5` stays literal
            MathSpan::Unclosed => break,            // no later `$` can close either
        }
    }
    if written == 0 {
        return Cow::Borrowed(text); // no bar inside any span
    }
    out.push_str(&text[written..]);
    Cow::Owned(out)
}

/// Escape unescaped `|` in `content`, or `None` when there is no bar to escape.
fn escape_pipes(content: &str) -> Option<String> {
    let mut out = String::new();
    let mut pos = 0;
    while let Some(bar) = find_unescaped(content, '|', pos) {
        out.push_str(&content[pos..bar]);
        out.push_str("\\|");
        pos = bar + 1;
    }
    if pos == 0 {
        return None;
    }
    out.push_str(&content[pos..]);
    Some(out)
}

/// Byte index of the closing delimiter of the math span opened at `open`, if any.
/// An inline closer must be preceded by a non-space (`$5 and $10` stays literal)
/// and a span may not cross a block boundary — a table cell is single-line.
fn math_close(text: &str, open: usize, display: bool) -> Option<usize> {
    let delim_len = if display { 2 } else { 1 };
    let mut pos = open + delim_len;
    loop {
        let close = find_unescaped(text, '$', pos)?;
        let closes = if display {
            text[close..].starts_with("$$")
        } else {
            !text[..close].ends_with(char::is_whitespace)
        };
        if !closes {
            pos = close + 1;
            continue;
        }
        // A blank line ends a display span, a line break an inline one; every
        // later closer lies past that boundary too.
        let span = &text[open..close + delim_len];
        let split = if display {
            span.lines().any(|line| line.trim().is_empty())
        } else {
            span.contains(['\n', '\r'])
        };
        return (!split).then_some(close);
    }
}

/// Byte index of the next unescaped `ch` at or after `from`.
fn find_unescaped(text: &str, ch: char, from: usize) -> Option<usize> {
    let mut pos = from;
    loop {
        let idx = pos + text[pos..].find(ch)?;
        if !is_escaped(text, idx) {
            return Some(idx);
        }
        pos = idx + ch.len_utf8();
    }
}

/// True if the character at `pos` is preceded by an odd number of backslashes.
fn is_escaped(text: &str, pos: usize) -> bool {
    let before = &text[..pos];
    before.bytes().rev().take_while(|&b| b == b'\\').count() % 2 == 1
}

impl TextContent {
    /// Create a new text content, parsing markdown caches immediately.
    pub fn new(content: String, reasoning: Option<String>) -> Self {
        let mut tc = Self {
            content,
            reasoning,
            ..Default::default()
        };
        if !tc.content.is_empty() || tc.reasoning.is_some() {
            tc.refresh_md_cache();
        }
        tc
    }

    /// Ensure the markdown cache is up to date with the raw text content.
    pub fn refresh_md_cache(&mut self) {
        let (md, has_url) = markdown_content(&self.content);
        self.content_md = Some(md);
        self.has_url = has_url;

        if let Some(reasoning) = &self.reasoning {
            let (md, has_url) = markdown_content(reasoning);
            self.reasoning_md = Some(md);
            self.reasoning_has_url = has_url;
        } else {
            self.reasoning_md = None;
            self.reasoning_has_url = false;
        }
    }
}

// ── ToolResult ───────────────────────────────────────────────────────

/// Paired tool call and its execution result.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub name: String,
    pub call_id: Option<String>,
    /// Tool call arguments as provided by the LLM.
    pub args: Value,
    /// Execution result — Ok(success) or Err(failure).
    pub result: Result<String, String>,
    /// Time the tool finished execution (HH:MM:SS).
    pub timestamp: String,
    /// True while the tool is still running and `result` holds partial
    /// streamed output. Transient — never persisted; the final result
    /// replaces the placeholder in place.
    pub streaming: bool,
}

impl ToolResult {
    /// If this is a successful `write` or `edit` tool call, return the
    /// file path that was modified.
    pub fn get_modified_file(&self) -> Option<&str> {
        if self.result.is_ok() && (self.name == "write" || self.name == "edit") {
            crate::tools::arg_path(&self.args)
        } else {
            None
        }
    }

    /// If this is a successful `read` tool call, return the file path that was read.
    pub fn get_read_file(&self) -> Option<&str> {
        if self.result.is_ok() && self.name == "read" {
            crate::tools::arg_path(&self.args)
        } else {
            None
        }
    }

    /// Track the file modified by this call (write / edit) in `tracked`.
    pub fn track_modified_file(&self, tracked: &mut Vec<String>) {
        push_unique(tracked, self.get_modified_file());
    }

    /// Track the file read by this call in `tracked`.
    pub fn track_read_file(&self, tracked: &mut Vec<String>) {
        push_unique(tracked, self.get_read_file());
    }
}

/// Append `path` to `tracked` unless already present.
fn push_unique(tracked: &mut Vec<String>, path: Option<&str>) {
    if let Some(path) = path
        && !tracked.iter().any(|p| p == path)
    {
        tracked.push(path.to_string());
    }
}

// ── Error envelope ───────────────────────────────────────────────────

/// Error-prefix marker for tool results, so the reload path can tell success from failure.
pub const ERROR_ENVELOPE: &str = "Error: ";

/// Wrap in [`ERROR_ENVELOPE`], skipping if already present (case-insensitive).
pub fn envelope_error(e: &str) -> String {
    if is_enveloped_error(e) {
        e.to_string()
    } else {
        format!("{ERROR_ENVELOPE}{e}")
    }
}

/// True if `s` starts with [`ERROR_ENVELOPE`] (case-insensitive, space after colon required).
pub fn is_enveloped_error(s: &str) -> bool {
    s.get(..ERROR_ENVELOPE.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(ERROR_ENVELOPE))
}

/// Remove [`ERROR_ENVELOPE`] from `s`; non-enveloped strings are returned unchanged.
pub fn strip_error_envelope(s: &str) -> &str {
    s.strip_prefix(ERROR_ENVELOPE).unwrap_or(s)
}

// ── ToolCall ─────────────────────────────────────────────────────────

/// A pending tool call that hasn't produced a result yet.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub call_id: Option<String>,
    pub args: serde_json::Value,
}

// ── TurnBody ────────────────────────────────────────────────────────

/// Body of a single turn in the conversation.
#[derive(Debug, Clone)]
pub enum TurnBody {
    /// Plain-text message (User or Assistant role).
    Text(TextContent),
    /// Paired tool calls and their results (one or more, from a single response).
    Tool(Vec<ToolResult>),
    /// Pending tool calls — execution in progress, no results yet.
    Temp(Vec<ToolCall>),
}

// ── Turn ────────────────────────────────────────────────────────────

/// A single turn in the conversation history, formatted for UI display.
#[derive(Debug, Clone)]
pub struct Turn {
    pub role: ChatRole,
    pub body: TurnBody,
    pub timestamp: String,
}

// ── Dialog ──────────────────────────────────────────────────────────

/// A named conversation — a sequence of turns grouped under a title.
#[derive(Debug, Clone)]
pub struct Dialog {
    pub title: String,
    pub turns: Vec<Turn>,
    /// Work mode under which this dialog was launched.
    pub mode: Option<WorkMode>,
}

impl Dialog {
    /// Title shown in the dialog header ("Dialog N" fallback for empty titles).
    pub fn display_title(&self, index: usize) -> String {
        if self.title.is_empty() {
            format!("Dialog {}", index + 1)
        } else {
            self.title.clone()
        }
    }

    /// Append a completed tool result to the in-progress tool group. A result
    /// matching a streaming placeholder replaces it in place; others append,
    /// so parallel batches keep their completion order.
    pub fn push_tool_result(&mut self, tr: ToolResult) {
        let n = self.turns.len();
        if n < 2 {
            return;
        }
        // Parallel tools finish out of order — remove the matching pending call;
        // stale ids fall back to FIFO so the group still drains.
        let pos =
            match &self.turns[n - 1].body {
                TurnBody::Temp(calls) => calls
                    .iter()
                    .position(|c| c.call_id == tr.call_id)
                    .or_else(|| {
                        (tr.call_id.is_some()).then(|| {
                            tracing::warn!(
                                call_id = ?tr.call_id,
                                "tool result matched no pending call"
                            );
                            0
                        })
                    }),
                _ => None,
            };
        if let TurnBody::Tool(trs) = &mut self.turns[n - 2].body {
            if let Some(slot) = tr
                .call_id
                .as_ref()
                .and_then(|id| trs.iter_mut().find(|t| t.call_id.as_ref() == Some(id)))
            {
                *slot = tr; // replace the streaming placeholder in place
            } else {
                trs.push(tr);
            }
        }
        let TurnBody::Temp(calls) = &mut self.turns[n - 1].body else {
            return;
        };
        if let Some(pos) = pos {
            calls.remove(pos);
        }
        if calls.is_empty() {
            self.turns.pop();
        }
    }

    /// Append an incremental output chunk to the streaming placeholder of the
    /// still-pending call `call_id`, creating the placeholder on the first
    /// chunk. Returns the placeholder's index and whether it was created.
    pub fn push_tool_output(
        &mut self,
        call_id: Option<&str>,
        chunk: &str,
    ) -> Option<(usize, bool)> {
        let n = self.turns.len();
        if n < 2 {
            return None;
        }
        // Chunks belong only to still-pending calls — stale ones are dropped.
        let TurnBody::Temp(calls) = &self.turns[n - 1].body else {
            return None;
        };
        let call = calls.iter().find(|c| c.call_id.as_deref() == call_id)?;
        let (name, args) = (call.name.clone(), call.args.clone());
        let TurnBody::Tool(trs) = &mut self.turns[n - 2].body else {
            return None;
        };
        let (idx, created) = match trs.iter().position(|t| t.call_id.as_deref() == call_id) {
            Some(idx) => (idx, false),
            None => {
                trs.push(ToolResult {
                    name,
                    call_id: call_id.map(str::to_string),
                    args,
                    result: Ok(String::new()),
                    timestamp: String::new(),
                    streaming: true,
                });
                (trs.len() - 1, true)
            }
        };
        let tr = &mut trs[idx];
        if tr.streaming
            && let Ok(buffer) = &mut tr.result
        {
            buffer.push_str(chunk);
        }
        Some((idx, created))
    }
}

impl Turn {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            body: TurnBody::Text(TextContent::new(content.into(), None)),
            timestamp: String::new(),
        }
    }

    pub fn assistant(content: impl Into<String>, reasoning: Option<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            body: TurnBody::Text(TextContent::new(replace_emoji(&content.into()), reasoning)),
            timestamp: String::new(),
        }
    }

    pub fn from_tool_results(results: Vec<ToolResult>) -> Self {
        Self {
            role: ChatRole::Tool,
            body: TurnBody::Tool(results),
            timestamp: String::new(),
        }
    }

    pub fn from_tool_calls(calls: Vec<ToolCall>) -> Self {
        Self {
            role: ChatRole::Tool,
            body: TurnBody::Temp(calls),
            timestamp: String::new(),
        }
    }

    /// Role label shown in the turn header ("User"/"Assistant"/"System").
    pub fn role_label(&self) -> &'static str {
        match self.role {
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
            _ => "System",
        }
    }
}

/// An assistant message with no text, reasoning, or tool calls is meaningless — never persist or resend it.
pub fn assistant_msg_is_empty(msg: &ChatMessage) -> bool {
    msg.role == ChatRole::Assistant
        && msg.content.joined_texts().is_none_or(|t| t.is_empty())
        && msg.content.first_reasoning_content().is_none()
        && msg.content.tool_calls().is_empty()
}

/// Static emoji replacer — compiled once and reused.
static EMOJI: LazyLock<Replacer> = LazyLock::new(Replacer::new);

/// Static link finder — URLs only (emails stay plain text).
static LINK_FINDER: LazyLock<LinkFinder> = LazyLock::new(|| {
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    finder
});

/// Markdown parser options matching the iced renderer.
pub fn markdown_options() -> Options {
    Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

/// Apply `f` outside `protected` ranges (kept verbatim); returns string + change flag.
fn transform_outside(
    text: &str,
    protected: &[Range<usize>],
    mut f: impl FnMut(&str) -> (String, bool),
) -> (String, bool) {
    let mut result = String::with_capacity(text.len());
    let mut changed = false;
    let mut pos = 0;
    for range in protected {
        if pos < range.start {
            let (part, c) = f(&text[pos..range.start]);
            changed |= c;
            result.push_str(&part);
        }
        result.push_str(&text[range.start..range.end]);
        pos = range.end;
    }
    if pos < text.len() {
        let (part, c) = f(&text[pos..]);
        changed |= c;
        result.push_str(&part);
    }
    (result, changed)
}

/// Replace `:emoji:` codes with Unicode, skipping the regions protected by
/// `markdown_source` so a link destination can't be corrupted.
pub fn replace_emoji(text: &str) -> String {
    transform_outside(text, &protected_ranges(text), |s| {
        let replaced = EMOJI.replace_all(s);
        let changed = replaced != s;
        (replaced.into_owned(), changed)
    })
    .0
}

/// Byte ranges of code, link/image and raw-HTML constructs (merged, sorted).
fn protected_ranges(text: &str) -> Vec<Range<usize>> {
    let mut protected = Vec::new();
    let mut block_start: Option<usize> = None;
    let mut link_start = Vec::new();
    let mut image_start = Vec::new();
    for (event, range) in Parser::new_ext(text, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => block_start = Some(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = block_start.take() {
                    protected.push(start..range.end);
                }
            }
            Event::Code(_) => protected.push(range),
            Event::Start(Tag::Link { .. }) => link_start.push(range.start),
            Event::End(TagEnd::Link) => {
                if let Some(start) = link_start.pop() {
                    protected.push(start..range.end);
                }
            }
            Event::Start(Tag::Image { .. }) => image_start.push(range.start),
            Event::End(TagEnd::Image) => {
                if let Some(start) = image_start.pop() {
                    protected.push(start..range.end);
                }
            }
            Event::Html(_) | Event::InlineHtml(_) => protected.push(range),
            _ => {}
        }
    }
    // Unclosed code block — extend to end of text.
    if let Some(start) = block_start {
        protected.push(start..text.len());
    }
    // Merge overlapping ranges (e.g. an image inside a link).
    protected.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(protected.len());
    for range in protected {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

/// Markdown source for `text`: table-cell and math pipes escaped, bare URLs
/// wrapped (the flag reports whether a URL was wrapped). Code, links/images and
/// raw HTML stay verbatim so a link destination can't be corrupted.
pub fn markdown_source(text: &str) -> (String, bool) {
    let text = escape_table_pipes(text);
    transform_outside(&text, &protected_ranges(&text), |segment| {
        linkify_segment(&escape_math_pipes(segment))
    })
}

/// Wrap bare URLs in `segment` with `<url>` autolink syntax.
fn linkify_segment(segment: &str) -> (String, bool) {
    let mut result = String::with_capacity(segment.len());
    let mut changed = false;
    let mut last = 0;
    for link in LINK_FINDER.links(segment) {
        changed = true;
        result.push_str(&segment[last..link.start()]);
        result.push('<');
        result.push_str(link.as_str());
        result.push('>');
        last = link.end();
    }
    if last < segment.len() {
        result.push_str(&segment[last..]);
    }
    (result, changed)
}

// ── tool-item flattening ───────────────────────────────────────────

/// A flattened tool item: (name, args, result, timestamp, streaming).
pub type ToolItem<'a> = (
    &'a str,
    &'a Value,
    Option<&'a Result<String, String>>,
    &'a str,
    bool,
);

/// Flatten a Tool/Temp turn into renderable items, hiding pending calls already
/// shown by a live streaming placeholder.
pub fn tool_items<'a>(turn: &'a Turn, streaming_ids: &HashSet<&str>) -> Vec<ToolItem<'a>> {
    match &turn.body {
        TurnBody::Tool(trs) => trs
            .iter()
            .map(|tr| {
                (
                    tr.name.as_str(),
                    &tr.args,
                    Some(&tr.result),
                    tr.timestamp.as_str(),
                    tr.streaming,
                )
            })
            .collect(),
        TurnBody::Temp(tcs) => tcs
            .iter()
            .filter(|tc| {
                tc.call_id
                    .as_deref()
                    .is_none_or(|id| !streaming_ids.contains(id))
            })
            .map(|tc| {
                (
                    tc.name.as_str(),
                    &tc.args,
                    None,
                    turn.timestamp.as_str(),
                    false,
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Call ids whose live output is already rendered by a streaming placeholder.
pub fn streaming_tool_ids(dialog: &Dialog) -> HashSet<&str> {
    dialog
        .turns
        .iter()
        .filter_map(|turn| match &turn.body {
            TurnBody::Tool(trs) => Some(trs.iter()),
            _ => None,
        })
        .flatten()
        .filter(|tr| tr.streaming)
        .filter_map(|tr| tr.call_id.as_deref())
        .collect()
}
