use iced::{
    Alignment, Element, Fill, Font, font,
    widget::{button, column, container, row, text},
};

use serde::Deserialize;
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

    /// Create a session entry for a session file; the year-month derives from the id.
    pub(crate) fn session(id: String, title: String, path: PathBuf) -> Self {
        Self {
            year_month: crabot::session::year_month_from_id(&id),
            id,
            title,
            path,
            is_header: false,
        }
    }

    /// Build an entry from a session; `None` when it has no save path yet.
    pub(crate) fn from_session(session: &crabot::session::Session) -> Option<Self> {
        Some(Self::session(
            session.id.clone(),
            session.title.clone(),
            session.save_path()?,
        ))
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
    loading: bool,
    lang: crabot::i18n::Lang,
) -> Element<'a, ConversationEvent> {
    let selected = session_options.iter().find(|e| e.id == current_session_id);

    let header_font = Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    };

    let picker: Element<'a, ConversationEvent> = if loading {
        container(
            text(lang.tr("Loading…"))
                .size(13)
                .color(crate::views::theme::color_muted()),
        )
        .width(Fill)
        .align_x(Alignment::Start)
        .padding([5, 8])
        .into()
    } else {
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
            list.style(crate::views::secondary_dropdown_style)
                .on_open(ConversationEvent::SessionPickerFocused)
        };

        container(list).into()
    };

    column![
        row![
            text(lang.tr("Session")).size(14).font(header_font),
            picker,
            button(text(lang.tr("New")).size(13).align_x(Alignment::Center))
                .on_press(ConversationEvent::NewSession)
                .style(crate::views::primary_button),
        ]
        .align_y(Alignment::Center)
        .spacing(8),
    ]
    .spacing(4)
    .into()
}

/// Only the fields needed for the dropdown; serde skips the rest.
/// For `.jsonl` only the first line is parsed; for legacy `.json` the whole file.
#[derive(Deserialize)]
struct SessionMeta {
    id: String,
    #[serde(default)]
    title: String,
}

/// Read just the `SessionMeta` from a session file — first line for `.jsonl`,
/// the whole document for legacy `.json`.
fn read_meta(path: &Path) -> Option<SessionMeta> {
    let file = std::fs::File::open(path).ok()?;
    if path.extension().is_some_and(|e| e == "jsonl") {
        let mut first = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(file), &mut first).ok()?;
        serde_json::from_str(&first).ok()
    } else {
        serde_json::from_reader(std::io::BufReader::new(file)).ok()
    }
}

/// List session metadata for a workspace, skipping unreadable/corrupt files.
/// Groups entries by year-month and inserts header entries between groups.
pub(crate) fn list_entries(workspace: &Path) -> Result<Vec<SessionEntry>, String> {
    let paths = crabot::session::list_session_paths(workspace, None)?;
    let mut entries: Vec<SessionEntry> = paths
        .into_iter()
        .filter_map(|path| {
            let meta = read_meta(&path)?;
            Some(SessionEntry::session(meta.id, meta.title, path))
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

/// Insert an entry into its year-month group (as built by [`list_entries`]),
/// keeping ids newest-first; insert at the top when the group is missing.
pub(crate) fn insert_listed_entry(list: &mut Vec<SessionEntry>, entry: SessionEntry) {
    let Some(header) = list
        .iter()
        .position(|e| e.is_header && e.year_month == entry.year_month)
    else {
        list.insert(0, entry);
        return;
    };
    let end = list[header + 1..]
        .iter()
        .position(|e| e.is_header)
        .map_or(list.len(), |off| header + 1 + off);
    let pos = list[header + 1..end]
        .iter()
        .position(|e| e.id < entry.id)
        .map_or(end, |off| header + 1 + off);
    list.insert(pos, entry);
}
