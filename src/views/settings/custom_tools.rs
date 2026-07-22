//! "Custom Tools" settings tab: each user-defined command-line tool is
//! shown as a collapsible card; expanding a card reveals its edit form.

use super::{
    SettingsEvent, SettingsState, SettingsTab, ToolTextField, card_rule, delete_button_style,
    field_row, form_card_style, sub_card_style, textarea_field_row,
};
use crate::Message;
use crate::views::theme::{CRABOT_PRIMARY, CRABOT_TEXT_MUTED};
use crate::widgets::textarea::TextArea;
use crabot::tools::custom::{CustomTool, ParameterType, ToolParameter};
use iced::padding;
use iced::{
    Alignment, Element, Length,
    widget::{button, checkbox, column, container, mouse_area, pick_list, row, text, text_input},
};

/// Simple parameter kinds offered by the type picker. Complex kinds
/// (array / object / union) are preserved but cannot be edited here.
const PARAM_KINDS: &[&str] = &["string", "integer", "number", "boolean"];

// ── Page ───────────────────────────────────────────────────────────

pub(super) fn custom_tools_page<'a>(state: &'a SettingsState) -> Element<'a, Message> {
    let header = row![
        text("Custom Tools")
            .size(13)
            .font(iced::Font {
                weight: iced::font::Weight::Bold,
                ..iced::Font::DEFAULT
            })
            .color(CRABOT_PRIMARY),
        iced::widget::Space::new().width(Length::Fill),
        button(text("+ New Tool").size(12))
            .padding([4, 10])
            .style(crate::views::styles::primary_button)
            .on_press(Message::SettingsEvent(SettingsEvent::NewTool)),
    ]
    .align_y(Alignment::Center);

    let body: Element<'a, Message> = if state.working_tools.custom_tools.is_empty() {
        container(
            text("No custom tools yet. Click + New Tool to define a command-line tool.")
                .size(12)
                .color(CRABOT_TEXT_MUTED),
        )
        .padding(16)
        .center_x(Length::Fill)
        .into()
    } else {
        let cards: Vec<Element<'a, Message>> = state
            .working_tools
            .custom_tools
            .iter()
            .enumerate()
            .map(|(i, tool)| {
                let expanded = state.expanded_tool == Some(i);
                tool_card(
                    i,
                    tool,
                    expanded,
                    &state.tool_desc_area,
                    &state.tool_instr_area,
                )
            })
            .collect();
        column(cards).spacing(8).into()
    };

    let save_label = if state.save_feedback == Some(SettingsTab::CustomTools) {
        "Saved ✓"
    } else {
        "Save"
    };
    let save_button = button(text(save_label).size(13))
        .style(crate::views::styles::primary_button)
        .on_press(Message::SettingsEvent(SettingsEvent::SaveTools));

    let action_row = row![iced::widget::Space::new().width(Length::Fill), save_button,]
        .spacing(10)
        .padding(padding::top(8));

    column![header, body, action_row].spacing(12).into()
}

// ── Tool card ─────────────────────────────────────────────────────

