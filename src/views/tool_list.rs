use std::collections::HashSet;

use iced::{
    Alignment, Element, Length, padding,
    widget::{Space, checkbox, column, container, mouse_area, row, text, text::Wrapping},
};

use crate::tools::mcp::McpTool;

pub const BUILTIN_TOOLS: &str = "Builtin Tools";
pub const CUSTOM_TOOLS: &str = "Custom Tools";
pub const MCP_TOOLS: &str = "MCP Tools";

/// Events emitted by the tool-list views in the left pane.
///
/// Callers should map these to their owning domain events:
/// - `ExpandSection` → [`crate::PromptEvent::ToggleExpanded`]
/// - `ToggleMcpServer` / `ToggleAgentTool` → [`crate::ToolEvent`] variants
#[derive(Clone)]
pub(crate) enum ToolListEvent {
    ExpandSection(&'static str),
    ToggleMcpServer(String, bool),
    ToggleAgentTool(String, bool),
}

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
    pub(crate) fn update(&mut self, name: &str) {
        match name {
            BUILTIN_TOOLS => {
                self.builtin_expanded = !self.builtin_expanded;
            }
            CUSTOM_TOOLS => {
                self.custom_expanded = !self.custom_expanded;
            }
            MCP_TOOLS => {
                self.mcp_expanded = !self.mcp_expanded;
            }
            _ => {}
        }
    }
}

/// Clickable header row for a collapsible section.
fn section_header<'a>(title: &'static str, expanded: bool) -> Element<'a, ToolListEvent> {
    let arrow = if expanded { "▼" } else { "⯈" };
    mouse_area(
        row![
            text(title).size(14),
            Space::new().width(Length::Fill),
            text(arrow).size(12),
        ]
        .align_y(Alignment::Center),
    )
    .on_press(ToolListEvent::ExpandSection(title))
    .into()
}

/// A labelled section of tool checkboxes (e.g. "Builtin Tools", "Custom Tools").
pub(crate) fn tools_section<'a>(
    title: &'static str,
    expanded: bool,
    selected: &'a HashSet<String>,
    names: &'a [String],
) -> Element<'a, ToolListEvent> {
    if names.is_empty() {
        return column![].into();
    }

    let header = section_header(title, expanded);
    if expanded {
        column![header, tools_view(selected, names)]
            .spacing(4)
            .into()
    } else {
        column![header].into()
    }
}

/// Number of columns used to lay out tool checkboxes in a grid.
const TOOL_GRID_COLS: usize = 3;

/// Distribute items into `TOOL_GRID_COLS` columns (row-major: fill across, then down).
fn distribute_into_columns<T: Copy>(items: &[T]) -> Vec<Vec<T>> {
    let n_rows = items.len().div_ceil(TOOL_GRID_COLS);
    let mut cols: Vec<Vec<T>> = (0..TOOL_GRID_COLS)
        .map(|_| Vec::with_capacity(n_rows))
        .collect();
    for (i, item) in items.iter().enumerate() {
        cols[i % TOOL_GRID_COLS].push(*item);
    }
    cols
}

/// Wrap pre-built checkbox columns in a spaced, left-padded row.
fn checkbox_grid<'a>(cols: Vec<Element<'a, ToolListEvent>>) -> Element<'a, ToolListEvent> {
    container(row(cols).spacing(12))
        .padding(padding::left(8))
        .width(Length::Fill)
        .into()
}

pub(crate) fn tools_view<'a>(
    selected: &'a HashSet<String>,
    names: &'a [String],
) -> Element<'a, ToolListEvent> {
    // Build actual iced columns: each column naturally sizes to its widest
    // checkbox, giving pixel-perfect alignment without width estimation.
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let cols: Vec<Element<'a, ToolListEvent>> = distribute_into_columns(&name_refs)
        .into_iter()
        .map(|names| {
            let checkboxes: Vec<Element<'a, ToolListEvent>> = names
                .into_iter()
                .map(|name| checkbox_cell(name, None, selected, true))
                .collect();
            column(checkboxes).spacing(4).into()
        })
        .collect();

    checkbox_grid(cols)
}

/// A labelled section for MCP tools, with server sub-groups nested under a
/// single collapsible "MCP Tools" header.
pub(crate) fn mcp_tools_section<'a>(
    expanded: bool,
    selected: &'a HashSet<String>,
    groups: &'a [(String, Vec<McpTool>)],
    enabled_mcp_servers: &'a HashSet<String>,
) -> Element<'a, ToolListEvent> {
    if groups.is_empty() {
        return column![].into();
    }

    let header = section_header(MCP_TOOLS, expanded);
    if expanded {
        let group_cols: Vec<Element<'a, ToolListEvent>> = groups
            .iter()
            .map(|(server, tools)| {
                let enabled = enabled_mcp_servers.contains(server);
                mcp_server_group_view(server, enabled, selected, tools)
            })
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
    enabled: bool,
    selected: &'a HashSet<String>,
    tools: &'a [McpTool],
) -> Element<'a, ToolListEvent> {
    if tools.is_empty() {
        return column![].into();
    }
    let server_cb = checkbox(enabled)
        .label(server)
        .style(crate::views::primary_checkbox)
        .text_wrapping(Wrapping::None)
        .on_toggle(move |v| ToolListEvent::ToggleMcpServer(server.to_string(), v));
    let checkboxes = mcp_tools_view(selected, tools, enabled);
    column![server_cb, checkboxes].spacing(2).into()
}

fn mcp_tools_view<'a>(
    selected: &'a HashSet<String>,
    tools: &'a [McpTool],
    enabled: bool,
) -> Element<'a, ToolListEvent> {
    let tool_refs: Vec<&McpTool> = tools.iter().collect();
    let cols: Vec<Element<'a, ToolListEvent>> = distribute_into_columns(&tool_refs)
        .into_iter()
        .map(|tools| {
            let checkboxes: Vec<Element<'a, ToolListEvent>> = tools
                .into_iter()
                .map(|tool| checkbox_cell(&tool.name, tool.title.as_deref(), selected, enabled))
                .collect();
            column(checkboxes).spacing(4).into()
        })
        .collect();

    checkbox_grid(cols)
}

fn checkbox_cell<'a>(
    name: &'a str,
    title: Option<&'a str>,
    selected: &'a HashSet<String>,
    enabled: bool,
) -> Element<'a, ToolListEvent> {
    let checked = selected.contains(name);
    let label = title.unwrap_or(name);
    let mut cb = checkbox(checked)
        .label(label)
        .style(crate::views::primary_checkbox)
        .text_wrapping(Wrapping::None);
    if enabled {
        cb = cb.on_toggle(move |v| ToolListEvent::ToggleAgentTool(name.to_string(), v));
    }
    Element::from(cb)
}
