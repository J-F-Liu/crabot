use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use super::Tool;

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
        _cancel: &AtomicBool,
    ) -> Result<String, String> {
        Err("ask must be handled by the user interface".into())
    }
}
