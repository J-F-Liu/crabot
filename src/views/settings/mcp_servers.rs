//! "MCP Servers" settings tab: each configured MCP server is shown as a
//! collapsible card; expanding a card reveals its edit form.

use super::{
    SettingsEvent, SettingsState, SettingsTab, add_section, card_rule, collapsible_header,
    delete_button_style, empty_hint, field_row, form_card_style, numbered_name, remove_expanded,
    section_header, sub_card_style, textarea_field_row, toggle_expanded, unique_name,
};
use crate::views::theme::color_muted;
use crate::widgets::textarea::TextArea;
use crabot::tools::mcp::{McpServer, McpTransport};
use iced::{
    Alignment, Element, Length,
    widget::{button, checkbox, column, container, pick_list, row, text, text_input},
};
use indexmap::IndexMap;

// ── Events ──────────────────────────────────────────────────────────

/// Events for the MCP Servers tab.
#[derive(Debug, Clone)]
pub(crate) enum McpEvent {
    /// Expand/collapse the MCP server card at the given index.
    ToggleMcp(usize),
    /// Append a new blank MCP server and expand its card.
    NewMcp,
    DeleteMcp(usize),
    EditMcpName(usize, String),
    /// Switch a server's transport kind ("stdio" or "http").
    EditMcpTransport(usize, String),
    /// Edit the spawn command of a stdio server.
    EditMcpCmd(usize, String),
    /// Edit the URL of an HTTP server.
    EditMcpUrl(usize, String),
    ToggleMcpQualify(usize, bool),
    /// Add a key/value entry to the active transport's option map
    /// (env vars for stdio servers, HTTP headers for http servers).
    AddMcpMapEntry(usize),
    DeleteMcpMapEntry(usize, usize),
    EditMcpMapKey(usize, usize, String),
    EditMcpMapValue(usize, usize, String),
    /// A [`TextArea`] edit in the MCP server prompt form.
    McpTextArea(crate::widgets::textarea::Message),
    /// Persist MCP servers to disk.
    SaveMcp,
}

/// Transport kinds offered by the picker.
const TRANSPORT_KINDS: &[&str] = &["stdio", "http"];

// ── Page ───────────────────────────────────────────────────────────

pub(super) fn mcp_servers_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = row![
        section_header("MCP Servers"),
        iced::widget::Space::new().width(Length::Fill),
        button(text("+ New Server").size(12))
            .padding([4, 10])
            .style(crate::views::styles::primary_button)
            .on_press(SettingsEvent::Mcp(McpEvent::NewMcp)),
    ]
    .align_y(Alignment::Center);

    let body: Element<'a, SettingsEvent> = if state.working_mcp.servers.is_empty() {
        empty_hint("No MCP servers yet. Click + New Server to configure one.")
    } else {
        let cards: Vec<Element<'a, SettingsEvent>> = state
            .working_mcp
            .servers
            .iter()
            .enumerate()
            .map(|(i, server)| {
                server_card(
                    i,
                    server,
                    state.expanded_mcp == Some(i),
                    &state.mcp_prompt_area,
                )
            })
            .collect();
        column(cards).spacing(8).into()
    };

    let action_row = super::save_action_row(
        state,
        SettingsTab::McpServers,
        SettingsEvent::Mcp(McpEvent::SaveMcp),
    );

    column![header, body, action_row].spacing(12).into()
}

// ── Server card ─────────────────────────────────────────────────────

/// A collapsible card: header with the server name and transport summary;
/// the edit form appears below when expanded.
fn server_card<'a>(
    index: usize,
    server: &'a McpServer,
    expanded: bool,
    prompt_area: &'a TextArea,
) -> Element<'a, SettingsEvent> {
    let display_name = if server.name.trim().is_empty() {
        "untitled"
    } else {
        &server.name
    };

    let title = collapsible_header(
        expanded,
        display_name.to_string(),
        transport_summary(&server.transport),
        SettingsEvent::Mcp(McpEvent::ToggleMcp(index)),
    );

    let delete = button(text("✕").size(11))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(SettingsEvent::Mcp(McpEvent::DeleteMcp(index)));

    let header_row = row![title, delete].spacing(4).align_y(Alignment::Center);

    container(if expanded {
        column![
            header_row,
            card_rule(),
            server_form(index, server, prompt_area)
        ]
        .spacing(10)
    } else {
        column![header_row]
    })
    .padding([10, 12])
    .style(form_card_style)
    .width(Length::Fill)
    .into()
}

/// One-line summary of the transport, e.g. `stdio · npx -y @org/server`.
fn transport_summary(transport: &McpTransport) -> String {
    let (kind, target) = match transport {
        McpTransport::Stdio { cmd, .. } => ("stdio", cmd.as_str()),
        McpTransport::Http { url, .. } => ("http", url.as_str()),
    };
    let target = target.trim();
    if target.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {}", truncate(target, 48))
    }
}

/// Truncate a string to at most `max` chars, appending an ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ── Edit form ─────────────────────────────────────────────────────

