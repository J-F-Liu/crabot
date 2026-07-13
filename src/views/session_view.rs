use iced::{
    Alignment, Element, Fill, Font, font,
    widget::{button, column, container, pick_list, row, text},
};

use json_escape::unescape;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::Message;
use crate::llm::DialogPhase;

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

    let mut list = pick_list(
        session_options,
        selected,
        if streaming == DialogPhase::Idle {
            Message::LoadSession
        } else {
            |_| Message::Noop
        },
    )
    .width(Fill);
    list = if streaming != DialogPhase::Idle {
        list.style(crate::views::disabled_pick_list_style)
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
            iced::widget::Space::new().width(Fill),
            button(text("New").align_x(Alignment::Center))
                .on_press_maybe(if streaming != DialogPhase::Idle {
                    None
                } else {
                    Some(Message::NewSession)
                })
                .style(crate::views::primary_button),
        ]
        .align_y(Alignment::Center)
        .spacing(8),
        container(list).clip(true),
    ]
    .spacing(4)
    .into()
}

/// List session metadata for a workspace (reads only first 8 KiB per file).
pub(crate) fn list_entries(workspace: &Path) -> Result<Vec<SessionEntry>, String> {
    let paths = crabot::session::list_session_paths(workspace)?;
    let mut entries = Vec::with_capacity(paths.len());
    let mut buf = vec![0u8; 8192];
    for path in paths {
        let (id, title) = match std::fs::File::open(&path) {
            Ok(mut file) => match file.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    (
                        extract_json_string(&text, "id").unwrap_or_default(),
                        extract_json_string(&text, "title").unwrap_or_default(),
                    )
                }
                _ => (String::new(), String::new()),
            },
            Err(_) => (String::new(), String::new()),
        };
        entries.push(SessionEntry { id, title, path });
    }
    Ok(entries)
}

/// Extract a top-level JSON string value for `key` from partial JSON text.
/// Unescaping (incl. `\uXXXX` surrogate pairs) is handled by `json_escape`;
/// truncated input yields the portion decoded so far.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let search = format!("\"{}\"", key);
    let rest = json.split_once(&search)?.1;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    // Isolate the quoted string: `unescape` won't stop at a closing quote,
    // so scan to the first unescaped `"` ourselves.
    let content = rest.strip_prefix('"')?;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => break,
            _ => i += 1,
        }
    }
    let inner = &content[..i.min(content.len())];
    Some(unescape(inner).display_utf8_lossy().to_string())
}
