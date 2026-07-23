use iced::{
    Alignment, Element, Fill, Length,
    widget::{
        Space, checkbox, column, mouse_area, pick_list, row, scrollable, text, text_editor,
        text_input,
    },
};

use crate::Message;
use crabot::system::{AGENTS_MD, DATE, FilepathEntry, TOOLS, WORKSPACE};

use super::theme::thin_vertical;

use std::path::PathBuf;

// ── internal helper ──────────────────────────────────────────────────

pub(crate) fn expandable_header<'a>(
    name: &'static str,
    checked: bool,
    expanded: bool,
) -> Element<'a, Message> {
    let arrow = if expanded { "▼" } else { "⯈" };
    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        Space::new().width(Length::Fill),
        mouse_area(text(arrow).size(12).width(16)).on_press(Message::ToggleExpanded(name)),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

// ── field views ──────────────────────────────────────────────────────

pub(crate) fn file_picker_field_view<'a>(
    name: &'static str,
    field: &'a (bool, String),
    options: &'a [FilepathEntry],
    selected_display: &'a str,
    on_select: fn(FilepathEntry) -> Message,
) -> Element<'a, Message> {
    let checked = field.0;
    let selected = if selected_display.is_empty() {
        None
    } else {
        options
            .iter()
            .find(|e| e.display == selected_display)
            .cloned()
    };

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v))
            .width(Fill),
        pick_list(options, selected, on_select).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub(crate) fn tools_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let name = TOOLS;
    let header = expandable_header(name, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                text_editor(content)
                    .on_action(move |a| Message::EditTextContent(name, a))
                    .height(Length::Fixed(120.0)),
            )
            .direction(thin_vertical()),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub(crate) fn workspace_field_view<'a>(
    field: &'a (bool, PathBuf),
    options: &'a [FilepathEntry],
) -> Element<'a, Message> {
    let checked = field.0;
    let name = WORKSPACE;
    let selected = if field.1.as_os_str().is_empty() {
        None
    } else {
        options.iter().find(|e| e.path == field.1).cloned()
    };

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v))
            .width(Fill),
        pick_list(options, selected, Message::SelectWorkspace).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub fn build_md_file_options(subdir: &str) -> Vec<FilepathEntry> {
    let dir = home::home_dir()
        .unwrap_or_default()
        .join(".crabot")
        .join(subdir);
    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                let display = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                entries.push(FilepathEntry { display, path });
            }
        }
    }
    entries
}

/// Load options and content for a prompt file picker (preamble / rules).
pub fn load_prompt_options(subdir: &str, selected: &str) -> (Vec<FilepathEntry>, String) {
    let options = build_md_file_options(subdir);
    let content = options
        .iter()
        .find(|e| e.display == selected)
        .map(|e| std::fs::read_to_string(&e.path).unwrap_or_default())
        .unwrap_or_default();
    (options, content)
}

pub fn build_workspace_options(recent: &[(PathBuf, bool)]) -> Vec<FilepathEntry> {
    use std::collections::HashMap;

    let mut entries: Vec<FilepathEntry> = recent
        .iter()
        .map(|(path, _)| {
            let display = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            FilepathEntry {
                display,
                path: path.clone(),
            }
        })
        .collect();

    // Disambiguate duplicate folder names by prepending parent
    let mut counts: HashMap<String, usize> = HashMap::new();
    for e in &entries {
        *counts.entry(e.display.clone()).or_default() += 1;
    }
    for e in &mut entries {
        if counts[&e.display] > 1
            && let Some(parent) = e.path.parent()
            && let Some(parent_name) = parent.file_name().and_then(|n| n.to_str())
        {
            e.display = format!("{}/{}", parent_name, e.display);
        }
    }

    entries.push(FilepathEntry {
        display: "📁 Select new...".to_string(),
        path: PathBuf::new(),
    });

    entries
}

pub(crate) fn agents_md_field_view<'a>(field: &'a (bool, String)) -> Element<'a, Message> {
    let checked = field.0;
    let name = AGENTS_MD;

    checkbox(checked)
        .label(name)
        .style(crate::views::primary_checkbox)
        .on_toggle(move |v| Message::ToggleEnabled(name, v))
        .into()
}

pub(crate) fn date_field_view<'a>(field: &'a (bool, String)) -> Element<'a, Message> {
    let checked = field.0;
    let name = DATE;

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        text_input("YYYY-MM-DD", &field.1)
            .on_input(move |s| Message::EditTextField(name, s))
            .width(Length::Fixed(110.0))
            .padding(4)
            .align_x(iced::alignment::Horizontal::Center),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}
