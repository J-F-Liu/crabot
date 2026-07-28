use iced::{
    Alignment, Element, Fill, Font, font,
    widget::{button, column, container, row, text},
};

use serde::Deserialize;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::ConversationEvent;
use crate::llm::DialogPhase;
use crate::widgets::dropdown::DropDown;

/// Lightweight session metadata for dropdown listing.
#[derive(Debug, Clone)]
pub(crate) struct SessionEntry {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    /// Year-month label derived from the session id (e.g. "2026-07").
    pub year_month: String,
    /// `true` when this entry is a non-selectable year-month header.
    pub is_header: bool,
}

impl SessionEntry {
    /// Create a year-month header entry for grouping sessions in the dropdown.
    fn header(label: &str) -> Self {
        Self {
            id: String::new(),
            title: label.to_string(),
            path: PathBuf::new(),
            year_month: label.to_string(),
            is_header: true,
        }
    }
}

impl std::fmt::Display for SessionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_header {
            write!(f, "{}", self.title)
        } else if self.title.is_empty() {
            write!(f, "{}", self.id)
        } else {
            write!(f, "{} — {}", self.id, self.title)
        }
    }
}

impl PartialEq for SessionEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

pub(crate) fn session_view<'a>(
    streaming: DialogPhase,
    session_options: &'a [SessionEntry],
    current_session_id: &'a str,
) -> Element<'a, ConversationEvent> {
    let selected = session_options.iter().find(|e| e.id == current_session_id);

    let header_font = Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    };

    let mut list = DropDown::new(session_options, selected, ConversationEvent::LoadSession)
        .width(Fill)
        .menu_width(600.0)
        .text_size(14.0)
        .item_is_header(|i| session_options.get(i).is_some_and(|e| e.is_header))
        .header_font(header_font)
        .item_indent(16.0);

    list = if streaming != DialogPhase::Idle {
        list.style(crate::views::disabled_dropdown_style)
    } else {
        list.on_open(ConversationEvent::SessionPickerFocused)
    };

    column![
        row![
            text("Session").size(14).font(Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }),
            container(list),
            button(text("New").size(13).align_x(Alignment::Center))
                .on_press(ConversationEvent::NewSession)
                .style(crate::views::primary_button),
        ]
        .align_y(Alignment::Center)
        .spacing(8),
    ]
    .spacing(4)
    .into()
}

/// Only the fields needed for the dropdown; serde skips the rest
/// (notably the large `history`) without allocating.
#[derive(Deserialize)]
struct SessionMeta {
    id: String,
    #[serde(default)]
    title: String,
}

/// List session metadata for a workspace, skipping unreadable/corrupt files.
/// Groups entries by year-month and inserts header entries between groups.
pub(crate) fn list_entries(workspace: &Path) -> Result<Vec<SessionEntry>, String> {
    let paths = crabot::session::list_session_paths(workspace)?;
    let mut entries: Vec<SessionEntry> = paths
        .into_iter()
        .filter_map(|path| {
            let file = std::fs::File::open(&path).ok()?;
            let meta: SessionMeta = serde_json::from_reader(BufReader::new(file)).ok()?;
            let year_month = crabot::session::year_month_from_id(&meta.id);
            Some(SessionEntry {
                id: meta.id,
                title: meta.title,
                path,
                year_month,
                is_header: false,
            })
        })
        .collect();

    // Sort by id descending (chronological order), then insert year-month headers.
    entries.sort_by(|a, b| b.id.cmp(&a.id));

    let mut grouped: Vec<SessionEntry> = Vec::new();
    let mut last_ym: Option<String> = None;
    for entry in entries {
        if last_ym.as_ref() != Some(&entry.year_month) {
            grouped.push(SessionEntry::header(&entry.year_month));
            last_ym = Some(entry.year_month.clone());
        }
        grouped.push(entry);
    }

    Ok(grouped)
}
