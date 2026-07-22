//! "MCP Servers" settings tab: each configured MCP server is shown as a
//! collapsible card; expanding a card reveals its edit form.

use super::{
    SettingsEvent, SettingsState, SettingsTab, card_rule, delete_button_style, field_row,
    form_card_style, sub_card_style, textarea_field_row,
};
use crate::Message;
use crate::views::theme::{CRABOT_PRIMARY, CRABOT_TEXT_MUTED};
use crate::widgets::textarea::TextArea;
use crabot::tools::mcp::{McpServer, McpTransport};
use iced::padding;
use iced::{
    Alignment, Element, Length,
    widget::{button, checkbox, column, container, mouse_area, pick_list, row, text, text_input},
};
use indexmap::IndexMap;

/// Transport kinds offered by the picker.
const TRANSPORT_KINDS: &[&str] = &["stdio", "http"];

// ── Page ───────────────────────────────────────────────────────────

pub(super) fn mcp_servers_page<'a>(state: &'a SettingsState) -> Element<'a, Message> {
    let header = row![
        text("MCP Servers")
            .size(13)
            .font(iced::Font {
                weight: iced::font::Weight::Bold,
                ..iced::Font::DEFAULT
            })
            .color(CRABOT_PRIMARY),
        iced::widget::Space::new().width(Length::Fill),
        button(text("+ New Server").size(12))
            .padding([4, 10])
            .style(crate::views::styles::primary_button)
            .on_press(Message::SettingsEvent(SettingsEvent::NewMcp)),
    ]
    .align_y(Alignment::Center);

    let body: Element<'a, Message> = if state.working_mcp.servers.is_empty() {
        container(
            text("No MCP servers yet. Click + New Server to configure one.")
                .size(12)
                .color(CRABOT_TEXT_MUTED),
        )
        .padding(16)
        .center_x(Length::Fill)
        .into()
    } else {
        let cards: Vec<Element<'a, Message>> = state
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

    let save_label = if state.save_feedback == Some(SettingsTab::McpServers) {
        "Saved ✓"
    } else {
        "Save"
    };
    let save_button = button(text(save_label).size(13))
        .style(crate::views::styles::primary_button)
        .on_press(Message::SettingsEvent(SettingsEvent::SaveMcp));

    let action_row = row![iced::widget::Space::new().width(Length::Fill), save_button,]
        .spacing(10)
        .padding(padding::top(8));

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
) -> Element<'a, Message> {
    let arrow = if expanded { "▼" } else { "⯈" };
    let named = !server.name.trim().is_empty();
    let display_name = if named { &server.name } else { "untitled" };
    let summary = transport_summary(&server.transport);

    let title = mouse_area(
        container(
            row![
                text(arrow).size(10).color(CRABOT_TEXT_MUTED).width(14),
                text(display_name).size(13).font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..iced::Font::DEFAULT
                }),
                text(summary).size(11).color(CRABOT_TEXT_MUTED),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill),
    )
    .on_press(Message::SettingsEvent(SettingsEvent::ToggleMcp(index)));

    let delete = button(text("✕").size(11))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(Message::SettingsEvent(SettingsEvent::DeleteMcp(index)));

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
) -> Element<'a, Message> {
    let transport_fields: Element<'a, Message> = match &server.transport {
        McpTransport::Stdio { cmd, env_vars } => column![
            field_row(
                "Command",
                cmd,
                "Command to spawn, e.g. npx -y @org/server",
                true,
                move |v| Message::SettingsEvent(SettingsEvent::EditMcpCmd(index, v)),
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
                move |v| Message::SettingsEvent(SettingsEvent::EditMcpUrl(index, v)),
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
            move |v| Message::SettingsEvent(SettingsEvent::EditMcpName(index, v)),
        ),
        transport_row(index, &server.transport),
        transport_fields,
        qualify_row(index, server),
        textarea_field_row(
            "Prompt",
            prompt_area,
            "System-prompt text injected when this server is enabled",
            move |msg| Message::SettingsEvent(SettingsEvent::McpTextArea(msg)),
        ),
        text(
            "Prompt is added to the system prompt when the server is enabled and \
              at least one of its tools is selected."
        )
        .size(11)
        .color(CRABOT_TEXT_MUTED),
    ]
    .spacing(8)
    .into()
}

/// Transport kind picker row.
fn transport_row<'a>(index: usize, transport: &'a McpTransport) -> Element<'a, Message> {
    let selected = match transport {
        McpTransport::Stdio { .. } => Some("stdio"),
        McpTransport::Http { .. } => Some("http"),
    };
    let label_col = container(text("Transport").size(14))
        .width(90)
        .align_x(Alignment::End);
    let picker = pick_list(TRANSPORT_KINDS, selected, move |kind| {
        Message::SettingsEvent(SettingsEvent::EditMcpTransport(index, kind.to_string()))
    })
    .text_size(12)
    .width(Length::Fixed(110.0));
    row![label_col, picker]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

/// Checkbox controlling whether tool names are prefixed with the server name.
fn qualify_row<'a>(index: usize, server: &'a McpServer) -> Element<'a, Message> {
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
        .on_toggle(move |v| Message::SettingsEvent(SettingsEvent::ToggleMcpQualify(index, v)))
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
) -> Element<'a, Message> {
    let header = row![
        container(text(label).size(14))
            .width(90)
            .align_x(Alignment::End),
        button(text("+ Add").size(12))
            .padding([4, 10])
            .style(crate::views::styles::secondary_button)
            .on_press(Message::SettingsEvent(SettingsEvent::AddMcpMapEntry(
                server_index
            ))),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    if map.is_empty() {
        return column![header].spacing(6).into();
    }

    let cards: Vec<Element<'a, Message>> = map
        .iter()
        .enumerate()
        .map(|(i, (key, value))| map_entry_card(server_index, i, key, value, key_placeholder))
        .collect();

    // Indent entry cards so they align with the field inputs above.
    column![
        header,
        row![
            iced::widget::Space::new().width(100),
            column(cards).spacing(6).width(Length::Fill),
        ],
    ]
    .spacing(6)
    .into()
}

/// One key/value row: key input, value input, and a remove button.
fn map_entry_card<'a>(
    server_index: usize,
    index: usize,
    key: &'a str,
    value: &'a str,
    key_placeholder: &'static str,
) -> Element<'a, Message> {
    let remove = button(text("✕").size(10))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(Message::SettingsEvent(SettingsEvent::DeleteMcpMapEntry(
            server_index,
            index,
        )));

    container(
        row![
            text_input(key_placeholder, key)
                .on_input(move |v| {
                    Message::SettingsEvent(SettingsEvent::EditMcpMapKey(server_index, index, v))
                })
                .padding(4)
                .size(13)
                .width(Length::FillPortion(2)),
            text_input("value", value)
                .on_input(move |v| {
                    Message::SettingsEvent(SettingsEvent::EditMcpMapValue(server_index, index, v))
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
