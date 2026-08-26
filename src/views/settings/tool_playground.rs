use iced::{
    Alignment, Border, Color, Element, Length,
    widget::{
        button, checkbox, column, container, row, scrollable, text, text::Wrapping, text_input,
    },
};

use iced_selection::Text as SelectableText;

use crate::tools::{Tool, ToolRegistry};
use crate::views::styles::sel_default;
use crate::views::theme::{
    CRABOT_DANGER, CRABOT_PRIMARY, color_border, color_card, color_muted, color_text_strong,
    is_dark,
};
use crate::views::{primary_button, secondary_button};
use crate::widgets::dropdown::DropDown;

use super::{SettingsEvent, SettingsState, form_card_style};

// ── Events ──────────────────────────────────────────────────────────

/// Raw in-progress array-item input text, keyed by `(param name, item index)`.
/// Keeps inputs showing exactly what was typed while the coerced value lives
/// in [`SettingsState::playground_param_values`].
pub(crate) type ArrayDrafts = std::collections::HashMap<(String, usize), String>;

/// Events for the Tool Playground tab.
#[derive(Debug, Clone)]
pub(crate) enum PlaygroundEvent {
    /// Select a tool by index in the playground list.  `None` clears the selection.
    SelectPlaygroundTool(Option<usize>),
    /// Edit a parameter value in the playground form.
    EditPlaygroundParam(String, String),
    /// Add an empty item to an array parameter (param name, optional item type).
    AddPlaygroundArrayItem(String, Option<String>),
    /// Remove an item from an array parameter by index.
    RemovePlaygroundArrayItem(String, usize),
    /// Edit an item of an array parameter (name, index, value, items_type).
    EditPlaygroundArrayItem(String, usize, String, Option<String>),
    /// Execute the selected playground tool with the given JSON args.
    ExecutePlaygroundTool,
    /// Cancel the in-flight playground execution.
    CancelPlaygroundTool,
    /// Result of a playground tool execution (generation, result, is_todo).
    PlaygroundToolResult(u64, Result<String, String>, bool),
}

/// Snapshot of a tool for display in the playground picker.
#[derive(Debug, Clone)]
pub(crate) struct ToolInfo {
    pub name: String,
    pub description: String,
    /// Raw JSON Schema value for parameter extraction.
    pub schema_raw: serde_json::Value,
    /// Group label: "Builtin", "Custom", or "MCP: <server>".
    pub group: String,
}

// ── Dropdown entries ────────────────────────────────────────────────

/// An entry in the [`DropDown`] tool list: either a category header or a tool.
#[derive(Debug, Clone)]
enum SelectorEntry {
    Header(String),
    Tool(usize, String),
}

impl SelectorEntry {
    fn is_header(&self) -> bool {
        matches!(self, SelectorEntry::Header(_))
    }
}

impl std::fmt::Display for SelectorEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectorEntry::Header(label) => write!(f, "{label}"),
            SelectorEntry::Tool(_idx, name) => write!(f, "{name}"),
        }
    }
}

impl PartialEq for SelectorEntry {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (SelectorEntry::Header(a), SelectorEntry::Header(b)) => a == b,
            (SelectorEntry::Tool(ai, an), SelectorEntry::Tool(bi, bn)) => ai == bi && an == bn,
            _ => false,
        }
    }
}

/// Build a flat list of [`SelectorEntry`] from the sorted tool list,
/// inserting group headers whenever the group changes.
fn build_selector_entries(tools: &[ToolInfo]) -> Vec<SelectorEntry> {
    let mut entries: Vec<SelectorEntry> = Vec::new();
    let mut last_group: Option<&str> = None;

    for (i, tool) in tools.iter().enumerate() {
        if last_group.is_none_or(|g| g != tool.group) {
            entries.push(SelectorEntry::Header(tool.group.clone()));
            last_group = Some(&tool.group);
        }
        entries.push(SelectorEntry::Tool(i, tool.name.clone()));
    }

    entries
}

// ── Parameter definitions ──────────────────────────────────────────

/// A parameter extracted from a tool's JSON Schema.
#[derive(Debug, Clone)]
struct ParamDef {
    name: String,
    /// JSON type: "string", "integer", "number", "boolean", "array", "object"
    param_type: String,
    /// For array types, the JSON type of items (e.g. "string", "object").
    items_type: Option<String>,
    description: String,
    required: bool,
    /// Default value from the schema, if any.
    default: Option<serde_json::Value>,
}

