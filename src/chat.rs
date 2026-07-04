use std::sync::LazyLock;

use genai::chat::ChatRole;
use gh_emoji::Replacer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── TextContent ──────────────────────────────────────────────────────

/// Plain-text message content (User or Assistant role).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

// ── ToolResult ───────────────────────────────────────────────────────

/// Paired tool call and its execution result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// Tool call arguments as provided by the LLM.
    pub args: Value,
    /// Execution result — Ok(success) or Err(failure).
    pub result: Result<String, String>,
    /// Time the tool finished execution (HH:MM:SS).
    pub timestamp: String,
}

impl ToolResult {
    /// If this is a successful `write` or `edit` tool call, return the
    /// file path that was modified.
    pub fn get_modified_file(&self) -> Option<&str> {
        if self.result.is_ok() && (self.name == "write" || self.name == "edit") {
            self.args.get("path").and_then(|v| v.as_str())
        } else {
            None
        }
    }
}

// ── ToolCall ─────────────────────────────────────────────────────────

/// A pending tool call that hasn't produced a result yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub args: serde_json::Value,
}

// ── TurnBody ────────────────────────────────────────────────────────

/// Body of a single turn in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Serialize, Deserialize)]
pub struct Turn {
    pub role: ChatRole,
    pub body: TurnBody,
    pub timestamp: String,
    /// Cached parsed Markdown for the text content (if any).
    #[serde(skip)]
    pub content_md: Option<iced::widget::markdown::Content>,
}

// ── Dialog ──────────────────────────────────────────────────────────

/// A named conversation — a sequence of turns grouped under a title.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dialog {
    pub title: String,
    pub turns: Vec<Turn>,
}

impl Dialog {
    /// Append a completed tool result to the in-progress tool group.
    pub fn push_tool_result(&mut self, tr: ToolResult) {
        let n = self.turns.len();
        if n >= 2 {
            if let TurnBody::Tool(trs) = &mut self.turns[n - 2].body {
                trs.push(tr);
            }
            if let TurnBody::Temp(calls) = &mut self.turns[n - 1].body {
                if !calls.is_empty() {
                    calls.remove(0);
                }
                if calls.is_empty() {
                    self.turns.pop();
                }
            }
        }
    }
}

impl Turn {
    pub fn user(content: impl Into<String>) -> Self {
        let content: String = content.into();
        let content_md = Some(iced::widget::markdown::Content::parse(&content));
        Self {
            role: ChatRole::User,
            body: TurnBody::Text(TextContent {
                content,
                reasoning: None,
            }),
            timestamp: String::new(),
            content_md,
        }
    }

    pub fn assistant(content: impl Into<String>, reasoning: Option<String>) -> Self {
        let content: String = replace_emoji(&content.into());
        let content_md = Some(iced::widget::markdown::Content::parse(&content));
        Self {
            role: ChatRole::Assistant,
            body: TurnBody::Text(TextContent { content, reasoning }),
            timestamp: String::new(),
            content_md,
        }
    }

    pub fn from_tool_results(results: Vec<ToolResult>) -> Self {
        Self {
            role: ChatRole::Tool,
            body: TurnBody::Tool(results),
            timestamp: String::new(),
            content_md: None,
        }
    }

    pub fn from_tool_calls(calls: Vec<ToolCall>) -> Self {
        Self {
            role: ChatRole::Tool,
            body: TurnBody::Temp(calls),
            timestamp: String::new(),
            content_md: None,
        }
    }

    /// Ensure the markdown cache is up to date with the raw text content.
    pub fn refresh_md_cache(&mut self) {
        if let TurnBody::Text(tc) = &self.body {
            self.content_md = Some(iced::widget::markdown::Content::parse(&tc.content));
        }
    }
}

impl Clone for Turn {
    fn clone(&self) -> Self {
        let mut cloned = Self {
            role: self.role.clone(),
            body: self.body.clone(),
            timestamp: self.timestamp.clone(),
            content_md: None,
        };
        cloned.refresh_md_cache();
        cloned
    }
}

/// Static emoji replacer — compiled once and reused.
static EMOJI: LazyLock<Replacer> = LazyLock::new(Replacer::new);

/// Replace GitHub-flavored `:emoji:` codes with Unicode emoji in text.
/// Use markdown parser to skip inline code and fenced code blocks.
pub fn replace_emoji(text: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    // Collect byte ranges covered by code regions.
    let mut code_ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut block_start: Option<usize> = None;

    let parser = Parser::new(text).into_offset_iter();
    for (event, range) in parser {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                block_start = Some(range.start);
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = block_start.take() {
                    code_ranges.push(start..range.end);
                }
            }
            Event::Code(_) => {
                code_ranges.push(range);
            }
            _ => {}
        }
    }
    // Unclosed code block — extend to end of text.
    if let Some(start) = block_start.take() {
        code_ranges.push(start..text.len());
    }

    // Apply emoji replacement only to regions outside code ranges.
    let mut result = String::with_capacity(text.len());
    let mut pos = 0;
    for range in &code_ranges {
        if pos < range.start {
            result.push_str(&EMOJI.replace_all(&text[pos..range.start]));
        }
        result.push_str(&text[range.start..range.end]);
        pos = range.end;
    }
    if pos < text.len() {
        result.push_str(&EMOJI.replace_all(&text[pos..]));
    }

    result
}
