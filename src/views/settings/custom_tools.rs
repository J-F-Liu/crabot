//! Settings cards for command-based and HTTP API custom tools.

use super::{
    SettingsEvent, SettingsState, SettingsTab, ToolDraft, add_section, card_rule,
    collapsible_header, count_label, delete_button_style, empty_hint, field_row, form_card_style,
    hint, label_col, numbered_name, remove_expanded, section_header, sub_card_style,
    textarea_field_row, toggle_expanded, unique_name,
};
use crate::views::styles::styled_pick_list;
use crate::views::theme::color_danger_text;
use crate::widgets::textarea::TextArea;
use crabot::i18n::Lang;
use crabot::tools::custom::{
    CustomTool, HttpHeader, HttpMethod, HttpRequest, ParameterType, ToolKind, ToolMode,
    ToolParameter,
};
use iced::{
    Alignment, Element, Length, Widget as _,
    widget::{button, checkbox, column, container, row, text, text_input},
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
    Body,
    ResponseTemplate,
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
    /// Switch between a CLI command and an HTTP request.
    SetToolKind(usize, ToolMode),
    EditToolCommand(usize, String),
    EditToolMethod(usize, HttpMethod),
    EditToolUrl(usize, String),
    AddToolHeader(usize),
    DeleteToolHeader(usize, usize),
    EditToolHeaderName(usize, usize, String),
    EditToolHeaderValue(usize, usize, String),
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
    let lang = state.language;
    let header = row![
        section_header(lang.tr("Custom Tools")),
        iced::widget::Space::new().width(Length::Fill),
        button(text(lang.tr("+ New Tool")).size(12))
            .padding([4, 10])
            .style(crate::views::styles::primary_button)
            .on_press(SettingsEvent::CustomTools(CustomToolsEvent::NewTool)),
    ]
    .align_y(Alignment::Center);

    let body: Element<'a, SettingsEvent> =
        if state.working_tools.custom_tools.is_empty() {
            empty_hint(lang.tr(
                "No custom tools yet. Click + New Tool to define a command-line or HTTP API tool.",
            ))
        } else {
            let areas = ToolAreas {
                description: &state.tool_desc_area,
                instruction: &state.tool_instr_area,
                body: &state.tool_body_area,
                response_template: &state.tool_response_template_area,
            };
            let cards: Vec<Element<'a, SettingsEvent>> = state
                .working_tools
                .custom_tools
                .iter()
                .enumerate()
                .map(|(i, tool)| {
                    let expanded = state.expanded_tool == Some(i);
                    tool_card(i, tool, expanded, &areas, lang)
                })
                .collect();
            column(cards).spacing(8).boxed()
        };

    let action_row = super::save_action_row(
        state,
        SettingsTab::CustomTools,
        SettingsEvent::CustomTools(CustomToolsEvent::SaveTools),
    );
    let mut page = column![header, body].spacing(12);
    if let Some(message) = state.tool_form_error.as_deref() {
        page = page.push(text(message).size(12).color(color_danger_text()).boxed());
    }
    page.push(action_row).boxed()
}

// ── Tool card ─────────────────────────────────────────────────────

/// Multi-line buffers shared by the expanded tool's form.
struct ToolAreas<'a> {
    description: &'a TextArea,
    instruction: &'a TextArea,
    body: &'a TextArea,
    response_template: &'a TextArea,
}

/// Collapsible card: name + summary; the edit form appears when expanded.
fn tool_card<'a>(
    index: usize,
    tool: &'a CustomTool,
    expanded: bool,
    areas: &ToolAreas<'a>,
    lang: Lang,
) -> Element<'a, SettingsEvent> {
    let display_name = if tool.name.trim().is_empty() {
        lang.tr("untitled")
    } else {
        &tool.name
    };
    let summary = match &tool.kind {
        ToolKind::Command(_) => count_label(lang, tool.parameters.len(), lang.tr("parameter")),
        ToolKind::Http(request) => format!(
            "HTTP {} · {}",
            request.method,
            count_label(lang, tool.parameters.len(), lang.tr("parameter"))
        ),
    };

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
        column![header_row, card_rule(), tool_form(index, tool, areas, lang)].spacing(10)
    } else {
        column![header_row]
    })
    .padding([10, 12])
    .style(form_card_style)
    .width(Length::Fill)
    .boxed()
}

