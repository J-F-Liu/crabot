use iced::{
    Element,
    widget::{Space, column, row, text},
};
use iced_selection::Text as SelectableText;

use super::styles::{sel_default, sel_primary, sel_secondary};
use super::theme::{color_primary, color_secondary};
use crate::Message;
use crate::tools::edit::EditParam;

/// Single tool-argument key-value row.
pub(super) fn arg_row<'a>(key: &'a str, value: String) -> Element<'a, Message> {
    row![
        SelectableText::new(key).size(12).style(sel_primary),
        Space::new().width(8),
        SelectableText::new(value).size(12).style(sel_secondary),
    ]
    .spacing(0)
    .into()
}

/// Embedded table for the `edits` argument — each edit becomes a labelled block.
fn edits_table<'a>(key: &'a str, edits: &'a [serde_json::Value]) -> Element<'a, Message> {
    let header = row![
        SelectableText::new(key).size(12).style(sel_primary),
        Space::new().width(8),
        SelectableText::new(format!("{}", edits.len()))
            .size(12)
            .style(sel_secondary),
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
            let idx = row![
                SelectableText::new(format!("#{}", i))
                    .size(11)
                    .style(sel_secondary),
                text(":").size(11).style(|t| text::Style {
                    color: Some(color_secondary(t))
                }),
            ]
            .spacing(0);
            let old_row = row![
                Space::new().width(12),
                text("-").size(12).style(|t| text::Style {
                    color: Some(color_secondary(t))
                }),
                Space::new().width(4),
                SelectableText::new(old_text).size(12).style(sel_secondary),
            ]
            .spacing(0);
            let new_row = row![
                Space::new().width(12),
                text("+").size(12).style(|t| text::Style {
                    color: Some(color_primary(t))
                }),
                Space::new().width(4),
                SelectableText::new(new_text).size(12).style(sel_primary),
            ]
            .spacing(0);
            [idx.into(), old_row.into(), new_row.into()]
        })
        .collect();

    column![header, column(rows).spacing(0)].spacing(2).into()
}

/// All tool-argument rows.
pub(super) fn args_rows(args: &serde_json::Value) -> Vec<Element<'_, Message>> {
    let Some(map) = args.as_object() else {
        return Vec::new();
    };
    map.iter()
        .map(|(k, v)| {
            if k == "edits"
                && let Some(arr) = v.as_array()
            {
                return edits_table(k, arr);
            }
            let val = v
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string());
            arg_row(k, val)
        })
        .collect()
}

/// Only the "path" argument row, when present.
pub(super) fn path_arg_row(args: &serde_json::Value) -> Option<Element<'_, Message>> {
    let path = args.as_object()?.get("path")?.as_str()?;
    Some(arg_row("path", path.to_string()))
}

/// Tool result text (success or error).
pub(super) fn result_text(result: &Result<String, String>) -> Element<'_, Message> {
    let display = result.clone().unwrap_or_else(|e| e);
    let style = if result.is_ok() {
        sel_default
    } else {
        sel_secondary
    };
    SelectableText::new(display).size(14).style(style).into()
}
