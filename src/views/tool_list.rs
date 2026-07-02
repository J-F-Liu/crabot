use std::collections::HashSet;

use iced::{
    Element, Length, padding,
    widget::{Space, checkbox, column, row},
};

use super::styles::label;
use crate::Message;

/// A labelled section of tool checkboxes (e.g. "Builtin Tools", "Custom Tools").
pub(crate) fn tools_section<'a>(
    title: &'a str,
    selected: &'a HashSet<String>,
    names: &'a [String],
) -> Element<'a, Message> {
    column![label(title, 140.0), tools_view(selected, names)]
        .spacing(4)
        .into()
}

pub(crate) fn tools_view<'a>(
    selected: &'a HashSet<String>,
    names: &'a [String],
) -> Element<'a, Message> {
    const COLS: usize = 3;

    let rows: Vec<Element<'a, Message>> = names
        .chunks(COLS)
        .map(|chunk| {
            let mut cells: Vec<Element<'a, Message>> = chunk
                .iter()
                .map(|name| checkbox_cell(name, selected))
                .collect();

            // Fill remaining columns so every cell in a column has the same width.
            for _ in cells.len()..COLS {
                cells.push(Space::new().width(Length::Fill).into());
            }

            row(cells).spacing(12).into()
        })
        .collect();

    column(rows)
        .spacing(4)
        .padding(padding::left(8))
        .max_width(400)
        .into()
}

fn checkbox_cell<'a>(name: &'a str, selected: &'a HashSet<String>) -> Element<'a, Message> {
    let checked = selected.contains(name);
    Element::from(
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleAgentTool(name.to_string(), v))
            .width(Length::Fill),
    )
}
