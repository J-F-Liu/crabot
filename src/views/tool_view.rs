//! Renderer-agnostic model of a tool call's arguments, shared by the live
//! center pane ([`super::tool_message`]) and the HTML export
//! ([`super::export`]) so the two never drift apart.

use crabot::tools::arg_path;
use crabot::tools::edit::EditParam;
use crabot::tools::todo::{TodoItem, TodoStatus};
use serde_json::Value;

/// One entry of an `edits` argument: parsed old/new text, or the raw JSON when
/// the entry does not deserialize as an [`EditParam`].
#[derive(Debug)]
pub enum EditRow {
    Edit { old_text: String, new_text: String },
    Invalid(String),
}

/// Language-agnostic status of a `todo` item; renderers supply colour/class.
#[derive(Debug, Clone, Copy)]
pub enum TodoState {
    Pending,
    InProgress,
    Completed,
    Invalid,
}

impl TodoState {
    /// i18n key for the status label, fed to `lang.tr`.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in progress",
            Self::Completed => "completed",
            Self::Invalid => "⚠ invalid",
        }
    }
}

/// One `todo` item; `content` already folds the depth into leading spaces.
#[derive(Debug)]
pub struct TodoRow {
    pub content: String,
    pub state: TodoState,
}

/// Argument name of the `edit` tool's edit list.
pub const EDITS_ARG: &str = "edits";

/// One renderable argument row of a tool call.
#[derive(Debug)]
pub enum ArgRow {
    /// A plain `key: value` row.
    Text { key: String, value: String },
    /// The `read` tool's `offset` + `limit` pair as a single row.
    OffsetLimit { offset: String, limit: String },
    /// The `edit` tool's `edits` array, rendered as a list of diffs.
    Edits(Vec<EditRow>),
    /// The `todo` tool's `items` array, rendered as a table.
    Todo(Vec<TodoRow>),
}

/// Rows for an expanded tool call: `todo`'s `items` becomes a table, an
/// `edits` array a diff list, and `offset`+`limit` one combined row.
pub fn arg_rows(tool_name: &str, args: &Value) -> Vec<ArgRow> {
    let Some(map) = args.as_object() else {
        return Vec::new();
    };

    let mut rows = Vec::new();

    let todo_items = map
        .get("items")
        .and_then(Value::as_array)
        .filter(|_| tool_name == "todo");
    if let Some(items) = todo_items {
        rows.push(ArgRow::Todo(todo_rows(items)));
    }

    let combined = map
        .get("offset")
        .zip(map.get("limit"))
        .filter(|_| tool_name == "read");
    if let Some((offset, limit)) = combined {
        rows.push(ArgRow::OffsetLimit {
            offset: value_text(offset),
            limit: value_text(limit),
        });
    }

    for (key, value) in map {
        // Keys already rendered as a dedicated row above.
        let consumed = match key.as_str() {
            "offset" | "limit" => combined.is_some(),
            "items" => todo_items.is_some(),
            _ => false,
        };
        if consumed {
            continue;
        }
        if tool_name == "edit"
            && key == EDITS_ARG
            && let Some(edits) = value.as_array()
        {
            rows.push(ArgRow::Edits(edit_rows(edits)));
            continue;
        }
        rows.push(ArgRow::Text {
            key: key.clone(),
            value: value_text(value),
        });
    }
    rows
}

/// Rows for a collapsed preview: the modified path alone for `edit`/`write`
/// (alias-aware, like the tools), the expanded rows for everything else.
pub fn preview_rows(tool_name: &str, args: &Value) -> Vec<ArgRow> {
    if matches!(tool_name, "edit" | "write") {
        return arg_path(args)
            .map(|path| ArgRow::Text {
                key: "path".to_string(),
                value: path.to_string(),
            })
            .into_iter()
            .collect();
    }
    arg_rows(tool_name, args)
}

/// Whether the tool renders its own view instead of the generic args/result
/// detail (`ask` shows an interactive prompt).
pub fn renders_own_view(tool_name: &str) -> bool {
    tool_name == "ask"
}

/// String form of a JSON argument value: the bare string, else its JSON text.
fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn edit_rows(edits: &[Value]) -> Vec<EditRow> {
    edits
        .iter()
        .map(
            |edit| match serde_json::from_value::<EditParam>(edit.clone()) {
                Ok(EditParam { old_text, new_text }) => EditRow::Edit { old_text, new_text },
                Err(_) => EditRow::Invalid(edit.to_string()),
            },
        )
        .collect()
}

fn todo_rows(items: &[Value]) -> Vec<TodoRow> {
    items
        .iter()
        .map(
            |item| match serde_json::from_value::<TodoItem>(item.clone()) {
                Ok(todo) => TodoRow {
                    content: format!("{}{}", "  ".repeat(todo.depth as usize), todo.text),
                    state: match todo.status {
                        TodoStatus::Pending => TodoState::Pending,
                        TodoStatus::InProgress => TodoState::InProgress,
                        TodoStatus::Completed => TodoState::Completed,
                    },
                },
                Err(_) => TodoRow {
                    content: item.to_string(),
                    state: TodoState::Invalid,
                },
            },
        )
        .collect()
}
