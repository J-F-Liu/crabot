use std::collections::{HashMap, HashSet};

use iced::{
    Alignment, Element, Length, padding,
    widget::{Space, checkbox, column, container, mouse_area, row, text, text::Wrapping},
};

use crate::tools::mcp::{DiscoverFailure, McpTool};

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
fn section_header<'a>(
    title: &'static str,
    expanded: bool,
    lang: crabot::i18n::Lang,
) -> Element<'a, ToolListEvent> {
    let arrow = if expanded { "▼" } else { "⯈" };
    mouse_area(
        row![
            text(lang.tr(title)).size(14),
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
    lang: crabot::i18n::Lang,
) -> Element<'a, ToolListEvent> {
    if names.is_empty() {
        return column![].into();
    }

    let header = section_header(title, expanded, lang);
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
    // Each column sizes to its widest checkbox, no width estimation needed.
    checkbox_grid_by(names.iter().map(String::as_str).collect(), |name| {
        checkbox_cell(name, None, selected, true)
    })
}

/// Collapsible "MCP Tools" section; failed servers show the reason and
/// retry by re-checking their box (label switches to "Reconnect..." meanwhile).
pub(crate) fn mcp_tools_section<'a>(
    expanded: bool,
    selected: &'a HashSet<String>,
    groups: &'a [(String, Vec<McpTool>)],
    enabled_mcp_servers: &'a HashSet<String>,
    errors: &'a HashMap<String, DiscoverFailure>,
    pending: &'a HashSet<String>,
    lang: crabot::i18n::Lang,
) -> Element<'a, ToolListEvent> {
    let header = section_header(MCP_TOOLS, expanded, lang);
    if !expanded || (groups.is_empty() && errors.is_empty()) {
        return column![header].into();
    }
    let mut rows: Vec<Element<'a, ToolListEvent>> = groups
        .iter()
        .map(|(server, tools)| {
            mcp_server_group_view(
                server,
                Some(tools.as_slice()),
                errors.get(server).copied(),
                pending.contains(server),
                enabled_mcp_servers.contains(server),
                selected,
                lang,
            )
        })
        .collect();
    // Error-only servers have no group yet (e.g. failed at boot).
    let mut missing: Vec<&str> = errors
        .keys()
        .filter(|name| !groups.iter().any(|(g, _)| g == *name))
        .map(String::as_str)
        .collect();
    missing.sort_unstable();
    rows.extend(missing.into_iter().map(|server| {
        mcp_server_group_view(
            server,
            None,
            errors.get(server).copied(),
            pending.contains(server),
            enabled_mcp_servers.contains(server),
            selected,
            lang,
        )
    }));
    column![header, column(rows).spacing(4).padding(padding::left(4))]
        .spacing(4)
        .into()
}

/// One MCP server row: checkbox (annotated on failure) plus its tools, if any.
/// A failed error-only server stays checked while its retry is in flight.
fn mcp_server_group_view<'a>(
    server: &'a str,
    tools: Option<&'a [McpTool]>,
    error: Option<DiscoverFailure>,
    pending: bool,
    enabled: bool,
    selected: &'a HashSet<String>,
    lang: crabot::i18n::Lang,
) -> Element<'a, ToolListEvent> {
    let label = if pending {
        format!("{server} ({})", lang.tr("Reconnect..."))
    } else {
        match error {
            Some(failure) => format!("{server} ({})", failure_label(failure, lang)),
            None => server.to_string(),
        }
    };
    let server_cb = server_checkbox(server, label, enabled);
    match tools {
        Some(tools) => column![server_cb, mcp_tools_view(selected, tools, enabled)]
            .spacing(2)
            .into(),
        None => server_cb.into(),
    }
}

/// Server checkbox toggling an MCP server on/off.
fn server_checkbox(
    server: &str,
    label: String,
    enabled: bool,
) -> iced::widget::Checkbox<'_, ToolListEvent> {
    checkbox(enabled)
        .label(label)
        .style(crate::views::primary_checkbox)
        .text_wrapping(Wrapping::None)
        .on_toggle(move |v| ToolListEvent::ToggleMcpServer(server.to_string(), v))
}

/// Short, translated label for a discovery failure category.
fn failure_label(failure: DiscoverFailure, lang: crabot::i18n::Lang) -> &'static str {
    let key = match failure {
        DiscoverFailure::BadCommand => "command not found",
        DiscoverFailure::Spawn => "failed to launch",
        DiscoverFailure::Connect => "connection failed",
        DiscoverFailure::List => "tool listing failed",
        DiscoverFailure::Empty => "no tools",
        DiscoverFailure::Timeout => "timed out",
    };
    lang.tr(key)
}

fn mcp_tools_view<'a>(
    selected: &'a HashSet<String>,
    tools: &'a [McpTool],
    enabled: bool,
) -> Element<'a, ToolListEvent> {
    checkbox_grid_by(tools.iter().collect(), |tool| {
        checkbox_cell(&tool.name, tool.title.as_deref(), selected, enabled)
    })
}

/// Lay out cells in a 3-column grid.
fn checkbox_grid_by<'a, T: Copy>(
    items: Vec<T>,
    mut cell: impl FnMut(T) -> Element<'a, ToolListEvent>,
) -> Element<'a, ToolListEvent> {
    checkbox_grid(
        distribute_into_columns(&items)
            .into_iter()
            .map(|col| {
                column(col.into_iter().map(&mut cell).collect::<Vec<_>>())
                    .spacing(4)
                    .into()
            })
            .collect(),
    )
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
