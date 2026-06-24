use indexmap::IndexMap;

use iced::{
    Element,
    widget::{checkbox, table},
};

use crate::{Message, tools::DevTool};

// ── view ───────────────────────────────────────────────────────────

pub fn dev_tools_view<'a>(selected: &'a IndexMap<DevTool, bool>) -> Element<'a, Message> {
    type Row = (DevTool, DevTool, DevTool);

    let rows: [Row; 1] = [(DevTool::Find, DevTool::Search, DevTool::Bash)];

    let header = |tool: DevTool| -> Element<'_, Message> {
        let checked = selected.get(&tool).copied().unwrap_or(false);
        Element::from(
            checkbox(checked)
                .label(tool.name())
                .style(crate::primary_checkbox)
                .on_toggle(move |v| Message::ToggleDevTool(tool.name().to_string(), v)),
        )
    };

    table(
        vec![
            table::column(header(DevTool::Read), |(t, _, _): Row| {
                let checked = selected.get(&t).copied().unwrap_or(false);
                Element::from(
                    checkbox(checked)
                        .label(t.name())
                        .style(crate::primary_checkbox)
                        .on_toggle(move |v| Message::ToggleDevTool(t.name().to_string(), v)),
                )
            }),
            table::column(header(DevTool::Write), |(_, t, _): Row| {
                let checked = selected.get(&t).copied().unwrap_or(false);
                Element::from(
                    checkbox(checked)
                        .label(t.name())
                        .style(crate::primary_checkbox)
                        .on_toggle(move |v| Message::ToggleDevTool(t.name().to_string(), v)),
                )
            }),
            table::column(header(DevTool::Edit), |(_, _, t): Row| {
                let checked = selected.get(&t).copied().unwrap_or(false);
                Element::from(
                    checkbox(checked)
                        .label(t.name())
                        .style(crate::primary_checkbox)
                        .on_toggle(move |v| Message::ToggleDevTool(t.name().to_string(), v)),
                )
            }),
        ],
        rows,
    )
    .separator(0)
    .into()
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
