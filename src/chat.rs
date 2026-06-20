use genai::chat::ChatRole;
use serde::{Deserialize, Serialize};

// ── MessageContent ────────────────────────────────────────────────────

/// The actual content of a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageContent {
    /// Plain-text message (User or Assistant role).
    Text {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
    },
    /// Paired tool call and its result.
    Tool {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        /// Prettified JSON of the call arguments.
        args: String,
        /// Execution result text.
        result: String,
    },
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
            content: MessageContent::Text {
                content: content.into(),
                reasoning: None,
            },
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }

    pub fn assistant(content: impl Into<String>, reasoning: Option<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: MessageContent::Text {
                content: content.into(),
                reasoning,
            },
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }

    pub fn tool(
        name: impl Into<String>,
        args: &serde_json::Value,
        id: Option<String>,
        result: impl Into<String>,
    ) -> Self {
        let args_str = serde_json::to_string_pretty(args).unwrap_or_else(|_| format!("{args:?}"));
        Self {
            role: ChatRole::Tool,
            content: MessageContent::Tool {
                name: name.into(),
                call_id: id,
                args: args_str,
                result: result.into(),
            },
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }
}
