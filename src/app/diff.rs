//! Right-pane diff action: opens a modified file's snapshot beside its current
//! content in VS Code. The pre-image paths come from [`super::snapshot`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use iced::Task;

use super::snapshot::{diff_root, diff_sides};
use super::{App, Message, spawn_detached};
use crabot::i18n::Lang;

/// Outcome of opening a diff — only a failure carries a message for the UI.
type DiffOutcome = Result<(), String>;

/// Diff-side filename keeping stem and extension, so the left side gets the
/// same syntax highlighting and a readable title.
fn diff_copy_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.snapshot.{ext}"),
        None => format!("{stem}.snapshot"),
    }
}

/// Copy the pre-image to `target` through a temp file, so a reader never sees a
/// partial copy and concurrent clicks cannot truncate each other.
fn place_copy(source: &Path, target: &Path) -> std::io::Result<()> {
    let parent = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::fs::copy(source, tmp.path())?;
    tmp.persist(target).map(|_| ()).map_err(|e| e.error)
}

/// Copy the pre-image into the per-file hash directory under a name derived
/// from `current`; falls back to the snapshot itself when the copy fails.
fn named_copy(source: &Path, current: &Path, dir: &Path) -> PathBuf {
    let target = dir
        .join(source.file_stem().unwrap_or_default())
        .join(diff_copy_name(current));
    match place_copy(source, &target) {
        Ok(()) => target,
        Err(e) => {
            tracing::debug!(source = %source.display(), "failed to copy snapshot for diff: {e}");
            source.to_path_buf()
        }
    }
}

/// Spawn `code --diff left right` detached; the CLI returns once the running
/// editor has taken the request.
fn launch_vs_code(left: &Path, right: &Path, lang: Lang) -> DiffOutcome {
    let Some(code) = ["code", "code-insiders", "codium"]
        .iter()
        .find_map(|name| which::which(name).ok())
    else {
        return Err(lang
            .tr("VS Code ('code') not found on PATH — install it to open the diff")
            .to_string());
    };
    let args = [OsStr::new("--diff"), left.as_os_str(), right.as_os_str()];
    spawn_detached(code, &args).map_err(|e| {
        lang.tr("Failed to open the diff in VS Code: {e}")
            .replacen("{e}", &e.to_string(), 1)
            .to_string()
    })
}

/// Open the pre-image ↔ current diff of `path` off the UI thread.
async fn diff_one(workspace: PathBuf, session_id: String, raw: String, lang: Lang) -> DiffOutcome {
    tokio::task::spawn_blocking(move || {
        let Some((source, current)) = diff_sides(&workspace, &session_id, &raw) else {
            // The row was drawn while the pre-image existed — it is gone now.
            return Err(lang
                .tr("Nothing to diff — the snapshot or file is gone")
                .to_string());
        };
        let left = named_copy(&source, &current, &diff_root(&workspace, &session_id));
        launch_vs_code(&left, &current, lang)
    })
    .await
    .unwrap_or_else(|e| {
        Err(lang
            .tr("Diff task failed: {e}")
            .replacen("{e}", &e.to_string(), 1)
            .to_string())
    })
}

/// Diff a modified file against its snapshot (right-pane row click).
pub(crate) fn open_diff(app: &mut App, raw: String) -> Task<Message> {
    let lang = app.settings.language;
    let number = app.conversation.viewing_tab_number();
    let workspace = app.conversation.viewing().session.workspace.clone();
    let session_id = app.conversation.viewing().session.id.clone();
    if workspace.as_os_str().is_empty() {
        tracing::debug!(path = %raw, "no workspace set, skipping diff");
        return Task::none();
    }
    Task::perform(diff_one(workspace, session_id, raw, lang), move |outcome| {
        Message::DiffDone(number, outcome)
    })
}

/// Surface a diff failure on the owning tab; a success leaves any revert error alone.
pub(crate) fn diff_done(app: &mut App, number: usize, outcome: DiffOutcome) -> Task<Message> {
    if let (Some(pos), Err(e)) = (app.conversation.tab_pos(number), outcome) {
        app.conversation.session_tabs[pos].modified_files_error = Some(e);
    }
    Task::none()
}
