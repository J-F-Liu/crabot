use iced::{
    Alignment, Border, Color, Element, Fill, Font, Theme, font, padding,
    widget::{Space, column, container, rich_text, row, span, text, text::Wrapping},
};
use iced_selection::Text as SelectableText;
use iced_selection::text::Style as SelectionStyle;

use super::ASK_INPUT;
use super::session_state::{AskAction, AskRequest};
use super::styles::{primary_button, secondary_button};
use super::styles::{sel_default, sel_primary, sel_secondary};
use super::theme::{
    CRABOT_DANGER, CRABOT_SUCCESS, CRABOT_TEXT_MUTED, CRABOT_TOOL_ACCENT, CRABOT_TOOL_CONTENT_BG,
    CRABOT_TOOL_CONTENT_BORDER, color_text,
};
use crate::Message;
use crate::tools::edit::EditParam;
use crate::tools::todo::{TodoItem, TodoStatus};
use iced::widget::{button, text_input};

/// Shared container style for ask tool views (active and completed).
fn ask_tool_container(content: impl Into<Element<'static, Message>>) -> Element<'static, Message> {
    container(content.into())
        .padding([10, 14])
        .style(|_theme: &Theme| container::Style {
            background: Some(CRABOT_TOOL_CONTENT_BG.into()),
            border: Border {
                color: CRABOT_TOOL_CONTENT_BORDER,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .width(Fill)
        .into()
}

/// Render a list of options with checkmarks.
/// In interactive mode each option is a clickable `button`; otherwise
/// options are rendered as read-only `SelectableText`.
fn ask_option_list(
    options: &[String],
    selected: &str,
    font_scale: f32,
    interactive: bool,
) -> Vec<Element<'static, Message>> {
    options
        .iter()
        .map(|option| {
            let is_selected = option == selected;
            let check = if is_selected { "✓" } else { " " };
            let label: Element<'static, Message> = if interactive {
                button(text(option.clone()).size(13.0 * font_scale))
                    .style(secondary_button)
                    .on_press(Message::AskAction(AskAction::OptionSelected(
                        option.clone(),
                    )))
                    .into()
            } else {
                SelectableText::new(option.clone())
                    .size(13.0 * font_scale)
                    .style(sel_default)
                    .into()
            };
            row![
                text(check).width(16.0 * font_scale).size(13.0 * font_scale),
                label,
            ]
            .align_y(Alignment::Center)
            .into()
        })
        .collect()
}

/// Interactive response controls for the builtin ask tool.
pub(crate) fn ask_view(
    request: &AskRequest,
    input: &str,
    font_scale: f32,
) -> Element<'static, Message> {
    let header = text("🤖 LLM asks:").size(13.0).color(CRABOT_TOOL_ACCENT);
    let question: Element<'static, Message> = SelectableText::new(request.question.clone())
        .style(sel_default)
        .into();
    let skip = button(text("Skip"))
        .style(secondary_button)
        .on_press(Message::AskAction(AskAction::Skip));
    let controls: Element<'static, Message> = if request.options.is_empty() {
        row![
            text_input("Type your answer…", input)
                .id(ASK_INPUT.clone())
                .on_input(Message::AskInputChanged)
                .on_submit(Message::AskAction(AskAction::Ok))
                .width(Fill),
            button(text("Ok"))
                .style(primary_button)
                .on_press(Message::AskAction(AskAction::Ok)),
            skip
        ]
        .spacing(8)
        .into()
    } else {
        let ok_enabled = !input.is_empty();
        let options_col =
            column(ask_option_list(&request.options, input, font_scale, true)).spacing(8);
        let action_row = row![
            button(text("Ok"))
                .style(primary_button)
                .on_press_maybe(if ok_enabled {
                    Some(Message::AskAction(AskAction::Ok))
                } else {
                    None
                }),
            skip
        ]
        .spacing(8)
        .padding([4, 16]);
        column![options_col, action_row].spacing(8).into()
    };
    ask_tool_container(column![header, question, controls].spacing(8))
}

