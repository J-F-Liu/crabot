use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex, atomic::AtomicBool};

use super::Tool;

/// Shared todo list — the tool writes to it; the UI reads from it.
pub type TodoList = Arc<Mutex<Vec<TodoItem>>>;

#[derive(Debug)]
pub struct TodoTool {
    pub items: TodoList,
}

impl TodoTool {
    pub fn new(items: TodoList) -> Self {
        Self { items }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TodoItem {
    pub text: String,
    pub depth: u8, // 0、1、2
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Create or update the entire todo list in current session."
    }

    fn instruction(&self) -> &str {
        "For complex multi-step tasks, use this tool to maintain the todo list of the session."
    }

    fn schema(&self) -> Value {
        json!({
          "type": "object",
          "properties": {
            "items": {
              "type": "array",
              "description": "Complete replacement of the current todo list. Always send the full list, not only the changed items.",
              "items": {
                "type": "object",
                "properties": {
                  "text": {
                    "type": "string",
                    "description": "Todo item text."
                  },
                  "depth": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2,
                    "description": "Tree depth. 0=root, 1=child, 2=grandchild."
                  },
                  "status": {
                    "type": "string",
                    "enum": [
                      "pending",
                      "in_progress",
                      "completed"
                    ],
                    "description": "Current status of the todo item."
                  }
                },
                "required": [
                  "text",
                  "depth",
                  "status"
                ],
                "additionalProperties": false
              }
            }
          },
          "required": [
            "items"
          ],
          "additionalProperties": false
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        _workspace: &Path,
        _cancel: &AtomicBool,
    ) -> Result<String, String> {
        let items: Vec<TodoItem> =
            serde_json::from_value(args.get("items").cloned().unwrap_or(json!([])))
                .map_err(|e| format!("Invalid todo items: {}", e))?;

        let count = items.len();
        let mut list = self
            .items
            .lock()
            .map_err(|e| format!("Todo list lock poisoned: {}", e))?;
        *list = items;
        Ok(format!("Updated {} todo items.", count))
    }
}
