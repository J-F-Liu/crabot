use iced::{
    Alignment, Element, Fill, Length,
    widget::{
        Space, checkbox, column, mouse_area, pick_list, row, scrollable, text, text_editor,
        text_input,
    },
};

use crate::PromptEvent;
use crate::{AGENTS_MD, DATE, FilepathEntry, TOOLS, WORKSPACE};

use super::theme::thin_vertical;
use crate::app::ExpandableEditor;

use std::borrow::Cow;
use std::path::{Path, PathBuf};

// ── internal helper ──────────────────────────────────────────────────

pub(crate) fn expandable_header<'a>(
    name: &'static str,
    checked: bool,
    expanded: bool,
) -> Element<'a, PromptEvent> {
    let arrow = if expanded { "▼" } else { "⯈" };
    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| PromptEvent::ToggleEnabled(name, v)),
        Space::new().width(Length::Fill),
        mouse_area(text(arrow).size(12).width(16)).on_press(PromptEvent::ToggleExpanded(name)),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

// ── field views ──────────────────────────────────────────────────────

pub(crate) fn file_picker_field_view<'a>(
    name: &'static str,
    checked: bool,
    options: &'a [FilepathEntry],
    selected_display: &'a str,
    on_select: fn(FilepathEntry) -> PromptEvent,
) -> Element<'a, PromptEvent> {
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
            .on_toggle(move |v| PromptEvent::ToggleEnabled(name, v))
            .width(Fill),
        pick_list(options, selected, on_select).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub(crate) fn tools_field_view<'a>(tools: &'a ExpandableEditor) -> Element<'a, PromptEvent> {
    let name = TOOLS;
    let header = expandable_header(name, tools.enabled, tools.expanded);

    if tools.expanded {
        column![
            header,
            scrollable(
                text_editor(&tools.content)
                    .on_action(move |a| PromptEvent::EditTextContent(name, a))
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
) -> Element<'a, PromptEvent> {
    let checked = field.0;
    let name = WORKSPACE;

    // Picker options come from recents (capped at 10); the active workspace may
    // have fallen off the list, so prepend it to keep the picker in sync.
    let options: Cow<'a, [FilepathEntry]> =
        if field.1.as_os_str().is_empty() || options.iter().any(|e| e.path == field.1) {
            Cow::Borrowed(options)
        } else {
            let mut entries = vec![FilepathEntry {
                display: workspace_display(&field.1),
                path: field.1.clone(),
            }];
            entries.extend_from_slice(options);
            Cow::Owned(entries)
        };

    let selected = if field.1.as_os_str().is_empty() {
        None
    } else {
        options.iter().find(|e| e.path == field.1).cloned()
    };

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| PromptEvent::ToggleEnabled(name, v))
            .width(Fill),
        pick_list(options, selected, PromptEvent::SelectWorkspace).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

/// Build picker options for a prompt file directory (preamble / rules).
/// Each entry's display name is the file stem (extension omitted).
pub fn build_md_file_options(subdir: &str) -> Vec<FilepathEntry> {
    let dir = crabot::setup::config_dir().join(subdir);
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

/// Display label for a workspace path: the folder name, or "unknown".
fn workspace_display(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string()
}

pub fn build_workspace_options(recent: &[(PathBuf, bool)]) -> Vec<FilepathEntry> {
    use std::collections::HashMap;

    let mut entries: Vec<FilepathEntry> = recent
        .iter()
        .map(|(path, _)| FilepathEntry {
            display: workspace_display(path),
            path: path.clone(),
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

pub(crate) fn agents_md_field_view<'a>(field: &'a (bool, String)) -> Element<'a, PromptEvent> {
    let checked = field.0;
    let name = AGENTS_MD;

    checkbox(checked)
        .label(name)
        .style(crate::views::primary_checkbox)
        .on_toggle(move |v| PromptEvent::ToggleEnabled(name, v))
        .into()
}

pub(crate) fn date_field_view<'a>(field: &'a (bool, String)) -> Element<'a, PromptEvent> {
    let checked = field.0;
    let name = DATE;

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| PromptEvent::ToggleEnabled(name, v)),
        text_input("YYYY-MM-DD", &field.1)
            .on_input(move |s| PromptEvent::EditTextField(name, s))
            .width(Length::Fixed(110.0))
            .padding(4)
            .align_x(iced::alignment::Horizontal::Center),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}