/// Completed ask tool result view — shows the question, all options
/// (with the selected one marked ✓), and the answer without interactive
/// controls (those only appear during active asking via [`ask_view`]).
pub(crate) fn ask_result_view(
    args: &serde_json::Value,
    result: &Result<String, String>,
    font_scale: f32,
) -> Element<'static, Message> {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let options: Vec<String> = args
        .get("options")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let (answer, is_ok) = match result {
        Ok(s) => (s.as_str(), true),
        Err(e) => (e.as_str(), false),
    };

    let question_text: Element<'static, Message> = SelectableText::new(question.to_string())
        .style(sel_default)
        .into();

    let answer_label = if is_ok { "Answer" } else { "Error" };
    let answer_color = if is_ok { CRABOT_SUCCESS } else { CRABOT_DANGER };

    let mut answer_col = column![];

    if is_ok && !options.is_empty() {
        let matched = options.iter().any(|opt| opt == answer);
        let option_rows = ask_option_list(&options, answer, font_scale, false);

        if matched {
            answer_col = answer_col
                .push(
                    text(format!("{answer_label}:"))
                        .size(12.0 * font_scale)
                        .color(answer_color)
                        .font(bold_font()),
                )
                .push(column(option_rows).spacing(2));
        } else {
            // When the answer doesn't match any option (e.g. the user skipped),
            // show the actual answer text so it isn't lost.
            answer_col = answer_col
                .push(
                    text("Options:")
                        .size(12.0 * font_scale)
                        .color(answer_color)
                        .font(bold_font()),
                )
                .push(column(option_rows).spacing(2))
                .push(
                    row![
                        text(format!("{answer_label}: "))
                            .size(12.0 * font_scale)
                            .color(answer_color)
                            .font(bold_font()),
                        SelectableText::new(answer.to_string())
                            .size(13.0 * font_scale)
                            .style(sel_default),
                    ]
                    .spacing(4)
                    .padding(padding::top(4)),
                );
        }
    } else {
        let answer_element: Element<'static, Message> = SelectableText::new(answer.to_string())
            .size(13.0 * font_scale)
            .style(sel_default)
            .into();
        answer_col = answer_col.push(
            row![
                text(format!("{answer_label}: "))
                    .size(12.0 * font_scale)
                    .color(answer_color)
                    .font(bold_font()),
                answer_element,
            ]
            .spacing(4),
        );
    }

    ask_tool_container(column![question_text, answer_col].spacing(8))
}

/// Color used for search keyword highlighting within text.
const SEARCH_HIGHLIGHT_BG: Color = Color::from_rgba(1.0, 0.92, 0.0, 0.35);

/// Build a vector of `Span`s from `content` where occurrences of `query` are
/// case-insensitively highlighted. Spans own their text (static lifetime).
pub(super) fn highlighted_spans(
    content: &str,
    query: &str,
) -> Vec<iced::widget::text::Span<'static, (), iced::Font>> {
    if query.trim().is_empty() {
        return vec![span(content.to_string())];
    }

    // Build a case-insensitive literal-match regex.  Escaping prevents the
    // search query from being interpreted as regex syntax.
    let re = match regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    {
        Ok(r) => r,
        Err(_) => return vec![span(content.to_string())],
    };

    let mut spans: Vec<iced::widget::text::Span<'static, (), iced::Font>> = Vec::new();
    let mut last_end = 0;

    for m in re.find_iter(content) {
        let start = m.start();
        let end = m.end();
        if start > last_end {
            spans.push(span(content[last_end..start].to_string()));
        }
        spans.push(span(content[start..end].to_string()).background(SEARCH_HIGHLIGHT_BG));
        last_end = end;
    }

    if last_end < content.len() {
        spans.push(span(content[last_end..].to_string()));
    }

    if spans.is_empty() {
        spans.push(span(content.to_string()));
    }

    spans
}

/// Build text with inline search keyword highlighting.
/// Returns a `rich_text` element when a query is active, or plain `text` otherwise.
pub(super) fn highlighted_text(content: &str, query: &str, size: f32) -> Element<'static, Message> {
    if query.trim().is_empty() {
        return text(content.to_string()).size(size).into();
    }
    rich_text(highlighted_spans(content, query))
        .size(size)
        .into()
}

