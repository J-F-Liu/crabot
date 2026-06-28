use std::sync::LazyLock;

use genai::chat::ChatRole;
use gh_emoji::Replacer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Static emoji replacer — compiled once and reused.
static EMOJI: LazyLock<Replacer> = LazyLock::new(Replacer::new);

/// Replace GitHub-flavored `:emoji:` codes with Unicode emoji in text.
pub fn replace_emoji(text: &str) -> String {
    EMOJI.replace_all(text).into()
}

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

// ── TurnBody ────────────────────────────────────────────────────────

/// Body of a single turn in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TurnBody {
    /// Plain-text message (User or Assistant role).
    Text(TextContent),
    /// Paired tool call and its result.
    Tool(ToolResult),
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

    pub fn from_tool_result(tr: ToolResult) -> Self {
        Self {
            role: ChatRole::Tool,
            body: TurnBody::Tool(tr),
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
