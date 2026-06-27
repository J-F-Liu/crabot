use indexmap::IndexMap;

use iced::{
    Element,
    widget::{checkbox, table},
};

use crate::{Message, tools::DevTool};

pub(crate) fn dev_tools_view<'a>(selected: &'a IndexMap<DevTool, bool>) -> Element<'a, Message> {
    type Row = (DevTool, DevTool, DevTool);

    let rows: [Row; 1] = [(DevTool::Find, DevTool::Search, DevTool::Bash)];

    let header = |tool: DevTool| -> Element<'_, Message> {
        let checked = selected.get(&tool).copied().unwrap_or(false);
        Element::from(
            checkbox(checked)
                .label(tool.name())
                .style(crate::views::primary_checkbox)
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
                        .style(crate::views::primary_checkbox)
                        .on_toggle(move |v| Message::ToggleDevTool(t.name().to_string(), v)),
                )
            }),
            table::column(header(DevTool::Write), |(_, t, _): Row| {
                let checked = selected.get(&t).copied().unwrap_or(false);
                Element::from(
                    checkbox(checked)
                        .label(t.name())
                        .style(crate::views::primary_checkbox)
                        .on_toggle(move |v| Message::ToggleDevTool(t.name().to_string(), v)),
                )
            }),
            table::column(header(DevTool::Edit), |(_, _, t): Row| {
                let checked = selected.get(&t).copied().unwrap_or(false);
                Element::from(
                    checkbox(checked)
                        .label(t.name())
                        .style(crate::views::primary_checkbox)
                        .on_toggle(move |v| Message::ToggleDevTool(t.name().to_string(), v)),
                )
            }),
        ],
        rows,
    )
    .separator(0)
    .into()
}