// ── Edit form ─────────────────────────────────────────────────────

fn tool_form<'a>(
    index: usize,
    tool: &'a CustomTool,
    areas: &ToolAreas<'a>,
    lang: Lang,
) -> Element<'a, SettingsEvent> {
    let request_form: Element<'a, SettingsEvent> = match &tool.kind {
        ToolKind::Command(command) => command_form(index, command, lang),
        ToolKind::Http(request) => {
            http_form(index, request, areas.body, areas.response_template, lang)
        }
    };

    column![
        field_row(
            lang.tr("Name"),
            &tool.name,
            lang.tr("snake_case name used by the model"),
            false,
            None,
            None,
            move |v| SettingsEvent::CustomTools(CustomToolsEvent::EditToolName(index, v))
        ),
        tool_kind_row(index, tool.kind.mode(), lang),
        textarea_field_row(
            lang.tr("Description"),
            areas.description,
            lang.tr("What the tool does — shown to the model"),
            64.0,
            false,
            move |msg| {
                SettingsEvent::CustomTools(CustomToolsEvent::ToolTextArea(
                    ToolTextField::Description,
                    msg,
                ))
            },
        ),
        textarea_field_row(
            lang.tr("Instruction"),
            areas.instruction,
            lang.tr("When and how the model should use this tool"),
            64.0,
            false,
            move |msg| {
                SettingsEvent::CustomTools(CustomToolsEvent::ToolTextArea(
                    ToolTextField::Instruction,
                    msg,
                ))
            },
        ),
        request_form,
        params_section(index, tool, lang),
    ]
    .spacing(8)
    .boxed()
}

/// Type selector row: CLI command or HTTP API request.
fn tool_kind_row(index: usize, mode: ToolMode, lang: Lang) -> Element<'static, SettingsEvent> {
    let command = lang.tr("Command").to_string();
    let http = lang.tr("HTTP API").to_string();
    let options = vec![command.clone(), http.clone()];
    let selected = match mode {
        ToolMode::Command => command,
        ToolMode::Http => http.clone(),
    };
    let picker = styled_pick_list(options, Some(selected), move |label: String| {
        let mode = if label == http {
            ToolMode::Http
        } else {
            ToolMode::Command
        };
        SettingsEvent::CustomTools(CustomToolsEvent::SetToolKind(index, mode))
    })
    .text_size(12)
    .width(Length::Fixed(160.0));
    row![label_col(lang.tr("Type")), picker]
        .spacing(10)
        .align_y(Alignment::Center)
        .boxed()
}

fn command_form<'a>(index: usize, command: &'a str, lang: Lang) -> Element<'a, SettingsEvent> {
    column![
        field_row(
            lang.tr("Command"),
            command,
            lang.tr("Command template, e.g. git log {args}"),
            true,
            None,
            None,
            move |v| SettingsEvent::CustomTools(CustomToolsEvent::EditToolCommand(index, v)),
        ),
        hint(lang.tr(
            "Command uses TinyTemplate syntax: {param} inserts a value, \
              {{ if param }}…{{ endif }} adds conditional arguments.",
        )),
    ]
    .spacing(8)
    .boxed()
}

