//! Per-session file snapshots backing the right-pane Revert buttons. The first
//! `write`/`edit` on a file stores its original content in
//! `.agent/snapshots/{session_id}/{hash}.existed`, or an empty `{hash}.created`
//! when the file didn't exist yet; Revert restores the content or deletes the
//! file. Snapshots may hold secrets — they live in gitignored `.agent/` and are
//! deleted on tab close / app exit.

use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use iced::Task;

use super::session_tab::SessionTab;
use super::{App, Message};
use crabot::chat::ToolCall;
use crabot::tools::{arg_path, resolve_path_partial};

/// Outcome of a single background revert — `Ok(raw)` unlists the file, `Err` shows the message.
type RevertOutcome = Result<String, String>;

fn snapshots_root(workspace: &Path) -> PathBuf {
    workspace.join(".agent").join("snapshots")
}

fn snapshot_dir(workspace: &Path, session_id: &str) -> PathBuf {
    snapshots_root(workspace).join(session_id)
}

/// Advisory-lock file marking an instance as active on a workspace.
fn lock_file(workspace: &Path) -> PathBuf {
    workspace.join(".agent").join("snapshots.lock")
}

// ── cross-instance workspace locks ────────────────────────────────
// Every running instance holds a shared lock per active workspace for its
// whole lifetime; exit-time cleanup takes an exclusive lock (after dropping
// its own shared one) to detect whether another instance is still alive.
fn open_lock_file(workspace: &Path) -> std::io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_file(workspace))
}

/// Take the shared workspace lock once, held until app exit, so a concurrent
/// instance's cleanup knows this instance is alive. Called on first snapshot
/// in `workspace`; never-snapshotted workspaces stay unlocked.
pub(crate) fn retain_workspace_lock(app: &mut App, workspace: &Path) {
    if workspace.as_os_str().is_empty() || app.snapshot_locks.contains_key(workspace) {
        return;
    }
    match open_lock_file(workspace).and_then(|file| file.lock_shared().map(|()| file)) {
        Ok(file) => {
            app.snapshot_locks.insert(workspace.to_path_buf(), file);
        }
        Err(e) => {
            tracing::debug!(workspace = %workspace.display(), "failed to lock snapshots.lock: {e}")
        }
    }
}

