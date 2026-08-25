use iced::{Task, widget::text_editor};

use std::path::{Path, PathBuf};

use crabot::workspace;

pub(crate) const PREAMBLE: &str = "Preamble";
pub(crate) const RULES: &str = "Rules";
pub(crate) const TOOLS: &str = "Tools";
pub(crate) const WORKSPACE: &str = "Workspace";
pub(crate) const AGENTS_MD: &str = "AGENTS.md";
pub(crate) const WORKSPACE_TREE: &str = "Workspace tree";
pub(crate) const DATE: &str = "Date";

#[derive(Debug, Clone)]
pub(crate) struct FilepathEntry {
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

use crate::app::{App, ConversationEvent, FocusedTarget, Message, PromptEvent};

pub(crate) fn update(app: &mut App, event: PromptEvent) -> Task<Message> {
    match event {
        PromptEvent::ToggleEnabled(name, enabled) => match name {
            WORKSPACE => app.prompt.workspace.0 = enabled,
            WORKSPACE_TREE => app.prompt.files.enabled = enabled,
            TOOLS => app.prompt.tools.enabled = enabled,
            PREAMBLE => {
                app.prompt.preamble_enabled = enabled;
            }
            RULES => {
                app.prompt.rules_enabled = enabled;
            }
            _ => {
                if let Some(field) = app.prompt.get_mut(name) {
                    field.0 = enabled;
                }
            }
        },
        PromptEvent::ToggleExpanded(name) => {
            if name == WORKSPACE_TREE {
                app.prompt.files.expanded = !app.prompt.files.expanded;
            } else if name == TOOLS {
                app.prompt.tools.expanded = !app.prompt.tools.expanded;
            } else {
                app.tools.tool_list_state.update(name);
            }
        }
        PromptEvent::EditTextField(name, value) => {
            if let Some(field) = app.prompt.get_mut(name) {
                field.1 = value;
            }
        }
        PromptEvent::EditTextContent(name, action) => {
            if matches!(action, text_editor::Action::Click(_)) {
                app.layout.focused = Some(FocusedTarget::EditText(name));
            }
            let Some(content) = app.prompt.content_mut(name) else {
                return Task::none();
            };
            content.perform(action);
            let text = content.text();
            if let Some(field) = app.prompt.get_mut(name) {
                field.1 = text;
            }
        }
        PromptEvent::EditTextArea(target, message) => {
            if message.is_click() {
                app.layout.focused = Some(target);
            } else if app.layout.focused != Some(target) {
                return Task::none();
            }
            if target == FocusedTarget::UserPrompt {
                if message.is_enter() && !app.layout.shift_held {
                    return crate::app::conversation::send_prompt(app);
                }
                app.prompt
                    .user_prompt
                    .update(message, app.layout.shift_held);
            }
        }
        PromptEvent::SelectWorkspace(entry) => {
            if entry.path.as_os_str().is_empty() {
                return Task::perform(async { rfd::FileDialog::new().pick_folder() }, |path| {
                    Message::Prompt(PromptEvent::WorkspaceDialogResult(path))
                });
            }
            return set_workspace(app, entry.path);
        }
        PromptEvent::WorkspaceDialogResult(Some(path)) => {
            return set_workspace(app, path);
        }
        PromptEvent::WorkspaceDialogResult(None) => {}
        PromptEvent::SelectPreamble(entry) => {
            app.conversation.viewing_mut().selected_preamble = entry.display.clone();
        }
        PromptEvent::SelectRules(entry) => {
            app.settings.selected_rules = entry.display;
        }
        PromptEvent::SelectWorkMode(mode) => {
            app.prompt.workmode = mode;
        }
        PromptEvent::ToggleWorkMode(enabled) => {
            app.prompt.workmode_enabled = enabled;
        }
        PromptEvent::ToggleRecipeDropdown => {
            app.prompt.recipe_dropdown_expanded = !app.prompt.recipe_dropdown_expanded;
        }
        PromptEvent::SelectRecipe(index) => {
            let mode_key = app.prompt.workmode.name.to_lowercase();
            if let Some(recipes) = app.settings.prompt_recipes.get(&mode_key)
                && let Some(recipe) = recipes.get(index)
            {
                app.prompt.user_prompt.replace_text(recipe);
            }
            app.prompt.recipe_dropdown_expanded = false;
        }
        PromptEvent::DismissRecipeDropdown => {
            app.prompt.recipe_dropdown_expanded = false;
        }
        PromptEvent::SendPrompt => {
            return crate::app::conversation::send_prompt(app);
        }
    }
    Task::none()
}

// ── Workspace & prompt-file helpers ───────────────────────────────

/// Apply `path` as the current workspace: save the outgoing workspace's
/// AGENTS.md preference, restore the incoming one, rebuild the files tree and
/// AGENTS.md content, refresh picker options and the session list.
///
/// When `bump_recents` is set, `path` is also moved to the front of the
/// recents list (explicit user selection).  Otherwise recents are left
/// untouched (e.g. tab-switch driven sync).
pub(crate) fn apply_workspace(app: &mut App, path: PathBuf, bump_recents: bool) -> Task<Message> {
    // Save current workspace preference before switching.
    let cur = &app.prompt.workspace.1;
    if !cur.as_os_str().is_empty() {
        let enabled = app.prompt.agents_md.0;
        app.settings.set_recent_workspace_enabled(cur, enabled);
    }

    // Restore the incoming workspace's AGENTS.md preference (default: enabled);
    // the file's existence is confirmed when the off-thread scan lands.
    let preferred = switch_workspace(app, &path);

    if bump_recents {
        app.settings.recent_workspaces.retain(|(p, _)| p != &path);
        app.settings
            .recent_workspaces
            .insert(0, (path.clone(), preferred));
        app.settings.recent_workspaces.truncate(10);
    }

    // Files tree + AGENTS.md are scanned off the UI thread.
    let scan_task = scan_workspace_content(path.clone(), Some(preferred));
    let list_task =
        if !bump_recents && let Some(entries) = app.conversation.session_list_cache.get(&path) {
            // Tab-switch sync: reuse the cached list without re-scanning.
            app.conversation.session_list = entries.clone();
            app.conversation.session_list_loading = false;
            Task::none()
        } else {
            // Explicit workspace selection or cache miss always re-scans.
            app.conversation.session_list.clear();
            app.conversation.session_list_loading = true;
            crate::app::conversation::refresh_session_list(path)
        };
    scan_task.chain(list_task)
}

/// Switch the in-memory prompt workspace to `path` without persisting anything.
pub(crate) fn sync_workspace(app: &mut App, path: PathBuf) -> Task<Message> {
    let preferred = switch_workspace(app, &path);
    scan_workspace_content(path, Some(preferred))
}

/// Make `path` the current prompt workspace: restore its AGENTS.md preference
/// (default: enabled) and refresh the picker options. Returns the restored
/// preference; the file's existence is confirmed when the off-thread scan
/// lands. No persistence side effects.
fn switch_workspace(app: &mut App, path: &Path) -> bool {
    let preferred = app
        .settings
        .recent_workspaces
        .iter()
        .find_map(|(p, e)| (p == path).then_some(*e))
        .unwrap_or(true);
    app.prompt.agents_md.0 = preferred;
    app.prompt.workspace.1 = path.to_path_buf();
    app.prompt.workspace_options =
        crate::views::build_workspace_options(&app.settings.recent_workspaces);
    // Hold the accepted workspace's shared snapshot lock for the process lifetime.
    crate::app::snapshot::retain_workspace_lock(app, path);
    preferred
}

/// Result of an off-thread workspace scan: the files tree and AGENTS.md.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceScan {
    pub(crate) workspace: PathBuf,
    pub(crate) files_tree: String,
    pub(crate) agents_md_exists: bool,
    pub(crate) agents_md_content: String,
    /// When set (workspace switch), AGENTS.md is enabled only while the file
    /// exists. `None` (fresh-session refresh) leaves the toggle untouched.
    pub(crate) agents_md_preferred: Option<bool>,
}

