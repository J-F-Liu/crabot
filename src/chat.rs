use genai::chat::ChatRole;
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
}

// ── MessageContent ───────────────────────────────────────────────────

/// The actual content of a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageContent {
    /// Plain-text message (User or Assistant role).
    Text(TextContent),
    /// Paired tool call and its result.
    Tool(ToolResult),
}

// ── DisplayMessage ──────────────────────────────────────────────────

/// A single message in the conversation history, formatted for UI display.
#[derive(Debug, Serialize, Deserialize)]
pub struct DisplayMessage {
    pub role: ChatRole,
    pub content: MessageContent,
    pub timestamp: String,
    /// Cached parsed Markdown for the text content (if any).
    #[serde(skip)]
    pub content_md: Option<iced::widget::markdown::Content>,
}

impl Clone for DisplayMessage {
    fn clone(&self) -> Self {
        Self {
            role: self.role.clone(),
            content: self.content.clone(),
            timestamp: self.timestamp.clone(),
            content_md: self.content_md.as_ref().map(|_| {
                if let MessageContent::Text(tc) = &self.content {
                    iced::widget::markdown::Content::parse(&tc.content)
                } else {
                    iced::widget::markdown::Content::new()
                }
            }),
        }
    }
}

impl DisplayMessage {
    pub fn user(content: impl Into<String>) -> Self {
        let content_str: String = content.into();
        let content_md = Some(iced::widget::markdown::Content::parse(&content_str));
        Self {
            role: ChatRole::User,
            content: MessageContent::Text(TextContent {
                content: content_str,
                reasoning: None,
            }),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            content_md,
        }
    }

    pub fn assistant(content: impl Into<String>, reasoning: Option<String>) -> Self {
        let content_str: String = content.into();
        let content_md = Some(iced::widget::markdown::Content::parse(&content_str));
        Self {
            role: ChatRole::Assistant,
            content: MessageContent::Text(TextContent {
                content: content_str,
                reasoning,
            }),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            content_md,
        }
    }

    pub fn from_tool_result(tr: ToolResult) -> Self {
        Self {
            role: ChatRole::Tool,
            content: MessageContent::Tool(tr),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            content_md: None,
        }
    }

    /// Ensure the markdown cache is up to date with the raw text content.
    pub fn refresh_md_cache(&mut self) {
        if let MessageContent::Text(tc) = &self.content {
            self.content_md = Some(iced::widget::markdown::Content::parse(&tc.content));
        }
    }
}
