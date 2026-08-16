use std::collections::HashSet;
use std::ops::Range;
use std::sync::LazyLock;

use genai::chat::ChatRole;
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

/// Linkify `text` and parse it; returns the markdown and whether a URL was wrapped.
fn linkified_md(text: &str) -> (Box<iced::widget::markdown::Content>, bool) {
    let (linked, has_url) = linkify_urls(text);
    let md = Box::new(iced::widget::markdown::Content::parse(&linked));
    (md, has_url)
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
        let (md, has_url) = linkified_md(&self.content);
        self.content_md = Some(md);
        self.has_url = has_url;

        if let Some(reasoning) = &self.reasoning {
            let (md, has_url) = linkified_md(reasoning);
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
    /// Append a completed tool result to the in-progress tool group. A result
    /// matching a streaming placeholder replaces it in place; others append,
    /// so parallel batches keep their completion order.
    pub fn push_tool_result(&mut self, tr: ToolResult) {
        let n = self.turns.len();
        if n < 2 {
            return;
        }
        // Parallel tools finish out of order — remove the matching pending call (FIFO fallback).
        let pos = match &self.turns[n - 1].body {
            TurnBody::Temp(calls) => calls.iter().position(|c| c.call_id == tr.call_id),
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
        if !calls.is_empty() {
            calls.remove(pos.unwrap_or(0));
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
/// `linkify_urls` so a link destination can't be corrupted.
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

/// Wrap bare URLs in `<url>` autolink syntax. Code, links/images and raw HTML
/// are untouched. Returns the string and whether any URL was wrapped.
pub fn linkify_urls(text: &str) -> (String, bool) {
    transform_outside(text, &protected_ranges(text), linkify_segment)
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
