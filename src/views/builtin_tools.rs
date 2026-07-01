use std::collections::HashSet;

use iced::{
    Element,
    widget::{checkbox, table},
};

use crate::{Message, tools};

pub(crate) fn builtin_tools_view<'a>(selected: &'a HashSet<String>) -> Element<'a, Message> {
    type Row = (&'static str, &'static str, &'static str);

    let names: Vec<&'static str> = tools::builtin_tools().keys().copied().collect();
    let rows: [Row; 1] = [(names[3], names[4], names[5])];

    let header = |name: &'static str| checkbox_cell(name, selected);

    table(
        vec![
            table::column(header(names[0]), |(t, _, _): Row| {
                checkbox_cell(t, selected)
            }),
            table::column(header(names[1]), |(_, t, _): Row| {
                checkbox_cell(t, selected)
            }),
            table::column(header(names[2]), |(_, _, t): Row| {
                checkbox_cell(t, selected)
            }),
        ],
        rows,
    )
    .separator(0)
    .into()
}

fn checkbox_cell<'a>(name: &'static str, selected: &'a HashSet<String>) -> Element<'a, Message> {
    let checked = selected.contains(name);
    Element::from(
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleAgentTool(name.to_string(), v)),
    )
}
