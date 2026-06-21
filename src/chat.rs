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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayMessage {
    pub role: ChatRole,
    pub content: MessageContent,
    pub timestamp: String,
}

impl DisplayMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: MessageContent::Text(TextContent {
                content: content.into(),
                reasoning: None,
            }),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }

    pub fn assistant(content: impl Into<String>, reasoning: Option<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: MessageContent::Text(TextContent {
                content: content.into(),
                reasoning,
            }),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }

    pub fn from_tool_result(tr: ToolResult) -> Self {
        Self {
            role: ChatRole::Tool,
            content: MessageContent::Tool(tr),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }
}
