use std::path::Path;
use tokio_util::sync::CancellationToken;

use serde_json::{Value, json};

use crate::chat::strip_error_envelope;
use crate::tools::{CANCEL_REASON, Tool};

/// Result content recorded when an ask times out; the model continues without an answer.
pub const TIMEOUT_REASON: &str = "User did not respond before the timeout.";
/// Result content recorded when the stream is cancelled while the ask is pending.
pub const CANCELLED_BY_USER: &str = "Session cancelled by user.";
/// Result content recorded when the ask event channel closes.
pub const EVENT_CHANNEL_CLOSED: &str = "Ask event channel closed.";
/// Result content recorded when the response channel closes.
pub const RESPONSE_CHANNEL_CLOSED: &str = "Response channel closed.";

/// Question text of an ask call; empty when absent.
pub fn ask_question(args: &Value) -> &str {
    args.get("question")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// True when `content` is an ask failure or timeout notice rather than a user answer.
pub fn is_ask_failure(content: &str) -> bool {
    [
        TIMEOUT_REASON,
        CANCELLED_BY_USER,
        EVENT_CHANNEL_CLOSED,
        RESPONSE_CHANNEL_CLOSED,
        CANCEL_REASON,
    ]
    .contains(&strip_error_envelope(content))
}

/// A request for input from the user. The UI completes this tool call.
pub struct AskTool;

impl Tool for AskTool {
    fn name(&self) -> &str {
        "ask"
    }

    fn description(&self) -> &str {
        "Ask the user for clarification or a decision when additional input is required to continue."
    }

    fn instruction(&self) -> &str {
        "Use the ask tool when you need user input before you can continue. When possible, provide concise options so the user can choose quickly. If ask times out, assume the user has no preference, make the best decision and continue if the task can reasonably proceed. Otherwise, stop and explain what information is still required."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The question to ask the user, don't repeat choices in the question." },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of choices for the user. Leave empty when free-form input is expected."
                }
            },
            "required": ["question", "options"]
        })
    }

    /// This is never called during normal operation — the streaming engine
    /// (`llm::send_stream`) intercepts `ask` tool calls before execution and
    /// routes them to the UI via [`SessionEvent::AskRequest`].
    fn execute_inner(
        &self,
        _args: &Value,
        _workspace: &Path,
        _cancel: &CancellationToken,
    ) -> Result<String, String> {
        Err("ask must be handled by the user interface".into())
    }
}
