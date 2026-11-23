//! Prompt paste: clipboard images become attachments, text falls through.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::{Task, clipboard, widget::text_editor};

use crabot::tools::unique_image_path;
use crabot::user::{AttachmentKind, UserAttachment};

use crate::widgets::textarea;

use super::{App, Message, PromptEvent};

/// Workspace-relative folder holding pasted images.
const PASTED_IMAGE_DIR: &str = ".agent/attachments";

/// Handle Ctrl/Cmd+V in the prompt editor.
pub(crate) fn paste(app: &mut App) -> Task<Message> {
    let dir = image_dir(app);
    clipboard::read_image().then(move |result| match result {
        Ok(image) => {
            let dir = dir.clone();
            Task::perform(store_image(image, dir), |result| {
                Message::Prompt(PromptEvent::ClipboardImage(result))
            })
        }
        Err(_) => clipboard::read_text().map(|result| {
            Message::Prompt(PromptEvent::ClipboardText(
                result.ok().map(|text| text.to_string()),
            ))
        }),
    })
}

/// Insert text at the editor's cursor or selection.
pub(crate) fn insert_text(app: &mut App, text: String) {
    let action = text_editor::Action::Edit(text_editor::Edit::Paste(Arc::new(text)));
    let shift_held = app.layout.shift_held;
    app.prompt
        .user_prompt
        .update(textarea::Message::Edit(action), shift_held);
}

/// Workspace scratch dir, or the config dir before a workspace is picked.
fn image_dir(app: &App) -> PathBuf {
    let workspace = &app.prompt.workspace.1;
    if workspace.is_dir() {
        workspace.join(PASTED_IMAGE_DIR)
    } else {
        crabot::setup::config_dir().join("attachments")
    }
}

/// Save clipboard pixels as a PNG off the UI thread and validate them.
async fn store_image(image: clipboard::Image, dir: PathBuf) -> Result<UserAttachment, String> {
    tokio::task::spawn_blocking(move || save_image(image, &dir))
        .await
        .unwrap_or_else(|error| Err(format!("pasted image task failed: {error}")))
}

fn save_image(image: clipboard::Image, dir: &Path) -> Result<UserAttachment, String> {
    let pixels = image::RgbaImage::from_raw(image.size.width, image.size.height, image.rgba.into())
        .ok_or("clipboard image data does not match its dimensions")?;

    std::fs::create_dir_all(dir)
        .map_err(|error| format!("Failed to create {}: {error}", dir.display()))?;
    let path = unique_image_path(dir, "pasted");
    image::DynamicImage::ImageRgba8(pixels)
        .save(&path)
        .map_err(|error| format!("Failed to save {}: {error}", path.display()))?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image");
    if let Err(reason) = crabot::tools::probe(&path, name) {
        let _ = std::fs::remove_file(&path);
        return Err(reason);
    }
    Ok(UserAttachment::new(
        AttachmentKind::Image,
        path.to_string_lossy(),
    ))
}
