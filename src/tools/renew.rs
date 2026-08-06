use std::path::Path;
use std::sync::atomic::AtomicBool;

use genai::chat::ToolCall;
use serde_json::{Value, json};

use super::Tool;

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
        "Create a new session with a condensed version of the current task. Use this tool when the context window is nearly full and the task is not yet complete. The prompt should restate original goal as title, summarize what has been accomplished so far and describe what still needs to be done."
    }

    fn instruction(&self) -> &str {
        "When user says that context fill ratio is near its limit and the current task cannot be completed soon, call the renew tool. If todo was called and items status is outdated, call todo to update status along with the renew call. renew can only be called at most once."
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
        _cancel: &AtomicBool,
    ) -> Result<String, String> {
        Err("renew must be handled by the user interface".into())
    }
}