fn server_form<'a>(
    index: usize,
    server: &'a McpServer,
    prompt_area: &'a TextArea,
) -> Element<'a, SettingsEvent> {
    let transport_fields: Element<'a, SettingsEvent> = match &server.transport {
        McpTransport::Stdio { cmd, env_vars } => column![
            field_row(
                "Command",
                cmd,
                "Command to spawn, e.g. npx -y @org/server",
                true,
                None,
                None,
                move |v| SettingsEvent::Mcp(McpEvent::EditMcpCmd(index, v)),
            ),
            map_section(index, "Env Vars", "NAME", env_vars),
        ]
        .spacing(8)
        .into(),
        McpTransport::Http { url, headers } => column![
            field_row(
                "URL",
                url,
                "Server URL, e.g. http://localhost:8000/mcp",
                true,
                None,
                None,
                move |v| SettingsEvent::Mcp(McpEvent::EditMcpUrl(index, v)),
            ),
            map_section(index, "Headers", "Header-Name", headers),
        ]
        .spacing(8)
        .into(),
    };

    column![
        field_row(
            "Name",
            &server.name,
            "Unique name for this server",
            false,
            None,
            None,
            move |v| SettingsEvent::Mcp(McpEvent::EditMcpName(index, v)),
        ),
        transport_row(index, &server.transport),
        transport_fields,
        qualify_row(index, server),
        textarea_field_row(
            "Prompt",
            prompt_area,
            "System-prompt text injected when this server is enabled",
            move |msg| SettingsEvent::Mcp(McpEvent::McpTextArea(msg)),
        ),
        text(
            "Prompt is added to the system prompt when the server is enabled and \
              at least one of its tools is selected."
        )
        .size(11)
        .color(color_muted()),
    ]
    .spacing(8)
    .into()
}

/// Transport kind picker row.
fn transport_row<'a>(index: usize, transport: &'a McpTransport) -> Element<'a, SettingsEvent> {
    let selected = match transport {
        McpTransport::Stdio { .. } => Some("stdio"),
        McpTransport::Http { .. } => Some("http"),
    };
    let label_col = container(text("Transport").size(14))
        .width(90)
        .align_x(Alignment::End);
    let picker = pick_list(TRANSPORT_KINDS, selected, move |kind| {
        SettingsEvent::Mcp(McpEvent::EditMcpTransport(index, kind.to_string()))
    })
    .text_size(12)
    .width(Length::Fixed(110.0));
    row![label_col, picker]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

/// Checkbox controlling whether tool names are prefixed with the server name.
fn qualify_row<'a>(index: usize, server: &'a McpServer) -> Element<'a, SettingsEvent> {
    let label_col = container(text("Qualify").size(14))
        .width(90)
        .align_x(Alignment::End);
    let name = if server.name.trim().is_empty() {
        "server"
    } else {
        server.name.trim()
    };
    let toggle = checkbox(server.qualify_tool_names)
        .label(format!("Prefix tool names with \"{name}_\""))
        .text_size(12)
        .on_toggle(move |v| SettingsEvent::Mcp(McpEvent::ToggleMcpQualify(index, v)))
        .style(crate::views::primary_checkbox);
    row![label_col, toggle]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

// ── Env vars / headers ──────────────────────────────────────────────

/// Key/value editor for the active transport's option map (env vars for
/// stdio servers, HTTP headers for http servers).
fn map_section<'a>(
    server_index: usize,
    label: &'static str,
    key_placeholder: &'static str,
    map: &'a IndexMap<String, String>,
) -> Element<'a, SettingsEvent> {
    let cards: Vec<Element<'a, SettingsEvent>> = map
        .iter()
        .enumerate()
        .map(|(i, (key, value))| map_entry_card(server_index, i, key, value, key_placeholder))
        .collect();

    add_section(
        label,
        SettingsEvent::Mcp(McpEvent::AddMcpMapEntry(server_index)),
        cards,
    )
}

