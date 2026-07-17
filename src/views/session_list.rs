use iced::{
    Alignment, Element, Fill, Font, font,
    widget::{button, column, container, row, text},
};

use serde::Deserialize;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::Message;
use crate::llm::DialogPhase;
use crate::widgets::dropdown::DropDown;

/// Lightweight session metadata for dropdown listing.
#[derive(Debug, Clone)]
pub(crate) struct SessionEntry {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
}

impl std::fmt::Display for SessionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.title.is_empty() {
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
) -> Element<'a, Message> {
    let selected = session_options.iter().find(|e| e.id == current_session_id);

    let mut list = DropDown::new(
        session_options,
        selected,
        if streaming == DialogPhase::Idle {
            Message::LoadSession
        } else {
            |_| Message::Noop
        },
    )
    .width(Fill)
    .menu_width(600.0);

    list = if streaming != DialogPhase::Idle {
        list.style(crate::views::disabled_dropdown_style)
    } else {
        list
    };
    if streaming == DialogPhase::Idle {
        list = list.on_open(Message::SessionPickerFocused);
    }

    column![
        row![
            text("Session").size(14).font(Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }),
            container(list).clip(true),
            button(text("New").size(13).align_x(Alignment::Center))
                .on_press_maybe(if streaming != DialogPhase::Idle {
                    None
                } else {
                    Some(Message::NewSession)
                })
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
pub(crate) fn list_entries(workspace: &Path) -> Result<Vec<SessionEntry>, String> {
    let paths = crabot::session::list_session_paths(workspace)?;
    let entries = paths
        .into_iter()
        .filter_map(|path| {
            let file = std::fs::File::open(&path).ok()?;
            let meta: SessionMeta = serde_json::from_reader(BufReader::new(file)).ok()?;
            Some(SessionEntry {
                id: meta.id,
                title: meta.title,
                path,
            })
        })
        .collect();
    Ok(entries)
}