/// Monospace font stack for paths and code snippets.
fn mono_font() -> Font {
    Font {
        family: font::Family::Monospace,
        ..Font::DEFAULT
    }
}

/// Bold weight version of the default font.
fn bold_font() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }
}

/// Background colours for diff-style rows.
const DIFF_BG_DEL: Color = Color::from_rgb8(0xFF, 0xF0, 0xF0);
const DIFF_BG_ADD: Color = Color::from_rgb8(0xF0, 0xFA, 0xF4);

/// A labelled, colour-coded row used inside the edits table.
///
/// `marker` is the leading glyph (e.g. "−", "+", "⚠"), coloured with
/// `marker_color`. `content` is rendered as selectable monospace text using
/// `sel_style`, all on a `bg` background with rounded corners.
fn diff_row<'a>(
    marker: &'a str,
    marker_color: Color,
    content: String,
    sel_style: fn(&Theme) -> SelectionStyle,
    bg: Color,
    font_scale: f32,
    search_query: &str,
) -> Element<'a, Message> {
    container(
        row![
            text(marker)
                .size(13.0 * font_scale)
                .color(marker_color)
                .font(bold_font()),
            Space::new().width(6),
            if search_query.trim().is_empty() {
                SelectableText::new(content)
                    .size(12.0 * font_scale)
                    .style(sel_style)
                    .font(mono_font())
                    .into()
            } else {
                highlighted_text(&content, search_query, 12.0 * font_scale)
            },
        ]
        .spacing(0),
    )
    .padding([4, 8])
    .width(Fill)
    .style(move |_theme: &Theme| container::Style {
        background: Some(bg.into()),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..container::Style::default()
    })
    .into()
}

/// Single tool-argument key-value row.
pub(super) fn arg_row<'a>(
    key: &'a str,
    value: String,
    font_scale: f32,
    search_query: &str,
) -> Element<'a, Message> {
    row![
        text(format!("{}:", key))
            .size(12.0 * font_scale)
            .color(CRABOT_TOOL_ACCENT)
            .font(bold_font()),
        Space::new().width(8),
        if search_query.trim().is_empty() {
            SelectableText::new(value)
                .size(12.0 * font_scale)
                .style(sel_default)
                .font(mono_font())
                .into()
        } else {
            highlighted_text(&value, search_query, 12.0 * font_scale)
        },
    ]
    .spacing(0)
    .into()
}

/// Embedded table for the `edits` argument — each edit becomes a labelled block.
fn edits_table<'a>(
    key: &'a str,
    edits: &'a [serde_json::Value],
    font_scale: f32,
    search_query: &str,
) -> Element<'a, Message> {
    let header = row![
        text(format!("{}:", key))
            .size(12.0 * font_scale)
            .color(CRABOT_TEXT_MUTED)
            .font(bold_font()),
        Space::new().width(8),
        text(format!("{} edit(s)", edits.len()))
            .size(12.0 * font_scale)
            .color(CRABOT_TEXT_MUTED),
    ]
    .spacing(0);

    let rows: Vec<Element<'_, Message>> = edits
        .iter()
        .enumerate()
        .flat_map(|(i, edit)| {
            let idx = container(
                text(format!("Edit #{}", i + 1))
                    .size(11.0 * font_scale)
                    .color(CRABOT_TEXT_MUTED),
            )
            .padding([2, 0])
            .into();

            let items: Vec<Element<'_, Message>> =
                match serde_json::from_value::<EditParam>(edit.clone()) {
                    Ok(EditParam { old_text, new_text }) => vec![
                        idx,
                        diff_row(
                            "−",
                            CRABOT_DANGER,
                            old_text,
                            sel_secondary,
                            DIFF_BG_DEL,
                            font_scale,
                            search_query,
                        ),
                        diff_row(
                            "+",
                            CRABOT_SUCCESS,
                            new_text,
                            sel_primary,
                            DIFF_BG_ADD,
                            font_scale,
                            search_query,
                        ),
                    ],
                    Err(_) => vec![
                        idx,
                        diff_row(
                            "⚠",
                            CRABOT_DANGER,
                            edit.to_string(),
                            sel_secondary,
                            DIFF_BG_DEL,
                            font_scale,
                            search_query,
                        ),
                    ],
                };
            items
        })
        .collect();

    column![header.width(Fill), column(rows).spacing(4).width(Fill)]
        .spacing(6)
        .width(Fill)
        .into()
}