/// Scan a workspace (files tree + AGENTS.md) off the UI thread; the result is
/// applied to the prompt state via `WorkspaceContentReady` when it lands.
pub(crate) fn scan_workspace_content(
    workspace: PathBuf,
    agents_md_preferred: Option<bool>,
) -> Task<Message> {
    Task::perform(
        async move {
            let path = workspace.clone();
            let (files_tree, agents_md_exists, agents_md_content) =
                tokio::task::spawn_blocking(move || {
                    let tree = workspace::build_files_tree(&path);
                    let (exists, content) = load_agents_md(&path);
                    (tree, exists, content)
                })
                .await
                .unwrap_or_default();
            WorkspaceScan {
                workspace,
                files_tree,
                agents_md_exists,
                agents_md_content,
                agents_md_preferred,
            }
        },
        |scan| Message::Conversation(ConversationEvent::WorkspaceContentReady(Box::new(scan))),
    )
}

/// Reload the files tree and AGENTS.md content when a fresh session tab is created.
pub(crate) fn refresh_workspace_content(app: &mut App) -> Task<Message> {
    let workspace = app.prompt.workspace.1.clone();
    app.prompt.files.enabled = true;
    scan_workspace_content(workspace, None)
}

/// Apply `path` as the workspace on explicit user selection, bumping it to the top of recents.
pub(crate) fn set_workspace(app: &mut App, path: PathBuf) -> Task<Message> {
    apply_workspace(app, path, true)
}

/// Read `AGENTS.md` from the workspace root, returning (exists, content).
pub(crate) fn load_agents_md(workspace: &Path) -> (bool, String) {
    if !workspace.as_os_str().is_empty() {
        let path = workspace.join("AGENTS.md");
        if path.is_file() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            return (true, content);
        }
    }
    (false, String::new())
}

/// Read a prompt file from disk by looking up the display name in the options.
/// Missing files or empty selections yield an empty string.
pub(crate) fn load_prompt_file(options: &[FilepathEntry], selected: &str) -> String {
    options
        .iter()
        .find(|e| e.display == selected)
        .and_then(|e| {
            std::fs::read_to_string(&e.path)
                .inspect_err(|err| {
                    tracing::warn!("Failed to read prompt file {}: {err}", e.path.display());
                })
                .ok()
        })
        .unwrap_or_default()
}
