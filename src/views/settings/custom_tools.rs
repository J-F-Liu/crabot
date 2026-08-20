//! "Custom Tools" settings tab: each user-defined command-line tool is
//! shown as a collapsible card; expanding a card reveals its edit form.

use super::{
    SettingsEvent, SettingsState, SettingsTab, add_section, card_rule, collapsible_header,
    count_label, delete_button_style, empty_hint, field_row, form_card_style, numbered_name,
    remove_expanded, section_header, sub_card_style, textarea_field_row, toggle_expanded,
    unique_name,
};
use crate::views::theme::color_muted;
use crate::widgets::textarea::TextArea;
use crabot::tools::custom::{CustomTool, ParameterType, ToolParameter};
use iced::{
    Alignment, Element, Length,
    widget::{button, checkbox, column, container, pick_list, row, text, text_input},
};

/// Simple parameter kinds offered by the type picker. Complex kinds
/// (array / object / union) are preserved but cannot be edited here.
const PARAM_KINDS: &[&str] = &["string", "integer", "number", "boolean"];

// ── Events ──────────────────────────────────────────────────────────

/// Identifies which text field in the custom-tool form is being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolTextField {
    Description,
    Instruction,
}

/// Events for the Custom Tools tab.
#[derive(Debug, Clone)]
pub(crate) enum CustomToolsEvent {
    /// Expand/collapse the tool card at the given index.
    ToggleTool(usize),
    /// Append a new blank tool and expand its card.
    NewTool,
    DeleteTool(usize),
    EditToolName(usize, String),
    EditToolCommand(usize, String),
    AddToolParam(usize),
    DeleteToolParam(usize, usize),
    EditParamName(usize, usize, String),
    EditParamKind(usize, usize, String),
    EditParamDescription(usize, usize, String),
    ToggleParamRequired(usize, usize, bool),
    /// A [`TextArea`] edit in the custom-tool form.
    ToolTextArea(ToolTextField, crate::widgets::textarea::Message),
    /// Persist custom tools to disk.
    SaveTools,
}

// ── Page ───────────────────────────────────────────────────────────

pub(super) fn custom_tools_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    let header = row![
        section_header("Custom Tools"),
        iced::widget::Space::new().width(Length::Fill),
        button(text("+ New Tool").size(12))
            .padding([4, 10])
            .style(crate::views::styles::primary_button)
            .on_press(SettingsEvent::CustomTools(CustomToolsEvent::NewTool)),
    ]
    .align_y(Alignment::Center);

    let body: Element<'a, SettingsEvent> = if state.working_tools.custom_tools.is_empty() {
        empty_hint("No custom tools yet. Click + New Tool to define a command-line tool.")
    } else {
        let cards: Vec<Element<'a, SettingsEvent>> = state
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

    let action_row = super::save_action_row(
        state,
        SettingsTab::CustomTools,
        SettingsEvent::CustomTools(CustomToolsEvent::SaveTools),
    );

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
) -> Element<'a, SettingsEvent> {
    let display_name = if tool.name.trim().is_empty() {
        "untitled"
    } else {
        &tool.name
    };
    let summary = count_label(tool.parameters.len(), "parameter");

    let title = collapsible_header(
        expanded,
        display_name.to_string(),
        summary,
        SettingsEvent::CustomTools(CustomToolsEvent::ToggleTool(index)),
    );

    let delete = button(text("✕").size(11))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(SettingsEvent::CustomTools(CustomToolsEvent::DeleteTool(
            index,
        )));

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
) -> Element<'a, SettingsEvent> {
    column![
        field_row(
            "Name",
            &tool.name,
            "snake_case name used by the model",
            false,
            None,
            None,
            move |v| SettingsEvent::CustomTools(CustomToolsEvent::EditToolName(index, v))
        ),
        textarea_field_row(
            "Description",
            desc_area,
            "What the tool does — shown to the model",
            move |msg| {
                SettingsEvent::CustomTools(CustomToolsEvent::ToolTextArea(
                    ToolTextField::Description,
                    msg,
                ))
            },
        ),
        textarea_field_row(
            "Instruction",
            instr_area,
            "When and how the model should use this tool",
            move |msg| {
                SettingsEvent::CustomTools(CustomToolsEvent::ToolTextArea(
                    ToolTextField::Instruction,
                    msg,
                ))
            },
        ),
        field_row(
            "Command",
            &tool.command,
            "Command template, e.g. git log {args}",
            true,
            None,
            None,
            move |v| SettingsEvent::CustomTools(CustomToolsEvent::EditToolCommand(index, v)),
        ),
        params_section(index, tool),
        text(
            "Command uses TinyTemplate syntax: {param} inserts a value, \
              {{ if param }}…{{ endif }} adds conditional arguments."
        )
        .size(11)
        .color(color_muted()),
    ]
    .spacing(8)
    .into()
}

// ── Parameters ────────────────────────────────────────────────────

fn params_section<'a>(tool_index: usize, tool: &'a CustomTool) -> Element<'a, SettingsEvent> {
    let cards: Vec<Element<'a, SettingsEvent>> = tool
        .parameters
        .iter()
        .enumerate()
        .map(|(i, param)| param_card(tool_index, i, param))
        .collect();

    add_section(
        "Parameters",
        SettingsEvent::CustomTools(CustomToolsEvent::AddToolParam(tool_index)),
        cards,
    )
}