/// Status colours for todo items.
const TODO_STATUS_PENDING: Color = Color::from_rgb8(0x99, 0x99, 0x99);
const TODO_STATUS_IN_PROGRESS: Color = Color::from_rgb8(0x29, 0x76, 0xFF);
const TODO_STATUS_WIDTH: f32 = 96.0;

fn todo_text_cell(
    content: String,
    font_scale: f32,
    search_query: &str,
) -> Element<'static, Message> {
    if search_query.trim().is_empty() {
        SelectableText::new(content)
            .size(12.0 * font_scale)
            .style(sel_default)
            .font(mono_font())
            .into()
    } else {
        highlighted_text(&content, search_query, 12.0 * font_scale)
    }
}

fn todo_row(
    content: String,
    status: &'static str,
    status_color: Color,
    font_scale: f32,
    search_query: &str,
) -> Element<'static, Message> {
    row![
        container(todo_text_cell(content, font_scale, search_query))
            .width(Fill)
            .padding(2),
        container(
            text(status)
                .size(12.0 * font_scale)
                .color(status_color)
                .font(bold_font())
                .wrapping(Wrapping::None),
        )
        .width(TODO_STATUS_WIDTH)
        .padding(2),
    ]
    .spacing(8)
    .into()
}

fn todo_item_row(
    item: &serde_json::Value,
    font_scale: f32,
    search_query: &str,
) -> Element<'static, Message> {
    match serde_json::from_value::<TodoItem>(item.clone()) {
        Ok(todo) => {
            let (status, color) = match todo.status {
                TodoStatus::Pending => ("pending", TODO_STATUS_PENDING),
                TodoStatus::InProgress => ("in progress", TODO_STATUS_IN_PROGRESS),
                TodoStatus::Completed => ("completed", CRABOT_SUCCESS),
            };
            let content = format!("{}{}", "  ".repeat(todo.depth as usize), todo.text);
            todo_row(content, status, color, font_scale, search_query)
        }
        Err(_) => todo_row(
            item.to_string(),
            "⚠ invalid",
            CRABOT_DANGER,
            font_scale,
            search_query,
        ),
    }
}

