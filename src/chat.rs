use serde::{Deserialize, Serialize};

// ── Role ──────────────────────────────────────────────────────────────

/// The role of a message in the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    #[serde(rename = "You")]
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Assistant",
            Self::Tool => "Tool",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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

// ── ChatMessage ──────────────────────────────────────────────────────

/// A single message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: MessageContent,
    pub timestamp: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text {
                content: content.into(),
                reasoning: None,
            },
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }

    pub fn assistant(content: impl Into<String>, reasoning: Option<String>) -> Self {
        Self {
            role: Role::Assistant,
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
            role: Role::Tool,
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