/// Extract top-level parameter definitions from a JSON Schema object.
fn extract_params(schema: &serde_json::Value) -> Vec<ParamDef> {
    let obj = match schema.as_object() {
        Some(o) => o,
        None => return vec![],
    };

    let properties = match obj.get("properties").and_then(|v| v.as_object()) {
        Some(props) => props,
        None => return vec![],
    };

    let required: Vec<&str> = obj
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    properties
        .iter()
        .map(|(name, prop)| {
            let param_type = prop
                .get("type")
                .and_then(|v| {
                    // Handle "type": ["string", "null"] union types
                    if let Some(arr) = v.as_array() {
                        arr.iter()
                            .find_map(|t| t.as_str().filter(|s| *s != "null"))
                            .or_else(|| v.as_str())
                    } else {
                        v.as_str()
                    }
                })
                .unwrap_or("string")
                .to_string();

            let description = prop
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let items_type = if param_type == "array" {
                prop.get("items")
                    .and_then(|items| items.get("type"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            };

            let required = required.contains(&name.as_str());

            let default = prop.get("default").cloned();

            ParamDef {
                name: name.clone(),
                param_type,
                items_type,
                description,
                required,
                default,
            }
        })
        .collect()
}

// ── Helpers ────────────────────────────────────────────────────────

/// If the string is valid JSON, return it pretty-printed; otherwise return as-is.
fn pretty_json_or_raw(s: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(s) {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| s.to_string())
    } else {
        s.to_string()
    }
}

/// Build a JSON object of call arguments from parameter form values, guided by
/// the tool's JSON Schema so that every value is coerced to the correct type.
pub(crate) fn build_params_json(
    schema: &serde_json::Value,
    param_values: &std::collections::HashMap<String, String>,
) -> serde_json::Value {
    let params = extract_params(schema);
    let mut args_map = serde_json::Map::new();

    for p in &params {
        let raw = param_values.get(&p.name).map(|s| s.as_str()).unwrap_or("");

        // Skip empty optional parameters that have no default.
        if raw.is_empty() && !p.required && p.default.is_none() {
            continue;
        }

        let val = if raw.is_empty() {
            // Use schema default when available.
            p.default.clone().unwrap_or_else(|| {
                if p.param_type == "boolean" {
                    serde_json::Value::Bool(false)
                } else {
                    serde_json::Value::Null
                }
            })
        } else {
            coerce_value(raw, &p.param_type)
        };

        args_map.insert(p.name.clone(), val);
    }

    serde_json::Value::Object(args_map)
}

/// Parse `raw` into a [`serde_json::Value`] whose variant matches `param_type`.
pub(crate) fn coerce_value(raw: &str, param_type: &str) -> serde_json::Value {
    match param_type {
        "boolean" => match raw.to_lowercase().as_str() {
            "true" | "1" | "yes" => serde_json::Value::Bool(true),
            "false" | "0" | "no" | "" => serde_json::Value::Bool(false),
            _ => {
                // Fall back to JSON parse; if that fails keep as string.
                serde_json::from_str(raw)
                    .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
            }
        },
        "integer" => {
            // Try JSON first (handles negative / large numbers), then direct parse.
            let parsed = serde_json::from_str::<serde_json::Value>(raw).ok();
            let as_int = parsed.as_ref().and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
                _ => None,
            });
            as_int
                .or_else(|| raw.parse::<i64>().ok())
                .map(|i| serde_json::Value::Number(i.into()))
                .unwrap_or_else(|| serde_json::Value::String(raw.to_string()))
        }
        "number" => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(serde_json::Value::Number(n)) => serde_json::Value::Number(n),
            _ => raw
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .unwrap_or_else(|| serde_json::Value::String(raw.to_string())),
        },
        "array" | "object" => {
            serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
        }
        // "string" and anything else: keep as a JSON string.
        _ => serde_json::Value::String(raw.to_string()),
    }
}

// ── Parameter field rendering ───────────────────────────────────

