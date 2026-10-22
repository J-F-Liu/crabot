use std::collections::{HashMap, HashSet};

use iced::{
    Alignment, Element, Length, padding,
    widget::{Space, button, checkbox, column, container, mouse_area, row, text, text::Wrapping},
};

use super::icons;
use crate::tools::mcp::{DiscoverFailure, McpTool};

pub const BUILTIN_TOOLS: &str = "Builtin Tools";
pub const CUSTOM_TOOLS: &str = "Custom Tools";
pub const MCP_TOOLS: &str = "MCP Tools";

/// Events emitted by the tool-list views; callers map them to the owning
/// [`crate::PromptEvent`] or [`crate::ToolEvent`] variants.
#[derive(Clone)]
pub(crate) enum ToolListEvent {
    ExpandSection(&'static str),
    ToggleMcpServer(String, bool),
    ToggleAgentTool(String, bool),
    SetMcpServerTools(String, bool),
    CycleMcpColumns(String),
    ToggleMcpServerExpanded(String),
}

/// Left-pane section state, plus the MCP tool grid preferences.
#[derive(Debug, Clone)]
pub(crate) struct ToolListState {
    pub builtin_expanded: bool,
    pub custom_expanded: bool,
    pub mcp_expanded: bool,
    /// Per-server MCP tool grid width; absent means the 3-column default.
    mcp_columns: HashMap<String, usize>,
    /// MCP servers whose tool grids are collapsed; absent means expanded.
    mcp_collapsed_servers: HashSet<String>,
}

impl Default for ToolListState {
    fn default() -> Self {
        Self {
            builtin_expanded: true,
            custom_expanded: true,
            mcp_expanded: true,
            mcp_columns: HashMap::new(),
            mcp_collapsed_servers: HashSet::new(),
        }
    }
}

impl ToolListState {
    /// Grid width for one MCP server, clamped to 1..=3 columns.
    pub(crate) fn mcp_columns_for(&self, server: &str) -> usize {
        self.mcp_columns
            .get(server)
            .copied()
            .unwrap_or(TOOL_GRID_COLS)
            .clamp(1, TOOL_GRID_COLS)
    }

    /// Advance one MCP server's grid 1 → 2 → 3 → 1 columns.
    pub(crate) fn cycle_mcp_columns(&mut self, server: &str) {
        let next = self.mcp_columns_for(server) % TOOL_GRID_COLS + 1;
        self.mcp_columns.insert(server.to_string(), next);
    }

    /// Whether one MCP server's tool grid is expanded.
    pub(crate) fn mcp_server_expanded(&self, server: &str) -> bool {
        !self.mcp_collapsed_servers.contains(server)
    }

    /// Toggle one MCP server's tool grid.
    pub(crate) fn toggle_mcp_server_expanded(&mut self, server: &str) {
        if !self.mcp_collapsed_servers.remove(server) {
            self.mcp_collapsed_servers.insert(server.to_string());
        }
    }

