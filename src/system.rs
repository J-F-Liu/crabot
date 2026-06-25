use iced::{
    Alignment, Element, Fill, Length, Padding,
    widget::{
        Space, checkbox, column, container, mouse_area, pick_list, row, scrollable, text,
        text_editor, text_input,
    },
};

use crate::FocusedTarget;
use crate::Message;
use crate::widgets::textarea::TextArea;

use std::path::PathBuf;

pub const PREAMBLE: &str = "Preamble";
pub const RULES: &str = "Rules";
pub const TOOLS: &str = "Tools";
pub const WORKSPACE: &str = "Workspace";
pub const WORKSPACE_TREE: &str = "Workspace tree";
pub const DATE: &str = "Date";

#[derive(Debug, Clone)]
pub struct FilepathEntry {
    pub display: String,
    pub path: PathBuf,
}

impl std::fmt::Display for FilepathEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display)
    }
}

impl PartialEq for FilepathEntry {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemPrompt {
    pub preamble: (bool, String),
    pub rules: (bool, String),
    pub tools: (bool, String),
    pub workspace: (bool, PathBuf),
    pub files: (bool, String),
    pub date: (bool, String),
}

impl SystemPrompt {
    pub fn get_mut(&mut self, name: &str) -> Option<&mut (bool, String)> {
        match name {
            PREAMBLE => Some(&mut self.preamble),
            RULES => Some(&mut self.rules),
            TOOLS => Some(&mut self.tools),
            WORKSPACE_TREE => Some(&mut self.files),
            DATE => Some(&mut self.date),
            _ => None,
        }
    }

    /// Concatenate all enabled components, returning the full prompt string.
    pub fn get_prompt(&self) -> String {
        let mut prompt = String::new();
        if let (true, content) = &self.preamble
            && !content.is_empty()
        {
            prompt.push_str(content);
            prompt.push('\n');
        }
        if let (true, content) = &self.rules
            && !content.is_empty()
        {
            prompt.push_str(content);
            prompt.push('\n');
        }
        if let (true, tools) = &self.tools
            && !tools.is_empty()
        {
            prompt.push_str(tools);
            prompt.push('\n');
        }
        if let (true, workspace) = &self.workspace
            && workspace.is_dir()
        {
            let path = crate::workspace::get_unix_style_path(workspace);
            prompt.push_str(&format!("Current Workspace: {}\n", path));
            prompt.push_str("Use relative paths for files inside the workspace.\n");
        }
        if let (true, files) = &self.files
            && !files.is_empty()
        {
            prompt.push_str("<workspace-tree>\nWorking directory layout (sorted by mtime, recent first; depth ≤ 3):\n");
            prompt.push_str(files);
            prompt.push_str("\n</workspace-tree>\n");
        }
        if let (true, date) = &self.date
            && !date.is_empty()
        {
            prompt.push_str(&format!("Current Date: {}\n", date));
        }
        prompt
    }
}

// ── field view functions ────────────────────────────────────────────

fn expandable_header<'a>(
    name: &'static str,
    checked: bool,
    expanded: bool,
) -> Element<'a, Message> {
    let arrow = if expanded { "▼" } else { "⯈" };
    row![
        checkbox(checked)
            .label(name)
            .style(crate::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        Space::new().width(Length::Fill),
        mouse_area(text(arrow).size(12).width(16)).on_press(Message::ToggleExpanded(name)),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub fn preamble_field_view<'a>(
    field: &'a (bool, String),
    options: &'a [FilepathEntry],
    selected_display: &'a str,
) -> Element<'a, Message> {
    let checked = field.0;
    let name = PREAMBLE;
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
            .style(crate::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        pick_list(options, selected, Message::SelectPreamble).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub fn rules_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a TextArea,
) -> Element<'a, Message> {
    let header = expandable_header(RULES, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                content
                    .view(|msg| Message::EditTextArea(FocusedTarget::EditText(RULES), msg))
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
    let name = TOOLS;
    let header = expandable_header(name, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                text_editor(content)
                    .on_action(move |a| Message::EditTextContent(name, a))
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
            .style(crate::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
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
    let name = WORKSPACE_TREE;
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
                            .on_action(move |a| Message::EditTextContent(name, a))
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
    let name = DATE;

    row![
        checkbox(checked)
            .label(name)
            .style(crate::primary_checkbox)
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

pub fn build_preamble_options() -> Vec<FilepathEntry> {
    let dir = home::home_dir()
        .unwrap_or_default()
        .join(".crabot")
        .join("preamble");
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

pub fn build_workspace_options(recent: &[PathBuf]) -> Vec<FilepathEntry> {
    use std::collections::HashMap;

    let mut entries: Vec<FilepathEntry> = recent
        .iter()
        .map(|path| {
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