fn render_param_field<'a>(
    p: &ParamDef,
    current_value: &str,
    array_drafts: &ArrayDrafts,
) -> Element<'a, SettingsEvent> {
    match p.param_type.as_str() {
        "boolean" => {
            let checked = current_value == "true";
            let name = p.name.clone();
            checkbox(checked)
                .label(p.name.clone())
                .text_size(13)
                .on_toggle(move |v| {
                    SettingsEvent::Playground(PlaygroundEvent::EditPlaygroundParam(
                        name.clone(),
                        v.to_string(),
                    ))
                })
                .into()
        }
        "array" => {
            let items: Vec<String> = serde_json::from_str(current_value)
                .ok()
                .map(|arr: Vec<serde_json::Value>| {
                    arr.iter()
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => {
                                serde_json::to_string(&other).unwrap_or_else(|_| other.to_string())
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let items_type = p.items_type.clone();
            let is_object_item = items_type.as_deref() == Some("object");

            let name = p.name.clone();
            let mut item_rows: Vec<Element<'_, SettingsEvent>> = Vec::new();
            for (i, item_text) in items.iter().enumerate() {
                let del_name = name.clone();
                let edit_name = name.clone();
                let edit_itype = items_type.clone();
                // Show raw draft if present so partially-typed values aren't
                // re-serialized with quotes.
                let display = array_drafts
                    .get(&(name.clone(), i))
                    .cloned()
                    .unwrap_or_else(|| item_text.clone());
                let mut input = text_input("", &display)
                    .on_input(move |v| {
                        SettingsEvent::Playground(PlaygroundEvent::EditPlaygroundArrayItem(
                            edit_name.clone(),
                            i,
                            v,
                            edit_itype.clone(),
                        ))
                    })
                    .padding(4)
                    .size(13)
                    .width(Length::Fill);
                if is_object_item {
                    input = input.font(iced::Font::MONOSPACE);
                }
                item_rows.push(
                    row![
                        input,
                        button(text("×").size(13).color(CRABOT_DANGER).font(super::BOLD),)
                            .style(secondary_button)
                            .padding([2, 6])
                            .on_press(SettingsEvent::Playground(
                                PlaygroundEvent::RemovePlaygroundArrayItem(del_name.clone(), i,),
                            )),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center)
                    .into(),
                );
            }

            let add_name = name.clone();
            let add_btn = button(text("＋ Add item").size(12).color(color_muted()))
                .style(secondary_button)
                .padding([2, 8])
                .on_press(SettingsEvent::Playground(
                    PlaygroundEvent::AddPlaygroundArrayItem(add_name, items_type),
                ));

            column(item_rows).spacing(4).push(add_btn).into()
        }
        _ => {
            let placeholder = if p.required { "required" } else { "optional" };
            let name = p.name.clone();
            let is_mono = p.param_type == "object";
            let mut input = text_input(placeholder, current_value)
                .on_input(move |v| {
                    SettingsEvent::Playground(PlaygroundEvent::EditPlaygroundParam(name.clone(), v))
                })
                .padding(4)
                .size(13)
                .width(Length::Fill);
            if is_mono {
                input = input.font(iced::Font::MONOSPACE);
            }
            Element::from(input)
        }
    }
}

// ── Result widget ───────────────────────────────────────────────

/// Monospace scrollable result text.
fn result_text(output: String) -> Element<'static, SettingsEvent> {
    scrollable(
        SelectableText::new(output)
            .size(12)
            .font(iced::Font::MONOSPACE)
            .style(sel_default),
    )
    .width(Length::Fill)
    .height(Length::Fixed(160.0))
    .into()
}

/// Box with a colored border and a theme-aware background.
fn result_box<'a>(
    content: impl Into<Element<'a, SettingsEvent>>,
    border: Color,
    dark: Color,
    light: Color,
) -> Element<'a, SettingsEvent> {
    container(content)
        .style(move |_: &iced::Theme| container::Style {
            background: Some(if is_dark() { dark } else { light }.into()),
            border: Border::default().rounded(6).width(1).color(border),
            ..container::Style::default()
        })
        .padding(8)
        .width(Length::Fill)
        .into()
}

fn render_result_widget<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    match &state.playground_result {
        Some(Ok(output)) => result_box(
            result_text(pretty_json_or_raw(output)),
            Color::from_rgb8(0x4C, 0xAF, 0x50),
            Color::from_rgb8(0x1B, 0x2E, 0x24),
            Color::from_rgb8(0xF0, 0xFF, 0xF0),
        ),
        Some(Err(err)) => result_box(
            result_text(err.clone()),
            CRABOT_DANGER,
            Color::from_rgb8(0x33, 0x20, 0x20),
            Color::from_rgb8(0xFF, 0xF0, 0xF0),
        ),
        None => result_box(
            text(if state.playground_running {
                "Executing…"
            } else {
                "Result will appear here."
            })
            .size(13)
            .color(color_muted()),
            color_border(),
            color_card(),
            color_card(),
        ),
    }
}

// ── View ────────────────────────────────────────────────────────────