    /// Forget one server's grid width and collapse state; it was removed.
    pub(crate) fn forget_server(&mut self, server: &str) {
        self.mcp_columns.remove(server);
        self.mcp_collapsed_servers.remove(server);
    }

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
/// No bulk select: these lists stay short, unlike MCP servers.
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

/// Default and maximum number of columns used for tool checkboxes.
const TOOL_GRID_COLS: usize = 3;

/// Distribute items into `columns` columns (row-major: fill across, then down).
fn distribute_into_columns<T: Copy>(items: &[T], columns: usize) -> Vec<Vec<T>> {
    let columns = columns.max(1);
    let n_rows = items.len().div_ceil(columns);
    let mut cols: Vec<Vec<T>> = (0..columns).map(|_| Vec::with_capacity(n_rows)).collect();
    for (i, item) in items.iter().enumerate() {
        cols[i % columns].push(*item);
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
    checkbox_grid_by(
        names.iter().map(String::as_str).collect(),
        TOOL_GRID_COLS,
        |name| checkbox_cell(name, None, selected, true),
    )
}

/// Collapsible "MCP Tools" section; servers being (re)discovered show that on
/// their label, and failed ones retry by re-checking their box.
pub(crate) fn mcp_tools_section<'a>(
    state: &ToolListState,
    selected: &'a HashSet<String>,
    groups: &'a [(String, Vec<McpTool>)],
    enabled_mcp_servers: &'a HashSet<String>,
    errors: &'a HashMap<String, DiscoverFailure>,
    pending: &'a HashSet<String>,
    lang: crabot::i18n::Lang,
) -> Element<'a, ToolListEvent> {
    let header = section_header(MCP_TOOLS, state.mcp_expanded, lang);
    if !state.mcp_expanded || (groups.is_empty() && errors.is_empty() && pending.is_empty()) {
        return column![header].into();
    }
    let group_view = |server: &'a str, tools: Option<&'a [McpTool]>| {
        let status = ServerStatus {
            error: errors.get(server).copied(),
            pending: pending.contains(server),
            enabled: enabled_mcp_servers.contains(server),
        };
        mcp_server_group_view(state, server, tools, status, selected, lang)
    };
    // Servers still connecting or failed before listing have no group yet.
    let mut missing: Vec<&str> = errors
        .keys()
        .chain(pending.iter())
        .filter(|name| !groups.iter().any(|(g, _)| g == *name))
        .map(String::as_str)
        .collect();
    missing.sort_unstable();
    missing.dedup();
    let rows: Vec<Element<'a, ToolListEvent>> = groups
        .iter()
        .map(|(server, tools)| (server.as_str(), Some(tools.as_slice())))
        .chain(missing.into_iter().map(|server| (server, None)))
        .map(|(server, tools)| group_view(server, tools))
        .collect();
    column![header, column(rows).spacing(4).padding(padding::left(4))]
        .spacing(4)
        .into()
}

/// One MCP server's discovery status, annotated onto its checkbox label.
struct ServerStatus {
    error: Option<DiscoverFailure>,
    pending: bool,
    enabled: bool,
}

/// One MCP server row: checkbox, bulk buttons, and the tool grid when expanded.
fn mcp_server_group_view<'a>(
    state: &ToolListState,
    server: &'a str,
    tools: Option<&'a [McpTool]>,
    status: ServerStatus,
    selected: &'a HashSet<String>,
    lang: crabot::i18n::Lang,
) -> Element<'a, ToolListEvent> {
    let ServerStatus {
        error,
        pending,
        enabled,
    } = status;
    let label = match (pending, error) {
        (true, Some(_)) => format!("{server} ({})", lang.tr("Reconnect...")),
        (true, None) => format!("{server} ({})", lang.tr("Connecting...")),
        (false, Some(failure)) => format!("{server} ({})", failure_label(failure, lang)),
        (false, None) => server.to_string(),
    };
    // Fill the row so the label can wrap; only the label stays clickable.
    let mut header = row![container(server_checkbox(server, label, enabled)).width(Length::Fill)]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let Some(tools) = tools.filter(|tools| !tools.is_empty()) else {
        return header.into();
    };
    let expanded = state.mcp_server_expanded(server);
    let columns = state.mcp_columns_for(server);
    let all_selected = tools.iter().all(|tool| selected.contains(&tool.name));
    let toggle = ToolListEvent::ToggleMcpServerExpanded(server.to_string());
    header = header
        .push(mcp_tool_buttons(
            server,
            all_selected,
            enabled,
            columns,
            lang,
        ))
        .push(icons::chevron_action(expanded, lang, toggle));
    let header: Element<'a, ToolListEvent> = header.into();
    if !expanded {
        return header;
    }
    column![header, mcp_tools_view(selected, tools, enabled, columns)]
        .spacing(4)
        .into()
}

/// Small secondary action button; `on_press` is `None` when disabled.
fn action_button(
    label: String,
    on_press: Option<ToolListEvent>,
) -> Element<'static, ToolListEvent> {
    button(text(label).size(12))
        .padding([2, 8])
        .style(crate::views::secondary_button)
        .on_press_maybe(on_press)
        .into()
}

/// Bulk-select and grid-width buttons for one server's tools.
fn mcp_tool_buttons(
    server: &str,
    all_selected: bool,
    server_enabled: bool,
    columns: usize,
    lang: crabot::i18n::Lang,
) -> Element<'static, ToolListEvent> {
    let select = if all_selected {
        "Deselect All"
    } else {
        "Select All"
    };
    let select_event =
        server_enabled.then(|| ToolListEvent::SetMcpServerTools(server.to_string(), !all_selected));
    let columns_label = if columns == 1 {
        lang.tr("1 col").to_string()
    } else {
        lang.tr("{} cols").replacen("{}", &columns.to_string(), 1)
    };
    row![
        action_button(lang.tr(select).to_string(), select_event),
        icons::with_tooltip(
            action_button(
                columns_label,
                Some(ToolListEvent::CycleMcpColumns(server.to_string())),
            ),
            lang.tr("Grid columns: click to cycle"),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
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
        .text_wrapping(Wrapping::Word)
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
    columns: usize,
) -> Element<'a, ToolListEvent> {
    checkbox_grid_by(tools.iter().collect(), columns, |tool| {
        checkbox_cell(&tool.name, tool.title.as_deref(), selected, enabled)
    })
}

/// Lay out cells in a grid with the given number of columns.
fn checkbox_grid_by<'a, T: Copy>(
    items: Vec<T>,
    columns: usize,
    mut cell: impl FnMut(T) -> Element<'a, ToolListEvent>,
) -> Element<'a, ToolListEvent> {
    checkbox_grid(
        distribute_into_columns(&items, columns)
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