fn http_form<'a>(
    index: usize,
    request: &'a HttpRequest,
    body_area: &'a TextArea,
    template_area: &'a TextArea,
    lang: Lang,
) -> Element<'a, SettingsEvent> {
    let braces_hint = lang.tr("JSON braces are written normally; optional fields can use \
          {{ if param }}…{{ endif }}.");
    let method_row = row![
        label_col(lang.tr("Method")),
        styled_pick_list(HttpMethod::ALL, Some(request.method), move |method| {
            SettingsEvent::CustomTools(CustomToolsEvent::EditToolMethod(index, method))
        })
        .text_size(12)
        .width(Length::Fixed(110.0)),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .boxed();

    column![
        method_row,
        field_row(
            lang.tr("Request URL"),
            &request.url,
            "https://api.example.com/v1/search",
            true,
            None,
            None,
            move |v| SettingsEvent::CustomTools(CustomToolsEvent::EditToolUrl(index, v)),
        ),
        headers_section(index, request, lang),
        textarea_field_row(
            lang.tr("Body"),
            body_area,
            lang.tr("JSON or text; {param} inserts a value"),
            120.0,
            true,
            move |msg| {
                SettingsEvent::CustomTools(CustomToolsEvent::ToolTextArea(ToolTextField::Body, msg))
            },
        ),
        textarea_field_row(
            lang.tr("Response Template"),
            template_area,
            lang.tr("Optional TinyTemplate for the JSON response"),
            100.0,
            true,
            move |msg| {
                SettingsEvent::CustomTools(CustomToolsEvent::ToolTextArea(
                    ToolTextField::ResponseTemplate,
                    msg,
                ))
            },
        ),
        hint(lang.tr(
            "HTTP request uses TinyTemplate syntax: {param} inserts a value \
              (percent-encoded in the URL, type-preserving in a JSON body), and \
              {env.VAR} inserts an environment variable.",
        )),
        hint(braces_hint),
        hint(lang.tr(
            "Response Template is applied to the JSON response: use {response.field}, \
              {body}, {status}, or {{ for item in response.results }}…{{ endfor }}. \
              Leave empty to return the raw body.",
        )),
    ]
    .spacing(8)
    .boxed()
}

// ── Headers ───────────────────────────────────────────────────────

fn headers_section<'a>(
    tool_index: usize,
    request: &'a HttpRequest,
    lang: Lang,
) -> Element<'a, SettingsEvent> {
    let rows: Vec<Element<'a, SettingsEvent>> = request
        .headers
        .iter()
        .enumerate()
        .map(|(header_index, header)| header_row(tool_index, header_index, header, lang))
        .collect();
    add_section(
        lang,
        lang.tr("Headers"),
        SettingsEvent::CustomTools(CustomToolsEvent::AddToolHeader(tool_index)),
        rows,
    )
}

/// One request header: name, value, remove.
fn header_row<'a>(
    tool_index: usize,
    header_index: usize,
    header: &'a HttpHeader,
    lang: Lang,
) -> Element<'a, SettingsEvent> {
    let name = text_input(lang.tr("Header name"), &header.name)
        .on_input(move |v| {
            SettingsEvent::CustomTools(CustomToolsEvent::EditToolHeaderName(
                tool_index,
                header_index,
                v,
            ))
        })
        .padding(4)
        .size(13)
        .width(Length::FillPortion(2));
    let value = text_input(lang.tr("Header value"), &header.value)
        .on_input(move |v| {
            SettingsEvent::CustomTools(CustomToolsEvent::EditToolHeaderValue(
                tool_index,
                header_index,
                v,
            ))
        })
        .padding(4)
        .size(13)
        .width(Length::FillPortion(3));
    let remove = button(text("✕").size(10))
        .padding([2, 6])
        .style(delete_button_style)
        .on_press(SettingsEvent::CustomTools(
            CustomToolsEvent::DeleteToolHeader(tool_index, header_index),
        ));
    container(
        row![name, value, remove]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding(8)
    .style(sub_card_style)
    .width(Length::Fill)
    .boxed()
}

// ── Parameters ────────────────────────────────────────────────────

