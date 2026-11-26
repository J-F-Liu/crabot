use std::path::Path;
use tokio_util::sync::CancellationToken;

use serde_json::{Value, json};

use crate::model::TaskTier;
use crate::tools::Tool;

/// Request emitted by the builtin task tool to spawn a sub-agent session.
#[derive(Debug, Clone)]
pub struct TaskRequest {
    /// Originating tool call_id — the sub-agent's report is tagged with it.
    pub call_id: String,
    /// Short tab title; falls back to a prompt-derived title when absent.
    pub title: Option<String>,
    /// Self-contained instructions for the sub-agent (it never sees the parent conversation).
    pub prompt: String,
    /// Execution mode — selects the sub-agent's preamble (`{mode}.md`).
    pub mode: Option<String>,
    /// Difficulty tier — selects the sub-agent's configured model.
    pub difficulty: Option<TaskTier>,
}

/// Parse a task-tool call into its spawn request payload.
pub fn task_request_from_call(call_id: &str, fn_arguments: &Value) -> TaskRequest {
    let arg = |key: &str| fn_arguments.get(key).and_then(Value::as_str);
    TaskRequest {
        call_id: call_id.to_string(),
        title: arg("title").map(str::to_owned),
        prompt: arg("prompt").unwrap_or_default().to_owned(),
        mode: arg("mode").map(str::to_owned),
        difficulty: arg("difficulty").and_then(TaskTier::from_name),
    }
}

/// Execution modes the `task` tool accepts; each maps to `preamble/{mode}.md`.
pub const TASK_MODES: &[&str] = &["explore", "planning", "coding", "review", "testing"];

/// Delegates a subtask to a new isolated agent session running in its own tab.
/// The sub-agent works autonomously; its final report becomes this tool's result.
pub struct TaskTool;

impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Delegate a self-contained subtask to an isolated agent instead of doing everything yourself. Include all context in the prompt; the sub-agent cannot see this conversation. `mode` selects its behavior preamble; `difficulty` (`easy`/`hard`) selects the configured model. Blocks until completion and returns the final report. Sub-agents may nest tasks; nested calls behave the same."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short title of the task."
                },
                "prompt": {
                    "type": "string",
                    "description": "Detailed instructions and expected outcome for the sub-agent."
                },
                "mode": {
                    "type": "string",
                    "enum": TASK_MODES,
                    "description": "Execution mode that determines the sub-agent behavior and available tools."
                },
                "difficulty": {
                    "type": "string",
                    "enum": TaskTier::ALL.map(TaskTier::name),
                    "description": "Task difficulty: easy for quick, well-scoped subtasks; hard for complex ones."
                }
            },
            "required": ["prompt"]
        })
    }

    /// This is intercepted by the streaming engine (`llm::send_stream`) before
    /// execution and routed to the UI via [`SessionEvent::TaskRequest`].
    fn execute_inner(
        &self,
        _args: &Value,
        _workspace: &Path,
        _cancel: &CancellationToken,
    ) -> Result<String, String> {
        Err("task must be handled by the user interface".into())
    }
}