/// Embedded table for the `items` argument of the `todo` tool.
fn todo_table(
    items: &[serde_json::Value],
    font_scale: f32,
    search_query: &str,
) -> Element<'static, Message> {
    let col_header = row![
        container(
            text("Text")
                .size(11.0 * font_scale)
                .color(CRABOT_TEXT_MUTED)
                .font(bold_font()),
        )
        .width(Fill)
        .padding(2),
        container(
            text("Status")
                .size(11.0 * font_scale)
                .color(CRABOT_TEXT_MUTED)
                .font(bold_font())
                .wrapping(Wrapping::None),
        )
        .width(TODO_STATUS_WIDTH)
        .padding(2),
    ]
    .spacing(8);

    let mut elements: Vec<Element<'static, Message>> = vec![col_header.into()];
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            elements.push(
                container(Space::new().width(Fill).height(1.0))
                    .style(|_theme: &Theme| container::Style {
                        background: Some(CRABOT_TOOL_CONTENT_BORDER.into()),
                        ..container::Style::default()
                    })
                    .into(),
            );
        }
        elements.push(todo_item_row(item, font_scale, search_query));
    }

    container(column(elements).spacing(0).width(Fill))
        .padding(4)
        .style(|_theme: &Theme| container::Style {
            border: Border {
                color: CRABOT_TOOL_CONTENT_BORDER,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .width(Fill)
        .into()
}

/// All tool-argument rows.
pub(super) fn args_rows<'a>(
    tool_name: &str,
    args: &'a serde_json::Value,
    font_scale: f32,
    search_query: &str,
) -> Vec<Element<'a, Message>> {
    let Some(map) = args.as_object() else {
        return Vec::new();
    };

    // Todo tool: render items as a table.
    if tool_name == "todo"
        && let Some(items) = map.get("items").and_then(|v| v.as_array())
    {
        return vec![todo_table(items, font_scale, search_query)];
    }

    let mut rows: Vec<Element<'_, Message>> = Vec::new();

    // Combine offset + limit into a single row when both are present
    // (used by the `read` tool).
    let has_offset_and_limit = map.contains_key("offset") && map.contains_key("limit");
    if has_offset_and_limit {
        let off = fmt_arg(map, "offset");
        let lim = fmt_arg(map, "limit");
        let combined = format!("offset: {}  limit: {}", off, lim);
        rows.push(
            container(
                row![if search_query.trim().is_empty() {
                    SelectableText::new(combined)
                        .size(12.0 * font_scale)
                        .style(sel_secondary)
                        .font(mono_font())
                        .into()
                } else {
                    highlighted_text(&combined, search_query, 12.0 * font_scale)
                },]
                .spacing(0),
            )
            .padding([4, 8])
            .style(|_theme: &Theme| container::Style {
                background: Some(CRABOT_TOOL_CONTENT_BG.into()),
                border: Border {
                    color: CRABOT_TOOL_CONTENT_BORDER,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            })
            .into(),
        );
    }

    for (k, v) in map {
        // Skip offset/limit if we already combined them above.
        if has_offset_and_limit && (k == "offset" || k == "limit") {
            continue;
        }
        if k == "edits"
            && let Some(arr) = v.as_array()
        {
            rows.push(edits_table(k, arr, font_scale, search_query));
            continue;
        }
        let val = v
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| v.to_string());
        rows.push(arg_row(k, val, font_scale, search_query));
    }
    rows
}

/// Format a single argument value from the args map as a string.
fn fmt_arg(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    map.get(key)
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string())
        })
        .unwrap_or_default()
}

/// Only the "path" argument row, when present.
pub(super) fn path_arg_row<'a>(
    args: &'a serde_json::Value,
    font_scale: f32,
    search_query: &str,
) -> Option<Element<'a, Message>> {
    let path = args.as_object()?.get("path")?.as_str()?;
    Some(arg_row("path", path.to_string(), font_scale, search_query))
}

/// Tool result text (success or error).
pub(super) fn result_text<'a>(
    result: &'a Result<String, String>,
    font_scale: f32,
    search_query: &str,
) -> Element<'a, Message> {
    let display: &str = result
        .as_ref()
        .map(|s| s.as_str())
        .unwrap_or_else(|e| e.as_str());
    let is_ok = result.is_ok();
    let accent = if is_ok {
        CRABOT_TOOL_ACCENT
    } else {
        CRABOT_DANGER
    };

    let body: Element<'_, Message> = if search_query.trim().is_empty() {
        SelectableText::new(display)
            .size(13.0 * font_scale)
            .style(move |theme: &Theme| SelectionStyle {
                color: Some(color_text(theme)),
                selection: accent,
            })
            .font(mono_font())
            .into()
    } else {
        highlighted_text(display, search_query, 13.0 * font_scale)
    };

    container(
        column![
            text(if is_ok { "Result" } else { "Error" })
                .size(11.0 * font_scale)
                .color(accent)
                .font(bold_font()),
            body,
        ]
        .spacing(4)
        .width(Fill),
    )
    .padding([8, 10])
    .style(move |_theme: &Theme| container::Style {
        background: Some(
            if is_ok {
                CRABOT_TOOL_CONTENT_BG
            } else {
                DIFF_BG_DEL
            }
            .into(),
        ),
        border: Border {
            color: if is_ok {
                CRABOT_TOOL_CONTENT_BORDER
            } else {
                accent.scale_alpha(0.4)
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}
