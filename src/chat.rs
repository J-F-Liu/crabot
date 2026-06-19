use serde::{Deserialize, Serialize};

// ── Role ──────────────────────────────────────────────────────────────

/// The role of a message in the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    #[serde(rename = "You")]
    User,
    Assistant,
    ToolCall,
    ToolResult,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Assistant",
            Self::ToolCall => "ToolCall",
            Self::ToolResult => "ToolResult",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── ToolCall ──────────────────────────────────────────────────────────

/// Info about a tool invocation when role is ToolCall or ToolResult.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

// ── ChatMessage ──────────────────────────────────────────────────────

/// A single message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Reasoning / chain-of-thought content (thinking mode).
    pub reasoning: Option<String>,
    pub timestamp: String,
    /// Tool info when role is ToolCall or ToolResult.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolCall>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            reasoning: None,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            tool: None,
        }
    }

    pub fn assistant(content: impl Into<String>, reasoning: Option<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            reasoning,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            tool: None,
        }
    }

    pub fn tool_call(
        name: impl Into<String>,
        args: &serde_json::Value,
        id: Option<String>,
    ) -> Self {
        let args_str = serde_json::to_string_pretty(args).unwrap_or_else(|_| format!("{args:?}"));
        Self {
            role: Role::ToolCall,
            content: args_str,
            reasoning: None,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            tool: Some(ToolCall {
                name: name.into(),
                call_id: id,
            }),
        }
    }

    pub fn tool_result(
        name: impl Into<String>,
        result: impl Into<String>,
        id: Option<String>,
    ) -> Self {
        Self {
            role: Role::ToolResult,
            content: result.into(),
            reasoning: None,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            tool: Some(ToolCall {
                name: name.into(),
                call_id: id,
            }),
        }
    }
}
