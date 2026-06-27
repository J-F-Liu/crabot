use iced::{
    Border, Color, Element, Fill, Font, Theme, font,
    widget::{Space, column, container, row, text},
};
use iced_selection::Text as SelectableText;
use iced_selection::text::Style as SelectionStyle;

use super::styles::{sel_default, sel_primary, sel_secondary};
use super::theme::{
    CRABOT_DANGER, CRABOT_SUCCESS, CRABOT_TEXT_MUTED, CRABOT_TOOL_ACCENT, CRABOT_TOOL_CONTENT_BG,
    CRABOT_TOOL_CONTENT_BORDER, color_text,
};
use crate::Message;
use crate::tools::edit::EditParam;

/// Monospace font stack for paths and code snippets.
fn mono_font() -> Font {
    Font {
        family: font::Family::Monospace,
        ..Font::DEFAULT
    }
}

/// Single tool-argument key-value row.
pub(super) fn arg_row<'a>(key: &'a str, value: String) -> Element<'a, Message> {
    row![
        text(format!("{}:", key))
            .size(12)
            .color(CRABOT_TOOL_ACCENT)
            .font(Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }),
        Space::new().width(8),
        SelectableText::new(value)
            .size(12)
            .style(sel_default)
            .font(mono_font()),
    ]
    .spacing(0)
    .into()
}

/// Embedded table for the `edits` argument — each edit becomes a labelled block.
fn edits_table<'a>(key: &'a str, edits: &'a [serde_json::Value]) -> Element<'a, Message> {
    let header = row![
        text(format!("{}:", key))
            .size(12)
            .color(CRABOT_TEXT_MUTED)
            .font(Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }),
        Space::new().width(8),
        text(format!("{} edit(s)", edits.len()))
            .size(12)
            .color(CRABOT_TEXT_MUTED),
    ]
    .spacing(0);

    let rows: Vec<Element<'_, Message>> = edits
        .iter()
        .enumerate()
        .flat_map(|(i, edit)| {
            let ep: EditParam = serde_json::from_value(edit.clone()).unwrap_or(EditParam {
                old_text: String::new(),
                new_text: String::new(),
            });
            let EditParam { old_text, new_text } = ep;
            let idx = container(
                text(format!("Edit #{}", i + 1))
                    .size(11)
                    .color(CRABOT_TEXT_MUTED),
            )
            .padding([2, 0])
            .into();

            let old_row = container(
                row![
                    text("−").size(13).color(CRABOT_DANGER).font(Font {
                        weight: font::Weight::Bold,
                        ..Font::DEFAULT
                    }),
                    Space::new().width(6),
                    SelectableText::new(old_text)
                        .size(12)
                        .style(sel_secondary)
                        .font(mono_font()),
                ]
                .spacing(0),
            )
            .padding([4, 8])
            .width(Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(Color::from_rgb8(0xFF, 0xF0, 0xF0).into()),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..container::Style::default()
            })
            .into();

            let new_row = container(
                row![
                    text("+").size(13).color(CRABOT_SUCCESS).font(Font {
                        weight: font::Weight::Bold,
                        ..Font::DEFAULT
                    }),
                    Space::new().width(6),
                    SelectableText::new(new_text)
                        .size(12)
                        .style(sel_primary)
                        .font(mono_font()),
                ]
                .spacing(0),
            )
            .padding([4, 8])
            .width(Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(Color::from_rgb8(0xF0, 0xFA, 0xF4).into()),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..container::Style::default()
            })
            .into();

            [idx, old_row, new_row]
        })
        .collect();

    column![header.width(Fill), column(rows).spacing(4).width(Fill)]
        .spacing(6)
        .width(Fill)
        .into()
}

/// All tool-argument rows.
pub(super) fn args_rows(args: &serde_json::Value) -> Vec<Element<'_, Message>> {
    let Some(map) = args.as_object() else {
        return Vec::new();
    };
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
                row![
                    SelectableText::new(combined)
                        .size(12)
                        .style(sel_secondary)
                        .font(mono_font()),
                ]
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
            rows.push(edits_table(k, arr));
            continue;
        }
        let val = v
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| v.to_string());
        rows.push(arg_row(k, val));
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
pub(super) fn path_arg_row(args: &serde_json::Value) -> Option<Element<'_, Message>> {
    let path = args.as_object()?.get("path")?.as_str()?;
    Some(arg_row("path", path.to_string()))
}

/// Tool result text (success or error).
pub(super) fn result_text(result: &Result<String, String>) -> Element<'_, Message> {
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

    container(
        column![
            text(if is_ok { "Result" } else { "Error" })
                .size(11)
                .color(accent)
                .font(Font {
                    weight: font::Weight::Bold,
                    ..Font::DEFAULT
                }),
            SelectableText::new(display)
                .size(13)
                .style(move |theme: &Theme| SelectionStyle {
                    color: Some(color_text(theme)),
                    selection: accent,
                })
                .font(mono_font()),
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
                Color::from_rgb8(0xFF, 0xF0, 0xF0)
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
