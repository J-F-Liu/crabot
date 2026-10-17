//! Materialize user-selected text attachments for a request.
//!
//! History carries only attachment markers (see
//! [`crabot::chat::attachment_marker_message`]), so text files are read here
//! when a request is built. Images in the same marker are rewritten into the
//! regular `[image: ...]` form and handled by [`super::image`].

use std::path::Path;

use genai::chat::{ChatMessage, ContentPart, MessageContent};

use crabot::chat::{attachment_marker_items, image_marker_text};
use crabot::tools::{read_text_attachment, resolve_path};
use crabot::user::AttachmentKind;

/// Read text attachments and rewrite image attachments in place. Idempotent:
/// messages without attachment markers pass through unchanged.
pub(super) async fn attach_text_files(messages: &mut Vec<ChatMessage>, workspace: &Path) {
    let mut out = Vec::with_capacity(messages.len());
    for msg in messages.drain(..) {
        let Some(items) = attachment_marker_items(&msg) else {
            out.push(msg);
            continue;
        };

        let mut parts = Vec::new();
        let mut image_paths = Vec::new();
        for item in items {
            match item.kind {
                AttachmentKind::Text => {
                    parts.push(ContentPart::Text(
                        read_text_file(&item.path, workspace).await,
                    ));
                }
                AttachmentKind::Image => image_paths.push(item.path),
            }
        }

        if !parts.is_empty() {
            out.push(ChatMessage::user(MessageContent::from_parts(parts)));
        }
        if !image_paths.is_empty() {
            out.push(ChatMessage::user(image_marker_text(&image_paths, &[])));
        }
    }
    *messages = out;
}

/// Read one text attachment with a bounded buffer, turning failures into a
/// notice the model can reason about instead of aborting the request.
async fn read_text_file(path: &str, workspace: &Path) -> String {
    let resolved = match resolve_path(path, workspace) {
        Ok(path) => path,
        Err(reason) => return unavailable(path, &reason.to_string()),
    };
    let content = tokio::task::spawn_blocking(move || read_text_attachment(&resolved))
        .await
        .unwrap_or_else(|err| Err(format!("read task failed: {err}")));
    match content {
        Ok(content) => format!("[Attached file: {path}]\n\n{content}"),
        Err(reason) => unavailable(path, &reason),
    }
}

fn unavailable(path: &str, reason: &str) -> String {
    format!("[Attachment unavailable: {path} — {reason}]")
}