pub(crate) fn playground_page<'a>(state: &'a SettingsState) -> Element<'a, SettingsEvent> {
    // Build selector entries with group headers.
    let entries = build_selector_entries(&state.playground_tools);

    // Determine the currently selected entry (for the trigger display).
    let selected_entry = state
        .playground_selected_index
        .and_then(|idx| {
            entries
                .iter()
                .find(|e| matches!(e, SelectorEntry::Tool(i, _) if *i == idx))
        })
        .cloned();

    // Build a parallel array of header flags for the item_is_header predicate.
    let header_flags: Vec<bool> = entries.iter().map(|e| e.is_header()).collect();

    // Tool selector dropdown with treeview-style overlay.
    let selector: Element<_> = DropDown::new(entries, selected_entry, move |entry| {
        if let SelectorEntry::Tool(idx, _) = entry {
            SettingsEvent::Playground(PlaygroundEvent::SelectPlaygroundTool(Some(idx)))
        } else {
            // Headers are never selectable, this branch is a defensive fallback.
            SettingsEvent::Playground(PlaygroundEvent::SelectPlaygroundTool(None))
        }
    })
    .width(Length::Fill)
    .placeholder("Choose a tool to test…")
    .text_size(13)
    .menu_width(300.0)
    .item_is_header(move |i| header_flags.get(i).copied().unwrap_or(false))
    .item_indent(16.0)
    .header_font(super::BOLD)
    .into();

    let selected_tool = state
        .playground_selected_index
        .and_then(|i| state.playground_tools.get(i));

    let tool_detail: Element<_> = if let Some(info) = selected_tool {
        // Parse parameter definitions from schema.
        let params = extract_params(&info.schema_raw);

        // Tool description
        let desc = if info.description.is_empty() {
            text("(no description)").size(13).color(color_muted())
        } else {
            text(info.description.clone())
                .size(13)
                .color(color_text_strong())
                .wrapping(Wrapping::Word)
        };

        let desc_row = row![
            text("Description:").size(13).color(color_muted()).width(80),
            desc,
        ]
        .spacing(8)
        .align_y(Alignment::Start);

        // Parameter form fields
        let param_section: Element<_> = if params.is_empty() {
            container(
                text("This tool takes no parameters.")
                    .size(13)
                    .color(color_muted()),
            )
            .style(form_card_style)
            .padding(12)
            .width(Length::Fill)
            .into()
        } else {
            let param_label = text("Parameters:").size(13).color(color_text_strong());

            let fields: Vec<Element<'_, SettingsEvent>> = params
                .iter()
                .map(|p| {
                    let p_name = p.name.clone();
                    let p_type = p.param_type.clone();
                    let p_desc = p.description.clone();

                    let current_value = state
                        .playground_param_values
                        .get(&p_name)
                        .cloned()
                        .unwrap_or_default();

                    let field =
                        render_param_field(p, &current_value, &state.playground_array_drafts);

                    let type_badge = text(format!("[{}]", p_type)).size(11).color(color_muted());

                    let desc_text = if p_desc.is_empty() {
                        None
                    } else {
                        Some(
                            text(p_desc)
                                .size(11)
                                .color(color_muted())
                                .wrapping(Wrapping::Word),
                        )
                    };

                    let mut col = column![
                        row![
                            text(p_name).size(13).color(color_text_strong()).width(120),
                            type_badge,
                        ]
                        .spacing(4)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(2);

                    if let Some(d) = desc_text {
                        col = col.push(d);
                    }
                    col = col.push(field);

                    container(col).padding(8).width(Length::Fill).into()
                })
                .collect();

            column![param_label, column(fields).spacing(6)]
                .spacing(8)
                .into()
        };

        // Execute / Cancel buttons
        let exec_row: Element<_> = if state.playground_running {
            row![
                button(text("⏳ Running…").size(13)).style(secondary_button),
                button(text("✕ Cancel").size(13))
                    .style(secondary_button)
                    .on_press(SettingsEvent::Playground(
                        PlaygroundEvent::CancelPlaygroundTool,
                    )),
            ]
            .spacing(8)
            .into()
        } else {
            row![
                button(text("▶ Execute").size(13))
                    .style(primary_button)
                    .on_press(SettingsEvent::Playground(
                        PlaygroundEvent::ExecutePlaygroundTool,
                    )),
                iced::widget::Space::new().width(Length::Fill),
            ]
            .width(Length::Fill)
            .into()
        };

        // Result area
        let result_widget = render_result_widget(state);

        let result_label = text("Result:").size(13).color(color_text_strong());

        column![
            desc_row,
            param_section,
            exec_row,
            result_label,
            result_widget,
        ]
        .spacing(10)
        .into()
    } else {
        container(
            text("Select a tool from the list above to see its description and test it with custom arguments.")
                .size(13)
                .color(color_muted())
                .wrapping(Wrapping::Word),
        )
        .style(form_card_style)
        .padding(16)
        .width(Length::Fill)
        .into()
    };

    column![
        text("Tool Playground")
            .size(16)
            .font(super::BOLD)
            .color(CRABOT_PRIMARY),
        text("Select a tool, fill in parameters, and execute it directly.")
            .size(12)
            .color(color_muted()),
        selector,
        tool_detail,
    ]
    .spacing(12)
    .into()
}

