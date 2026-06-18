use std::collections::HashMap;
use std::fmt::Write;

use iced::{
    Element,
    widget::{checkbox, column, row},
};
use serde_json::{Map, Value};

use crate::Message;

// ── DevTools ────────────────────────────────────────────────────────

/// The six coding-agent devtools exposed to the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DevTool {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Bash,
}

impl DevTool {
    pub const ALL: &[DevTool] = &[
        Self::Read,
        Self::Write,
        Self::Edit,
        Self::Glob,
        Self::Grep,
        Self::Bash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Glob => "glob",
            Self::Grep => "grep",
            Self::Bash => "bash",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Read => "Read a file from the filesystem.",
            Self::Write => "Write content to a file.",
            Self::Edit => "Replace an exact string in a file with another.",
            Self::Glob => "Find files matching a glob pattern.",
            Self::Grep => "Search for a regular expression in files.",
            Self::Bash => "Execute a shell command.",
        }
    }

    /// Returns the JSON Schema for this tool's parameters.
    fn schema(self) -> Value {
        match self {
            Self::Read => serde_json::from_value(Value::Object({
                let mut m = Map::new();
                m.insert("type".into(), "object".into());
                let mut props = Map::new();
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert("description".into(), "Path to the file".into());
                    props.insert("path".into(), Value::Object(p));
                }
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "integer".into());
                    p.insert(
                        "description".into(),
                        "0-based line offset to start reading from".into(),
                    );
                    props.insert("offset".into(), Value::Object(p));
                }
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "integer".into());
                    p.insert("description".into(), "Maximum lines to read".into());
                    props.insert("limit".into(), Value::Object(p));
                }
                m.insert("properties".into(), Value::Object(props));
                let required: Vec<Value> = vec!["path".into()];
                m.insert("required".into(), Value::Array(required));
                m
            }))
            .unwrap(),
            Self::Write => serde_json::from_value(Value::Object({
                let mut m = Map::new();
                m.insert("type".into(), "object".into());
                let mut props = Map::new();
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert("description".into(), "Path to the file".into());
                    props.insert("path".into(), Value::Object(p));
                }
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert("description".into(), "Full content to write".into());
                    props.insert("content".into(), Value::Object(p));
                }
                m.insert("properties".into(), Value::Object(props));
                let required: Vec<Value> = vec!["path".into(), "content".into()];
                m.insert("required".into(), Value::Array(required));
                m
            }))
            .unwrap(),
            Self::Edit => serde_json::from_value(Value::Object({
                let mut m = Map::new();
                m.insert("type".into(), "object".into());
                let mut props = Map::new();
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert("description".into(), "Path to the file".into());
                    props.insert("path".into(), Value::Object(p));
                }
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert("description".into(), "Exact text to find".into());
                    props.insert("old_string".into(), Value::Object(p));
                }
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert("description".into(), "Replacement text".into());
                    props.insert("new_string".into(), Value::Object(p));
                }
                m.insert("properties".into(), Value::Object(props));
                let required: Vec<Value> =
                    vec!["path".into(), "old_string".into(), "new_string".into()];
                m.insert("required".into(), Value::Array(required));
                m
            }))
            .unwrap(),
            Self::Glob => serde_json::from_value(Value::Object({
                let mut m = Map::new();
                m.insert("type".into(), "object".into());
                let mut props = Map::new();
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert(
                        "description".into(),
                        "Glob pattern (e.g. \"*.rs\", \"**/*.ts\")".into(),
                    );
                    props.insert("pattern".into(), Value::Object(p));
                }
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert(
                        "description".into(),
                        "Directory to search (default \".\")".into(),
                    );
                    props.insert("path".into(), Value::Object(p));
                }
                m.insert("properties".into(), Value::Object(props));
                let required: Vec<Value> = vec!["pattern".into()];
                m.insert("required".into(), Value::Array(required));
                m
            }))
            .unwrap(),
            Self::Grep => serde_json::from_value(Value::Object({
                let mut m = Map::new();
                m.insert("type".into(), "object".into());
                let mut props = Map::new();
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert(
                        "description".into(),
                        "Regular expression (RE2 syntax)".into(),
                    );
                    props.insert("pattern".into(), Value::Object(p));
                }
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert(
                        "description".into(),
                        "File or directory to search (default \".\")".into(),
                    );
                    props.insert("path".into(), Value::Object(p));
                }
                m.insert("properties".into(), Value::Object(props));
                let required: Vec<Value> = vec!["pattern".into()];
                m.insert("required".into(), Value::Array(required));
                m
            }))
            .unwrap(),
            Self::Bash => serde_json::from_value(Value::Object({
                let mut m = Map::new();
                m.insert("type".into(), "object".into());
                let mut props = Map::new();
                {
                    let mut p = Map::new();
                    p.insert("type".into(), "string".into());
                    p.insert("description".into(), "Shell command to execute".into());
                    props.insert("command".into(), Value::Object(p));
                }
                m.insert("properties".into(), Value::Object(props));
                let required: Vec<Value> = vec!["command".into()];
                m.insert("required".into(), Value::Array(required));
                m
            }))
            .unwrap(),
        }
    }

    /// Full tool declaration suitable for `LlmRequest.tools`.
    pub fn tool_declaration(self) -> (String, Value) {
        let name = self.name().to_string();
        let mut m = Map::new();
        m.insert(
            "description".into(),
            Value::String(self.description().to_string()),
        );
        m.insert("parameters".into(), self.schema());
        (name, Value::Object(m))
    }

    /// Build the `tools` map for `LlmRequest` from selected tools.
    pub fn build_tools_map(selected: &HashMap<DevTool, bool>) -> HashMap<String, Value> {
        let mut tools = HashMap::new();
        for (tool, enabled) in selected {
            if *enabled {
                let (name, decl) = tool.tool_declaration();
                tools.insert(name, decl);
            }
        }
        tools
    }
}

// ── view ───────────────────────────────────────────────────────────
pub fn dev_tools_view<'a>(selected: &'a HashMap<DevTool, bool>) -> Element<'a, Message> {
    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    for chunk in DevTool::ALL.chunks(3) {
        let mut row_children = Vec::new();
        for tool in chunk {
            let tool_name = tool.name().to_string();
            let is_checked = selected.get(tool).copied().unwrap_or(false);
            row_children.push(
                checkbox(is_checked)
                    .label(tool_name)
                    .on_toggle(move |v| Message::ToggleDevTool(tool.name().to_string(), v))
                    .into(),
            );
        }
        rows.push(row(row_children).spacing(4).into());
    }
    column(rows).spacing(4).into()
}

/// Generate a human-readable summary of enabled dev tools.
pub fn tools_summary(selected: &HashMap<DevTool, bool>) -> String {
    let mut result = String::new();
    result.push_str("Available tools:\n");

    for (tool, &enabled) in selected {
        if enabled {
            let _ = write!(result, "- {}: {}\n", tool.name(), tool.description());
        }
    }

    result.push_str("You may have access to additional custom tools.");
    result
}
