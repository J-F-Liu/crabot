use chrono::Local;
use iced::{
    Alignment, Element, Fill, Length, Padding,
    widget::{
        checkbox, column, container, mouse_area, pick_list, row, scrollable, text, text_editor,
        text_input,
    },
};

use crate::Message;

#[derive(Debug, Clone)]
pub struct WorkspaceEntry {
    pub display: String,
    pub path: String,
}

impl std::fmt::Display for WorkspaceEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display)
    }
}

impl PartialEq for WorkspaceEntry {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

pub struct SystemPrompt {
    pub preamble: (bool, String),
    pub rules: (bool, String),
    pub tools: (bool, String),
    pub workspace: (bool, String),
    pub files: (bool, String),
    pub date: (bool, String),
}

impl SystemPrompt {
    pub fn get_mut(&mut self, name: &str) -> Option<&mut (bool, String)> {
        match name {
            "Preamble" => Some(&mut self.preamble),
            "Rules" => Some(&mut self.rules),
            "Tools" => Some(&mut self.tools),
            "Workspace" => Some(&mut self.workspace),
            "Files" => Some(&mut self.files),
            "Date" => Some(&mut self.date),
            _ => None,
        }
    }
}

// ── field view functions ────────────────────────────────────────────

fn expandable_header<'a>(
    name: &'static str,
    checked: bool,
    expanded: bool,
) -> Element<'a, Message> {
    let arrow = if expanded { "▼" } else { "▶" };
    row![
        checkbox(checked)
            .label(name)
            .on_toggle(move |v| Message::ToggleSystemEnabled(name, v)),
        mouse_area(text(arrow).size(12).width(16)).on_press(Message::ToggleSystemExpanded(name)),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub fn preamble_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let name = "Preamble";
    let header = expandable_header(name, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                text_editor(content)
                    .on_action(move |a| Message::EditSystemContent(name, a))
                    .height(Length::Fixed(120.0)),
            ),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub fn rules_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let name = "Rules";
    let header = expandable_header(name, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                text_editor(content)
                    .on_action(move |a| Message::EditSystemContent(name, a))
                    .height(Length::Fixed(120.0)),
            ),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub fn tools_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let name = "Tools";
    let header = expandable_header(name, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                text_editor(content)
                    .on_action(move |a| Message::EditSystemContent(name, a))
                    .height(Length::Fixed(120.0)),
            ),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub fn workspace_field_view<'a>(
    field: &'a (bool, String),
    options: Vec<WorkspaceEntry>,
) -> Element<'a, Message> {
    let checked = field.0;
    let name = "Workspace";
    let selected = if field.1.is_empty() {
        None
    } else {
        options.iter().find(|e| e.path == field.1).cloned()
    };

    row![
        checkbox(checked)
            .label(name)
            .on_toggle(move |v| Message::ToggleSystemEnabled(name, v)),
        pick_list(options, selected, Message::SelectWorkspace).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub fn files_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let name = "Files";
    let header = expandable_header(name, field.0, expanded);

    use iced::widget::scrollable::Direction;
    use iced::widget::text::Wrapping;

    if expanded {
        column![
            header,
            container(
                scrollable(
                    container(
                        text_editor(content)
                            .on_action(move |a| Message::EditSystemContent(name, a))
                            .font(iced::Font::MONOSPACE)
                            .wrapping(Wrapping::None),
                    )
                    .padding(Padding::new(0.0).bottom(12.0)),
                )
                .direction(Direction::Both {
                    vertical: Default::default(),
                    horizontal: Default::default(),
                })
                .height(Length::Fixed(200.0)),
            )
            .style(container::bordered_box)
            .width(Fill),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub fn date_field_view<'a>(field: &'a (bool, String)) -> Element<'a, Message> {
    let checked = field.0;
    let name = "Date";
    let today = Local::now().format("%Y-%m-%d").to_string();
    let value: &str = if field.1.is_empty() { &today } else { &field.1 };

    row![
        checkbox(checked)
            .label(name)
            .on_toggle(move |v| Message::ToggleSystemEnabled(name, v)),
        text_input("YYYY-MM-DD", value)
            .on_input(move |s| Message::EditSystemField(name, s))
            .width(Length::Fixed(100.0))
            .padding(4),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub fn build_workspace_options(recent: &[String]) -> Vec<WorkspaceEntry> {
    use std::collections::HashMap;

    let mut entries: Vec<WorkspaceEntry> = recent
        .iter()
        .map(|path| {
            let display = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_string();
            WorkspaceEntry {
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
        if counts[&e.display] > 1 {
            if let Some(parent) = std::path::Path::new(&e.path).parent() {
                if let Some(parent_name) = parent.file_name().and_then(|n| n.to_str()) {
                    e.display = format!("{}/{}", parent_name, e.display);
                }
            }
        }
    }

    entries.push(WorkspaceEntry {
        display: "📁 Select new...".to_string(),
        path: String::new(),
    });

    entries
}