/// Stable per-file filename — hash of the canonical key, so capture and
/// revert agree without any path sanitization.
fn key_file(key: &str) -> String {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Canonical key for a tool path — same resolution at capture and revert.
fn canonical_key(workspace: &Path, path: &str) -> Option<String> {
    resolve_path_partial(path, workspace)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Snapshot paths for a canonical key — `existed` holds the original content,
/// `created` (empty) marks a file the session created.
fn snapshot_paths(dir: &Path, key: &str) -> (PathBuf, PathBuf) {
    let hash = key_file(key);
    (
        dir.join(format!("{hash}.existed")),
        dir.join(format!("{hash}.created")),
    )
}

/// Capture the original content of `path` — first capture wins.
/// Returns `false` (no Revert button) for unresolvable paths or binary files.
fn capture_into(workspace: &Path, session_id: &str, path: &str) -> bool {
    let Some(key) = canonical_key(workspace, path) else {
        return false;
    };
    let dir = snapshot_dir(workspace, session_id);
    let (existed_file, created_file) = snapshot_paths(&dir, &key);
    if existed_file.exists() || created_file.exists() {
        return true;
    }
    // `existed` → Revert restores content; `created` → Revert deletes the file.
    let (file, content) = match std::fs::read_to_string(&key) {
        Ok(content) => (existed_file, content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (created_file, String::new()),
        Err(_) => return false,
    };
    let _ = std::fs::create_dir_all(&dir);
    // `create_new` → first writer wins; a racing capture keeps the original pre-image.
    let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
    else {
        return true; // already snapshotted
    };
    let Ok(()) = f.write_all(content.as_bytes()) else {
        // No snapshot means no Revert button — worth surfacing in the log.
        tracing::warn!(path = %key, "failed to write snapshot, revert unavailable");
        let _ = std::fs::remove_file(&file);
        return false;
    };
    true
}

/// Snapshot all `write`/`edit` targets of a tool-call batch off the UI thread;
/// returns the raw paths snapshotted. Runs on a blocking thread, awaited before
/// tool execution so the pre-image read beats the tool's own write.
pub(crate) async fn capture_tool_targets(
    workspace: PathBuf,
    session_id: String,
    tcs: &[ToolCall],
) -> Vec<String> {
    if workspace.as_os_str().is_empty() {
        return Vec::new();
    }
    // Only `write`/`edit` modify files — extract their target paths first.
    let paths: Vec<String> = tcs
        .iter()
        .filter(|tc| matches!(tc.name.as_str(), "write" | "edit"))
        .filter_map(|tc| arg_path(&tc.args).map(str::to_string))
        .collect();
    if paths.is_empty() {
        return Vec::new();
    }
    tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .filter(|p| capture_into(&workspace, &session_id, p))
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Restore the original content of `path` (Revert action).
fn restore(workspace: &Path, session_id: &str, path: &str) -> Result<(), String> {
    let key =
        canonical_key(workspace, path).ok_or_else(|| format!("Failed to resolve path '{path}'"))?;
    let dir = snapshot_dir(workspace, session_id);
    let (existed_file, created_file) = snapshot_paths(&dir, &key);
    if existed_file.exists() {
        let content = std::fs::read_to_string(&existed_file)
            .map_err(|e| format!("Failed to read snapshot for '{path}': {e}"))?;
        if let Some(parent) = Path::new(&key).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir: {e}"))?;
        }
        std::fs::write(&key, content).map_err(|e| format!("Failed to restore '{path}': {e}"))?;
        let _ = std::fs::remove_file(&existed_file);
    } else if created_file.exists() {
        match std::fs::remove_file(&key) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Failed to delete '{path}': {e}")),
        }
        let _ = std::fs::remove_file(&created_file);
    } else {
        return Err(format!("No snapshot for '{path}' — nothing to revert"));
    }
    Ok(())
}

/// Delete the session's snapshot files (tab closed).
pub(crate) fn cleanup(workspace: &Path, session_id: &str) {
    if !workspace.as_os_str().is_empty()
        && let Err(e) = std::fs::remove_dir_all(snapshot_dir(workspace, session_id))
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!(session = %session_id, "failed to clean up snapshots: {e}");
    }
}

/// Clear the snapshots of every locked workspace (app exit / restart). Draining
/// `snapshot_locks` releases our shared lock, then an exclusive-lock probe
/// decides whether another instance is still alive on that workspace.
/// Iterating locks (not just open tabs) also covers switched-away workspaces.
pub(crate) fn cleanup_snapshots(app: &mut App) {
    for (workspace, lock) in app.snapshot_locks.drain() {
        drop(lock);
        let Ok(probe) = open_lock_file(&workspace) else {
            tracing::debug!(workspace = %workspace.display(), "failed to open snapshots.lock");
            continue;
        };
        match probe.try_lock() {
            Ok(()) => {
                // Keep the snapshots root itself, clear only its entries.
                if let Err(e) = clear_dir(&snapshots_root(&workspace)) {
                    tracing::debug!(workspace = %workspace.display(), "failed to clean snapshots: {e}");
                }
            }
            // Another instance still holds the shared lock — keep its snapshots.
            Err(e) => tracing::debug!(
                workspace = %workspace.display(),
                "another crabot instance active, keeping snapshots ({e})"
            ),
        }
    }
}

/// Empty a directory but keep it — removes every entry (dirs recursively).
fn clear_dir(dir: &Path) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

// ── Background revert ──────────────────────────────────────────────

/// Revert one file off the UI thread.
async fn revert_one(workspace: PathBuf, session_id: String, raw: String) -> RevertOutcome {
    tokio::task::spawn_blocking(move || restore(&workspace, &session_id, &raw).map(|()| raw))
        .await
        .unwrap_or_else(|e| Err(format!("Revert task failed: {e}")))
}

/// Revert all `raws` off the UI thread; outcomes stay ordered for the UI.
async fn revert_many(
    workspace: PathBuf,
    session_id: String,
    raws: Vec<String>,
) -> Vec<RevertOutcome> {
    tokio::task::spawn_blocking(move || {
        raws.into_iter()
            .map(|raw| restore(&workspace, &session_id, &raw).map(|()| raw))
            .collect()
    })
    .await
    .unwrap_or_else(|e| vec![Err(format!("Revert task failed: {e}"))])
}

/// Revert a single file (Revert button) — restores in the background.
pub(crate) fn revert(app: &mut App, raw: String) -> Task<Message> {
    let number = app.conversation.viewing_tab_number();
    let workspace = app.conversation.viewing().session.workspace.clone();
    let session_id = app.conversation.viewing().session.id.clone();
    if workspace.as_os_str().is_empty() {
        app.conversation.viewing_mut().modified_files_error =
            Some(format!("No workspace set — cannot revert '{raw}'"));
        return Task::none();
    }
    Task::perform(revert_one(workspace, session_id, raw), move |outcome| {
        Message::RevertDone(number, outcome)
    })
}

/// Open the Revert-All confirmation dialog. The owning tab is captured up
/// front so Ctrl+1..9 tab shortcuts still work under the modal.
pub(crate) fn request_revert_all(app: &mut App) -> Task<Message> {
    app.overlay.revert_all_tab = app.conversation.viewing_tab_number();
    app.overlay.show_revert_all_confirm = true;
    Task::none()
}

/// Execute the confirmed Revert All — restores every snapshotted file of the
/// owning tab in one background task.
pub(crate) fn revert_all(app: &mut App) -> Task<Message> {
    let number = app.overlay.revert_all_tab;
    let Some(pos) = app.conversation.tab_pos(number) else {
        return Task::none();
    };
    let workspace = app.conversation.session_tabs[pos].session.workspace.clone();
    let session_id = app.conversation.session_tabs[pos].session.id.clone();
    let raws: Vec<String> = app.conversation.session_tabs[pos]
        .snapshot_files
        .iter()
        .cloned()
        .collect();
    if workspace.as_os_str().is_empty() {
        app.conversation.session_tabs[pos].modified_files_error =
            Some("No workspace set — cannot revert files.".into());
        return Task::none();
    }
    Task::perform(revert_many(workspace, session_id, raws), move |outcomes| {
        Message::RevertAllDone(number, outcomes)
    })
}

/// Apply revert outcomes: unlist restored files, surface errors.
fn apply_outcomes(tab: &mut SessionTab, outcomes: Vec<RevertOutcome>) {
    let mut errors = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(raw) => {
                tab.snapshot_files.remove(&raw);
            }
            Err(e) => errors.push(e),
        }
    }
    tab.modified_files_error = (!errors.is_empty()).then(|| errors.join("\n"));
}

/// Apply the result of a single-file revert to the owning tab.
pub(crate) fn revert_done(app: &mut App, number: usize, outcome: RevertOutcome) -> Task<Message> {
    revert_all_done(app, number, vec![outcome])
}

/// Apply the results of a Revert All to the owning tab.
pub(crate) fn revert_all_done(
    app: &mut App,
    number: usize,
    outcomes: Vec<RevertOutcome>,
) -> Task<Message> {
    if let Some(pos) = app.conversation.tab_pos(number) {
        apply_outcomes(&mut app.conversation.session_tabs[pos], outcomes);
    }
    Task::none()
}