/// Two-row editor for one parameter: name + type + required + remove on the
/// first row, full-width description on the second.
fn param_card<'a>(
    tool_index: usize,
    index: usize,
    param: &'a ToolParameter,
) -> Element<'a, SettingsEvent> {
    let kind_picker = pick_list(PARAM_KINDS, simple_kind(&param.kind), move |kind| {
        SettingsEvent::CustomTools(CustomToolsEvent::EditParamKind(
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
            SettingsEvent::CustomTools(CustomToolsEvent::ToggleParamRequired(tool_index, index, v))
        })
        .style(crate::views::primary_checkbox);

    let remove = button(text("✕").size(10))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(SettingsEvent::CustomTools(
            CustomToolsEvent::DeleteToolParam(tool_index, index),
        ));

    container(
        column![
            row![
                text_input("Parameter name", &param.name)
                    .on_input(move |v| {
                        SettingsEvent::CustomTools(CustomToolsEvent::EditParamName(
                            tool_index, index, v,
                        ))
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
                SettingsEvent::CustomTools(CustomToolsEvent::EditParamDescription(
                    tool_index, index, v,
                ))
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

// ── Update ─────────────────────────────────────────────────────────

/// Handle a Custom Tools tab event, mutating `state.working_tools`.
pub(super) fn update(state: &mut SettingsState, event: CustomToolsEvent) {
    match event {
        CustomToolsEvent::ToggleTool(index) => {
            state.flush_tool_text_areas();
            toggle_expanded(&mut state.expanded_tool, index);
            state.init_tool_text_areas();
        }
        CustomToolsEvent::NewTool => {
            state.flush_tool_text_areas();
            let name = unique_name("new_tool", |n| {
                state.working_tools.custom_tools.iter().any(|t| t.name == n)
            });
            state.working_tools.custom_tools.push(CustomTool {
                name,
                description: String::new(),
                instruction: String::new(),
                parameters: vec![],
                command: String::new(),
            });
            state.expanded_tool = Some(state.working_tools.custom_tools.len() - 1);
            state.init_tool_text_areas();
        }
        CustomToolsEvent::DeleteTool(index) => {
            state.flush_tool_text_areas();
            if index < state.working_tools.custom_tools.len() {
                state.working_tools.custom_tools.remove(index);
            }
            state.expanded_tool = remove_expanded(state.expanded_tool, index);
            state.init_tool_text_areas();
        }
        CustomToolsEvent::EditToolName(index, v) => {
            if let Some(t) = state.tool_mut(index) {
                t.name = v;
            }
        }
        CustomToolsEvent::EditToolCommand(index, v) => {
            if let Some(t) = state.tool_mut(index) {
                t.command = v;
            }
        }
        CustomToolsEvent::AddToolParam(index) => {
            if let Some(t) = state.tool_mut(index) {
                let name = numbered_name("param", t.parameters.len() + 1, |n| {
                    t.parameters.iter().any(|p| p.name == n)
                });
                t.parameters.push(ToolParameter {
                    name,
                    kind: ParameterType::String,
                    description: String::new(),
                    required: true,
                });
            }
        }
        CustomToolsEvent::DeleteToolParam(tool_index, index) => {
            if let Some(t) = state.tool_mut(tool_index)
                && index < t.parameters.len()
            {
                t.parameters.remove(index);
            }
        }
        CustomToolsEvent::EditParamName(tool_index, index, v) => {
            if let Some(p) = state.param_mut(tool_index, index) {
                p.name = v;
            }
        }
        CustomToolsEvent::EditParamKind(tool_index, index, kind) => {
            if let Some(p) = state.param_mut(tool_index, index) {
                p.kind = match kind.as_str() {
                    "integer" => ParameterType::Integer,
                    "number" => ParameterType::Number,
                    "boolean" => ParameterType::Boolean,
                    _ => ParameterType::String,
                };
            }
        }
        CustomToolsEvent::EditParamDescription(tool_index, index, v) => {
            if let Some(p) = state.param_mut(tool_index, index) {
                p.description = v;
            }
        }
        CustomToolsEvent::ToggleParamRequired(tool_index, index, v) => {
            if let Some(p) = state.param_mut(tool_index, index) {
                p.required = v;
            }
        }
        CustomToolsEvent::ToolTextArea(field, msg) => match field {
            ToolTextField::Description => state.tool_desc_area.update(msg, false),
            ToolTextField::Instruction => state.tool_instr_area.update(msg, false),
        },
        CustomToolsEvent::SaveTools => {
            // Flush any pending TextArea edits to tool structs.
            state.flush_tool_text_areas();
            // Drop custom tools left with a blank name — they cannot be invoked.
            state
                .working_tools
                .custom_tools
                .retain(|t| !t.name.trim().is_empty());
            // Trim leading/trailing whitespace from remaining tool names.
            for t in &mut state.working_tools.custom_tools {
                t.name = t.name.trim().to_string();
            }
            state.save_feedback = Some(SettingsTab::CustomTools);
        }
    }
}