fn params_section<'a>(
    tool_index: usize,
    tool: &'a CustomTool,
    lang: Lang,
) -> Element<'a, SettingsEvent> {
    let cards: Vec<Element<'a, SettingsEvent>> = tool
        .parameters
        .iter()
        .enumerate()
        .map(|(i, param)| param_card(tool_index, i, param, lang))
        .collect();

    add_section(
        lang,
        lang.tr("Parameters"),
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
    lang: Lang,
) -> Element<'a, SettingsEvent> {
    let kind_picker = styled_pick_list(PARAM_KINDS, simple_kind(&param.kind), move |kind| {
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
        .label(lang.tr("required"))
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
                text_input(lang.tr("Parameter name"), &param.name)
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
                lang.tr("Parameter description — shown to the model"),
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
    .boxed()
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
    // A new edit clears the previous save error; SaveTools sets it again on failure.
    state.tool_form_error = None;
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
                ..CustomTool::default()
            });
            state.tool_drafts.push(ToolDraft::default());
            state.expanded_tool = Some(state.working_tools.custom_tools.len() - 1);
            state.init_tool_text_areas();
        }
        CustomToolsEvent::DeleteTool(index) => {
            state.flush_tool_text_areas();
            if index < state.working_tools.custom_tools.len() {
                state.working_tools.custom_tools.remove(index);
                if index < state.tool_drafts.len() {
                    state.tool_drafts.remove(index);
                }
            }
            state.expanded_tool = remove_expanded(state.expanded_tool, index);
            state.init_tool_text_areas();
        }
        CustomToolsEvent::EditToolName(index, v) => {
            if let Some(t) = state.tool_mut(index) {
                t.name = v;
            }
        }
        CustomToolsEvent::SetToolKind(index, mode) => state.set_tool_kind(index, mode),
        CustomToolsEvent::EditToolCommand(index, v) => {
            if let Some(t) = state.tool_mut(index)
                && let ToolKind::Command(command) = &mut t.kind
            {
                *command = v;
            }
        }
        CustomToolsEvent::EditToolMethod(index, method) => {
            if let Some(http) = state.http_mut(index) {
                http.method = method;
            }
        }
        CustomToolsEvent::EditToolUrl(index, v) => {
            if let Some(http) = state.http_mut(index) {
                http.url = v;
            }
        }
        CustomToolsEvent::AddToolHeader(index) => {
            if let Some(http) = state.http_mut(index) {
                http.headers.push(HttpHeader::default());
            }
        }
        CustomToolsEvent::DeleteToolHeader(tool_index, index) => {
            if let Some(http) = state.http_mut(tool_index)
                && index < http.headers.len()
            {
                http.headers.remove(index);
            }
        }
        CustomToolsEvent::EditToolHeaderName(tool_index, index, v) => {
            if let Some(header) = state.header_mut(tool_index, index) {
                header.name = v;
            }
        }
        CustomToolsEvent::EditToolHeaderValue(tool_index, index, v) => {
            if let Some(header) = state.header_mut(tool_index, index) {
                header.value = v;
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
            ToolTextField::Body => state.tool_body_area.update(msg, false),
            ToolTextField::ResponseTemplate => state.tool_response_template_area.update(msg, false),
        },
        CustomToolsEvent::SaveTools => match save_tools(state) {
            Ok(()) => state.save_feedback = Some(SettingsTab::CustomTools),
            Err(error) => state.tool_form_error = Some(error),
        },
    }
}

/// Validate a normalized clone, leaving the working copy untouched on error.
fn save_tools(state: &mut SettingsState) -> Result<(), String> {
    state.flush_tool_text_areas();

    // Normalize a clone so failed validation cannot discard working edits.
    let mut normalized = state.working_tools.clone();
    for tool in &mut normalized.custom_tools {
        tool.name = tool.name.trim().to_string();
        if let ToolKind::Http(request) = &mut tool.kind {
            request.url = request.url.trim().to_string();
            for header in &mut request.headers {
                header.name = header.name.trim().to_string();
            }
        }
    }

    for tool in normalized
        .custom_tools
        .iter()
        .filter(|tool| !tool.name.is_empty())
    {
        tool.validate()?;
    }

    // Retain after validation so a header value without a name reports an error.
    for tool in &mut normalized.custom_tools {
        if let ToolKind::Http(request) = &mut tool.kind {
            request.headers.retain(|header| !header.name.is_empty());
        }
    }
    let dropped: Vec<usize> = normalized
        .custom_tools
        .iter()
        .enumerate()
        .filter_map(|(index, tool)| tool.name.is_empty().then_some(index))
        .collect();
    normalized.custom_tools.retain(|tool| !tool.name.is_empty());
    for &index in dropped.iter().rev() {
        if index < state.tool_drafts.len() {
            state.tool_drafts.remove(index);
        }
    }
    state.working_tools = normalized;
    if let Some(expanded) = state.expanded_tool {
        if dropped.contains(&expanded) {
            state.expanded_tool = None;
            state.init_tool_text_areas();
        } else {
            state.expanded_tool =
                Some(expanded - dropped.iter().filter(|&&index| index < expanded).count());
        }
    }
    Ok(())
}