/// Build [`ToolInfo`] snapshots from the live tool registry for the playground.
pub(crate) fn build_playground_tools(registry: &ToolRegistry) -> Vec<ToolInfo> {
    let mut list = Vec::new();

    // Builtin tools
    for tool in &registry.builtin {
        let schema = tool.schema();
        list.push(ToolInfo {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            schema_raw: schema,
            group: "Builtin".to_string(),
        });
    }

    // Custom tools
    for tool in &registry.custom {
        let schema = tool.schema();
        list.push(ToolInfo {
            name: tool.name.clone(),
            description: tool.description.clone(),
            schema_raw: schema,
            group: "Custom".to_string(),
        });
    }

    // MCP tools
    for (server, tools) in &registry.mcp {
        for tool in tools {
            let schema = tool.schema();
            list.push(ToolInfo {
                name: tool.name.clone(),
                description: tool.description.clone(),
                schema_raw: schema,
                group: format!("MCP: {server}"),
            });
        }
    }

    // Sort: builtin first, then custom, then MCP; alphabetically within each.
    list.sort_by(|a, b| {
        let group_order = |g: &str| match g {
            "Builtin" => 0,
            "Custom" => 1,
            _ => 2,
        };
        group_order(&a.group)
            .cmp(&group_order(&b.group))
            .then_with(|| a.name.cmp(&b.name))
    });

    list
}

// ── Update ─────────────────────────────────────────────────────────

/// Handle a Tool Playground tab event, mutating the playground state.
pub(super) fn update(state: &mut SettingsState, event: PlaygroundEvent) {
    match event {
        PlaygroundEvent::SelectPlaygroundTool(selected) => {
            if let Some(index) = selected
                && index < state.playground_tools.len()
            {
                state.playground_selected_index = Some(index);
                // Reset param values when switching tools.
                state.playground_param_values.clear();
                state.playground_array_drafts.clear();
                state.playground_result = None;
                state.playground_running = false;
            }
        }
        PlaygroundEvent::EditPlaygroundParam(name, value) => {
            state.playground_param_values.insert(name, value);
        }
        PlaygroundEvent::AddPlaygroundArrayItem(name, items_type) => {
            let default = match items_type.as_deref() {
                Some("object") => serde_json::Value::Object(serde_json::Map::new()),
                Some("boolean") => serde_json::Value::Bool(false),
                Some("number") | Some("integer") => serde_json::Value::Number(0_i32.into()),
                _ => serde_json::Value::String(String::new()),
            };
            edit_array_items(state, &name, |items| items.push(default));
        }
        PlaygroundEvent::RemovePlaygroundArrayItem(name, index) => {
            edit_array_items(state, &name, |items| {
                if index < items.len() {
                    items.remove(index);
                }
            });
            state.clear_array_drafts(&name);
        }
        PlaygroundEvent::EditPlaygroundArrayItem(name, index, value, items_type) => {
            // Remember raw text so the input shows exactly what was typed.
            state
                .playground_array_drafts
                .insert((name.clone(), index), value.clone());
            edit_array_items(state, &name, |items| {
                if index < items.len() {
                    items[index] = coerce_value(&value, items_type.as_deref().unwrap_or("string"));
                }
            });
        }
        PlaygroundEvent::ExecutePlaygroundTool => {
            // Cancel any in-flight execution before starting a new one.
            state.reset_playground_cancel();
            state.playground_running = true;
            state.playground_result = None;
            // Bump generation so stale results from previous runs are ignored.
            state.playground_generation = state.playground_generation.wrapping_add(1);
        }
        PlaygroundEvent::CancelPlaygroundTool => {
            state.playground_cancel.cancel();
        }
        PlaygroundEvent::PlaygroundToolResult(generation, result, _is_todo) => {
            if state.playground_generation == generation {
                state.playground_running = false;
                state.playground_result = Some(result);
            }
        }
    }
}

/// Load the JSON array stored under `name`, mutate it, and write it back.
fn edit_array_items(
    state: &mut SettingsState,
    name: &str,
    f: impl FnOnce(&mut Vec<serde_json::Value>),
) {
    let mut items: Vec<serde_json::Value> = state
        .playground_param_values
        .get(name)
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default();
    f(&mut items);
    if let Ok(json) = serde_json::to_string(&items) {
        state.playground_param_values.insert(name.to_string(), json);
    }
}
