use std::collections::HashSet;

use iced::{
    Alignment, Element, Length, padding,
    widget::{Space, checkbox, column, container, mouse_area, row, text, text::Wrapping},
};

use crate::Message;
use crate::tools::mcp::McpTool;

pub const BUILTIN_TOOLS: &str = "Builtin Tools";
pub const CUSTOM_TOOLS: &str = "Custom Tools";
pub const MCP_TOOLS: &str = "MCP Tools";

/// Collapse/expand state for the tools sections in the left pane.
#[derive(Debug, Clone)]
pub(crate) struct ToolListState {
    pub builtin_expanded: bool,
    pub custom_expanded: bool,
    pub mcp_expanded: bool,
}

impl Default for ToolListState {
    fn default() -> Self {
        Self {
            builtin_expanded: true,
            custom_expanded: true,
            mcp_expanded: true,
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
            MCP_TOOLS => {
                self.mcp_expanded = !self.mcp_expanded;
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

    // Distribute names into columns (row-major: fill across, then down).
    let n_rows = names.len().div_ceil(COLS);
    let mut cols: Vec<Vec<&str>> = (0..COLS).map(|_| Vec::with_capacity(n_rows)).collect();
    for (i, name) in names.iter().enumerate() {
        let col = i % COLS;
        cols[col].push(name.as_str());
    }

    // Build actual iced columns: each column naturally sizes to its widest
    // checkbox, giving pixel-perfect alignment without width estimation.
    let cols: Vec<Element<'a, Message>> = cols
        .into_iter()
        .map(|names| {
            let checkboxes: Vec<Element<'a, Message>> = names
                .into_iter()
                .map(|name| checkbox_cell(name, None, selected))
                .collect();
            column(checkboxes).spacing(4).into()
        })
        .collect();

    container(row(cols).spacing(12))
        .padding(padding::left(8))
        .width(Length::Fill)
        .into()
}

/// A labelled section for MCP tools, with server sub-groups nested under a
/// single collapsible "MCP Tools" header.
pub(crate) fn mcp_tools_section<'a>(
    expanded: bool,
    selected: &'a HashSet<String>,
    groups: &'a [(String, Vec<McpTool>)],
) -> Element<'a, Message> {
    if groups.is_empty() {
        return column![].into();
    }

    let arrow = if expanded { "▼" } else { "⯈" };
    let header = mouse_area(
        row![
            text(MCP_TOOLS).size(14),
            Space::new().width(Length::Fill),
            text(arrow).size(12),
        ]
        .align_y(Alignment::Center),
    )
    .on_press(Message::ToggleExpanded(MCP_TOOLS));

    if expanded {
        let group_cols: Vec<Element<'a, Message>> = groups
            .iter()
            .map(|(server, tools)| mcp_server_group_view(server, selected, tools))
            .collect();
        column![
            header,
            column(group_cols).spacing(4).padding(padding::left(4))
        ]
        .spacing(4)
        .into()
    } else {
        column![header].into()
    }
}

fn mcp_server_group_view<'a>(
    server: &'a str,
    selected: &'a HashSet<String>,
    tools: &'a [McpTool],
) -> Element<'a, Message> {
    if tools.is_empty() {
        return column![].into();
    }
    let label = text(server).size(13).style(|_theme| text::Style {
        color: Some(crate::views::theme::CRABOT_TEXT_MUTED),
    });
    let checkboxes = mcp_tools_view(selected, tools);
    column![label, checkboxes].spacing(2).into()
}

fn mcp_tools_view<'a>(selected: &'a HashSet<String>, tools: &'a [McpTool]) -> Element<'a, Message> {
    const COLS: usize = 3;

    let n_rows = tools.len().div_ceil(COLS);
    let mut cols: Vec<Vec<&McpTool>> = (0..COLS).map(|_| Vec::with_capacity(n_rows)).collect();
    for (i, tool) in tools.iter().enumerate() {
        let col = i % COLS;
        cols[col].push(tool);
    }

    let cols: Vec<Element<'a, Message>> = cols
        .into_iter()
        .map(|tools| {
            let checkboxes: Vec<Element<'a, Message>> = tools
                .into_iter()
                .map(|tool| checkbox_cell(&tool.name, tool.title.as_deref(), selected))
                .collect();
            column(checkboxes).spacing(4).into()
        })
        .collect();

    container(row(cols).spacing(12))
        .padding(padding::left(8))
        .width(Length::Fill)
        .into()
}

fn checkbox_cell<'a>(
    name: &'a str,
    title: Option<&'a str>,
    selected: &'a HashSet<String>,
) -> Element<'a, Message> {
    let checked = selected.contains(name);
    let label = title.unwrap_or(name);
    Element::from(
        checkbox(checked)
            .label(label)
            .style(crate::views::primary_checkbox)
            .text_wrapping(Wrapping::None)
            .on_toggle(move |v| Message::ToggleAgentTool(name.to_string(), v)),
    )
}
