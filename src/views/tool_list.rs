use std::collections::HashSet;

use iced::{
    Alignment, Element, Length, padding,
    widget::{Space, checkbox, column, container, mouse_area, row, text},
};

use crate::Message;

pub const BUILTIN_TOOLS: &str = "Builtin Tools";
pub const CUSTOM_TOOLS: &str = "Custom Tools";

/// Collapse/expand state for the tools sections in the left pane.
#[derive(Debug, Clone)]
pub(crate) struct ToolListState {
    pub builtin_expanded: bool,
    pub custom_expanded: bool,
}

impl Default for ToolListState {
    fn default() -> Self {
        Self {
            builtin_expanded: true,
            custom_expanded: true,
        }
    }
}

impl ToolListState {
    /// Handle a `ToggleExpanded` message for tool-list section titles.
    pub(crate) fn update(&mut self, name: &str) -> bool {
        match name {
            BUILTIN_TOOLS => {
                self.builtin_expanded = !self.builtin_expanded;
                true
            }
            CUSTOM_TOOLS => {
                self.custom_expanded = !self.custom_expanded;
                true
            }
            _ => false,
        }
    }
}

/// A labelled section of tool checkboxes (e.g. "Builtin Tools", "Custom Tools").
pub(crate) fn tools_section<'a>(
    title: &'static str,
    expanded: bool,
    selected: &'a HashSet<String>,
    names: &'a [String],
) -> Element<'a, Message> {
    if names.is_empty() {
        return column![].into();
    }

    let arrow = if expanded { "▼" } else { "⯈" };
    let header = mouse_area(
        row![
            text(title).size(14),
            Space::new().width(Length::Fill),
            text(arrow).size(12),
        ]
        .align_y(Alignment::Center),
    )
    .on_press(Message::ToggleExpanded(title));

    if expanded {
        column![header, tools_view(selected, names)]
            .spacing(4)
            .into()
    } else {
        column![header].into()
    }
}

pub(crate) fn tools_view<'a>(
    selected: &'a HashSet<String>,
    names: &'a [String],
) -> Element<'a, Message> {
    const COLS: usize = 3;

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
