use std::path::Path;
use tokio_util::sync::CancellationToken;

use genai::chat::ToolCall;
use serde_json::{Value, json};

use crate::tools::Tool;

/// Move renew calls to the end so the turn's other tools run first.
/// Only the first renew takes effect; later renews are reported as errors.
pub fn move_renews_to_end(calls: &mut Vec<ToolCall>) {
    let (mut renews, others): (Vec<_>, Vec<_>) =
        calls.drain(..).partition(|tc| tc.fn_name == "renew");
    *calls = others;
    calls.append(&mut renews);
}

/// Triggers creation of a new session when the context window is nearly full.
/// The prompt string describes the remaining task so the new session can continue.
pub struct RenewTool;

impl Tool for RenewTool {
    fn name(&self) -> &str {
        "renew"
    }

    fn description(&self) -> &str {
        "When context is nearly full and work will not finish soon, create a session with a condensed task prompt: title the goal, summarize completed work, and list remaining work. Update a stale `todo` list in the same response. Call renew at most once."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "A condensed summary describing the goal, what has been accomplished and what still needs to be done, so the new session can continue the task." }
            },
            "required": ["prompt"]
        })
    }

    /// This is intercepted by the streaming engine (`llm::send_stream`) before
    /// execution and routed to the UI via [`SessionEvent::RenewRequest`].
    fn execute_inner(
        &self,
        _args: &Value,
        _workspace: &Path,
        _cancel: &CancellationToken,
    ) -> Result<String, String> {
        Err("renew must be handled by the user interface".into())
    }
}
