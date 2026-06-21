use indexmap::IndexMap;

use iced::{
    Element,
    widget::{checkbox, column, row},
};

use crate::{Message, tools::DevTool};

// ── view ───────────────────────────────────────────────────────────

pub fn dev_tools_view<'a>(selected: &'a IndexMap<DevTool, bool>) -> Element<'a, Message> {
    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    for chunk in DevTool::ALL.chunks(3) {
        let mut row_children = Vec::new();
        for tool in chunk {
            let tool_name = tool.name().to_string();
            let is_checked = selected.get(tool).copied().unwrap_or(false);
            row_children.push(
                checkbox(is_checked)
                    .label(tool_name)
                    .style(crate::primary_checkbox)
                    .on_toggle(move |v| Message::ToggleDevTool(tool.name().to_string(), v))
                    .into(),
            );
        }
        rows.push(row(row_children).spacing(4).into());
    }
    column(rows).spacing(4).into()
}

/// Generate a human-readable summary of enabled dev tools.
pub fn tools_summary(selected: &IndexMap<DevTool, bool>) -> String {
    let mut result = String::new();
    result.push_str("Available tools:\n");

    for (tool, &enabled) in selected {
        if enabled {
            result.push_str(&format!("- {}: {}\n", tool.name(), tool.description()));
        }
    }

    result.push_str("You may have access to additional custom tools.\n");
    result
}