/// One key/value row: key input, value input, and a remove button.
fn map_entry_card<'a>(
    server_index: usize,
    index: usize,
    key: &'a str,
    value: &'a str,
    key_placeholder: &'static str,
) -> Element<'a, SettingsEvent> {
    let remove = button(text("✕").size(10))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(SettingsEvent::Mcp(McpEvent::DeleteMcpMapEntry(
            server_index,
            index,
        )));

    container(
        row![
            text_input(key_placeholder, key)
                .on_input(move |v| {
                    SettingsEvent::Mcp(McpEvent::EditMcpMapKey(server_index, index, v))
                })
                .padding(4)
                .size(13)
                .width(Length::FillPortion(2)),
            text_input("value", value)
                .on_input(move |v| {
                    SettingsEvent::Mcp(McpEvent::EditMcpMapValue(server_index, index, v))
                })
                .padding(4)
                .size(13)
                .width(Length::FillPortion(3)),
            remove,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding(8)
    .style(sub_card_style)
    .width(Length::Fill)
    .into()
}

// ── Update ─────────────────────────────────────────────────────────

/// Handle an MCP Servers tab event, mutating `state.working_mcp`.
pub(super) fn update(state: &mut SettingsState, event: McpEvent) {
    match event {
        McpEvent::ToggleMcp(index) => {
            state.flush_mcp_text_area();
            toggle_expanded(&mut state.expanded_mcp, index);
            state.init_mcp_text_area();
        }
        McpEvent::NewMcp => {
            state.flush_mcp_text_area();
            let name = unique_name("new_server", |n| {
                state.working_mcp.servers.iter().any(|s| s.name == n)
            });
            state.working_mcp.servers.push(McpServer {
                name,
                transport: McpTransport::Stdio {
                    cmd: String::new(),
                    env_vars: IndexMap::new(),
                },
                qualify_tool_names: false,
                prompt: String::new(),
            });
            state.expanded_mcp = Some(state.working_mcp.servers.len() - 1);
            state.init_mcp_text_area();
        }
        McpEvent::DeleteMcp(index) => {
            state.flush_mcp_text_area();
            if index < state.working_mcp.servers.len() {
                state.working_mcp.servers.remove(index);
            }
            state.expanded_mcp = remove_expanded(state.expanded_mcp, index);
            state.init_mcp_text_area();
        }
        McpEvent::EditMcpName(index, v) => {
            if let Some(s) = state.mcp_mut(index) {
                s.name = v;
            }
        }
        McpEvent::EditMcpTransport(index, kind) => {
            if let Some(s) = state.mcp_mut(index) {
                let new_transport = match (kind.as_str(), &s.transport) {
                    ("http", McpTransport::Stdio { .. }) => Some(McpTransport::Http {
                        url: String::new(),
                        headers: IndexMap::new(),
                    }),
                    ("stdio", McpTransport::Http { .. }) => Some(McpTransport::Stdio {
                        cmd: String::new(),
                        env_vars: IndexMap::new(),
                    }),
                    _ => None,
                };
                if let Some(transport) = new_transport {
                    s.transport = transport;
                }
            }
        }
        McpEvent::EditMcpCmd(index, v) => {
            if let Some(s) = state.mcp_mut(index)
                && let McpTransport::Stdio { cmd, .. } = &mut s.transport
            {
                *cmd = v;
            }
        }
        McpEvent::EditMcpUrl(index, v) => {
            if let Some(s) = state.mcp_mut(index)
                && let McpTransport::Http { url, .. } = &mut s.transport
            {
                *url = v;
            }
        }
        McpEvent::ToggleMcpQualify(index, v) => {
            if let Some(s) = state.mcp_mut(index) {
                s.qualify_tool_names = v;
            }
        }
        McpEvent::AddMcpMapEntry(index) => {
            if let Some(s) = state.mcp_mut(index) {
                let (map, base) = match &mut s.transport {
                    McpTransport::Stdio { env_vars, .. } => (env_vars, "KEY"),
                    McpTransport::Http { headers, .. } => (headers, "HEADER"),
                };
                let key = numbered_name(base, map.len() + 1, |k| map.contains_key(k));
                map.insert(key, String::new());
            }
        }
        McpEvent::DeleteMcpMapEntry(server_index, index) => {
            if let Some(map) = state.mcp_map_mut(server_index) {
                map.shift_remove_index(index);
            }
        }
        McpEvent::EditMcpMapKey(server_index, index, new_key) => {
            // Rename the key in place, keeping the entry's position so
            // the row (and its input focus) doesn't jump while typing.
            // Renames that would collide with an existing key are ignored.
            if let Some(map) = state.mcp_map_mut(server_index)
                && index < map.len()
                && !map.contains_key(&new_key)
                && let Some((_, value)) = map.shift_remove_index(index)
            {
                let last = map.len();
                map.insert(new_key, value);
                map.move_index(last, index);
            }
        }
        McpEvent::EditMcpMapValue(server_index, index, v) => {
            if let Some(map) = state.mcp_map_mut(server_index)
                && let Some((_, value)) = map.get_index_mut(index)
            {
                *value = v;
            }
        }
        McpEvent::McpTextArea(msg) => state.mcp_prompt_area.update(msg, false),
        McpEvent::SaveMcp => {
            // Flush any pending TextArea edits to server structs.
            state.flush_mcp_text_area();
            // Drop servers left with a blank name — they cannot be connected.
            state
                .working_mcp
                .servers
                .retain(|s| !s.name.trim().is_empty());
            for s in &mut state.working_mcp.servers {
                s.name = s.name.trim().to_string();
                // Drop key/value entries with a blank key.
                match &mut s.transport {
                    McpTransport::Stdio { env_vars, .. } => {
                        env_vars.retain(|k, _| !k.trim().is_empty());
                    }
                    McpTransport::Http { headers, .. } => {
                        headers.retain(|k, _| !k.trim().is_empty());
                    }
                }
            }
            // Deduplicate server names — keep the first occurrence of each name.
            // Duplicate names would corrupt the connection map and enable state.
            let mut seen = std::collections::HashSet::new();
            state
                .working_mcp
                .servers
                .retain(|s| seen.insert(s.name.clone()));
            // Collapse the card — indices may have shifted after pruning.
            state.expanded_mcp = None;
            state.save_feedback = Some(SettingsTab::McpServers);
        }
    }
}