/// A collapsible card: header with the tool name and parameter count;
/// the edit form appears below when expanded.
fn tool_card<'a>(
    index: usize,
    tool: &'a CustomTool,
    expanded: bool,
    desc_area: &'a TextArea,
    instr_area: &'a TextArea,
) -> Element<'a, Message> {
    let arrow = if expanded { "▼" } else { "⯈" };
    let named = !tool.name.trim().is_empty();
    let display_name = if named { &tool.name } else { "untitled" };
    let count = tool.parameters.len();
    let summary = format!("{count} parameter{}", if count == 1 { "" } else { "s" });

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
    .on_press(Message::SettingsEvent(SettingsEvent::ToggleTool(index)));

    let delete = button(text("✕").size(11))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(Message::SettingsEvent(SettingsEvent::DeleteTool(index)));

    let header_row = row![title, delete].spacing(4).align_y(Alignment::Center);

    container(if expanded {
        column![
            header_row,
            card_rule(),
            tool_form(index, tool, desc_area, instr_area)
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

// ── Edit form ─────────────────────────────────────────────────────

fn tool_form<'a>(
    index: usize,
    tool: &'a CustomTool,
    desc_area: &'a TextArea,
    instr_area: &'a TextArea,
) -> Element<'a, Message> {
    column![
        field_row(
            "Name",
            &tool.name,
            "snake_case name used by the model",
            false,
            move |v| { Message::SettingsEvent(SettingsEvent::EditToolName(index, v)) }
        ),
        textarea_field_row(
            "Description",
            desc_area,
            "What the tool does — shown to the model",
            move |msg| Message::SettingsEvent(SettingsEvent::ToolTextArea(
                ToolTextField::Description,
                msg,
            )),
        ),
        textarea_field_row(
            "Instruction",
            instr_area,
            "When and how the model should use this tool",
            move |msg| Message::SettingsEvent(SettingsEvent::ToolTextArea(
                ToolTextField::Instruction,
                msg,
            )),
        ),
        field_row(
            "Command",
            &tool.command,
            "Command template, e.g. git log {args}",
            true,
            move |v| Message::SettingsEvent(SettingsEvent::EditToolCommand(index, v)),
        ),
        params_section(index, tool),
        text(
            "Command uses TinyTemplate syntax: {param} inserts a value, \
              {{ if param }}…{{ endif }} adds conditional arguments."
        )
        .size(11)
        .color(CRABOT_TEXT_MUTED),
    ]
    .spacing(8)
    .into()
}

// ── Parameters ────────────────────────────────────────────────────

fn params_section<'a>(tool_index: usize, tool: &'a CustomTool) -> Element<'a, Message> {
    let header = row![
        container(text("Parameters").size(14))
            .width(90)
            .align_x(Alignment::End),
        button(text("+ Add").size(12))
            .padding([4, 10])
            .style(crate::views::styles::secondary_button)
            .on_press(Message::SettingsEvent(SettingsEvent::AddToolParam(
                tool_index
            ))),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    if tool.parameters.is_empty() {
        return column![header].spacing(6).into();
    }

    let cards: Vec<Element<'a, Message>> = tool
        .parameters
        .iter()
        .enumerate()
        .map(|(i, param)| param_card(tool_index, i, param))
        .collect();

    // Indent param cards so they align with the field inputs above.
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

/// Two-row editor for one parameter: name + type + required + remove on the
/// first row, full-width description on the second.
fn param_card<'a>(
    tool_index: usize,
    index: usize,
    param: &'a ToolParameter,
) -> Element<'a, Message> {
    let kind_picker = pick_list(PARAM_KINDS, simple_kind(&param.kind), move |kind| {
        Message::SettingsEvent(SettingsEvent::EditParamKind(
            tool_index,
            index,
            kind.to_string(),
        ))
    })
    .text_size(12)
    .placeholder(kind_name(&param.kind))
    .width(Length::Fixed(110.0));

    let required = checkbox(param.required)
        .label("required")
        .text_size(12)
        .on_toggle(move |v| {
            Message::SettingsEvent(SettingsEvent::ToggleParamRequired(tool_index, index, v))
        })
        .style(crate::views::primary_checkbox);

    let remove = button(text("✕").size(10))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(Message::SettingsEvent(SettingsEvent::DeleteToolParam(
            tool_index, index,
        )));

    container(
        column![
            row![
                text_input("Parameter name", &param.name)
                    .on_input(move |v| {
                        Message::SettingsEvent(SettingsEvent::EditParamName(tool_index, index, v))
                    })
                    .padding(4)
                    .size(13)
                    .width(Length::Fill),
                kind_picker,
                required,
                remove,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text_input(
                "Parameter description — shown to the model",
                &param.description
            )
            .on_input(move |v| {
                Message::SettingsEvent(SettingsEvent::EditParamDescription(tool_index, index, v))
            })
            .padding(4)
            .size(13)
            .width(Length::Fill),
        ]
        .spacing(6),
    )
    .padding(8)
    .style(sub_card_style)
    .width(Length::Fill)
    .into()
}

// ── Kind helpers ──────────────────────────────────────────────────

/// Map a parameter type to its simple kind name, if it is one.
fn simple_kind(kind: &ParameterType) -> Option<&'static str> {
    match kind {
        ParameterType::String => Some("string"),
        ParameterType::Integer => Some("integer"),
        ParameterType::Number => Some("number"),
        ParameterType::Boolean => Some("boolean"),
        _ => None,
    }
}

/// Human-readable name of any parameter type — shown as the picker
/// placeholder for complex types the form cannot edit.
fn kind_name(kind: &ParameterType) -> &'static str {
    match kind {
        ParameterType::Null => "null",
        ParameterType::String => "string",
        ParameterType::Integer => "integer",
        ParameterType::Number => "number",
        ParameterType::Boolean => "boolean",
        ParameterType::Array(_) => "array",
        ParameterType::Object(_) => "object",
        ParameterType::Union(_) => "union",
    }
}
