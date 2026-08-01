use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use super::Tool;

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
        "Delegate a task to a new isolated agent session."
    }

    fn instruction(&self) -> &str {
        "Use the task tool to delegate self-contained subtasks to a sub-agent instead of doing everything yourself. The prompt must be complete and self-contained — the sub-agent does not see this conversation. The mode selects a behavior preamble for the sub-agent, the difficulty selects the configured subtask model. The call blocks until the sub-agent finishes and returns its final report as the tool result. Sub-agents can spawn further nested subtasks — each call blocks until that nested sub-agent finishes and returns its report."
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
                    "enum": ["easy", "medium", "hard"],
                    "description": "Estimated task difficulty used for resource and model selection."
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
        _cancel: &AtomicBool,
    ) -> Result<String, String> {
        Err("task must be handled by the user interface".into())
    }
}
