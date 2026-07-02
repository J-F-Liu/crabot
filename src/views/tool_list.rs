use std::collections::HashSet;

use iced::{
    Element, padding,
    widget::{checkbox, column, container, row},
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

    if names.is_empty() {
        return column![].into();
    }

    // Distribute names into columns (column-major: fill down, then across).
    let n_rows = names.len().div_ceil(COLS);
    let mut cols: Vec<Vec<&str>> = (0..COLS).map(|_| Vec::with_capacity(n_rows)).collect();
    for (i, name) in names.iter().enumerate() {
        let col = i / n_rows;
        cols[col].push(name.as_str());
    }

    // Build actual iced columns: each column naturally sizes to its widest
    // checkbox, giving pixel-perfect alignment without width estimation.
    let cols: Vec<Element<'a, Message>> = cols
        .into_iter()
        .map(|names| {
            let checkboxes: Vec<Element<'a, Message>> = names
                .into_iter()
                .map(|name| checkbox_cell(name, selected))
                .collect();
            column(checkboxes).spacing(4).into()
        })
        .collect();

    container(row(cols).spacing(12))
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
            .on_toggle(move |v| Message::ToggleAgentTool(name.to_string(), v)),
    )
}
