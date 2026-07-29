use iced::{Task, widget::text_editor};

use std::path::PathBuf;

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

use crate::app::{App, FocusedTarget, Message, PromptEvent};

pub(crate) fn update(app: &mut App, event: PromptEvent) -> Task<Message> {
    match event {
        PromptEvent::ToggleEnabled(name, enabled) => {
            if name == WORKSPACE {
                app.prompt.workspace.0 = enabled;
            } else if name == WORKSPACE_TREE {
                app.prompt.files.enabled = enabled;
            } else if name == TOOLS {
                app.prompt.tools.enabled = enabled;
            } else if let Some(field) = app.prompt.get_mut(name) {
                field.0 = enabled;
            }
        }
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
            return select_prompt_file(entry, &mut app.settings.selected_preamble, |result| {
                Message::Prompt(PromptEvent::PreambleFileResult(result))
            });
        }
        PromptEvent::PreambleFileResult(Ok(content)) => {
            app.prompt.preamble.1 = content;
        }
        PromptEvent::PreambleFileResult(Err(_)) => {}
        PromptEvent::SelectRules(entry) => {
            return select_prompt_file(entry, &mut app.settings.selected_rules, |result| {
                Message::Prompt(PromptEvent::RulesFileResult(result))
            });
        }
        PromptEvent::RulesFileResult(Ok(content)) => {
            app.prompt.rules.1 = content;
        }
        PromptEvent::RulesFileResult(Err(_)) => {}
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

/// Bump `path` to top of recents, persist it as current workspace,
/// restore its agents_md_enabled preference, rebuild the files tree, and refresh the session list.
pub(crate) fn set_workspace(app: &mut App, path: PathBuf) -> Task<Message> {
    // Save current workspace preference before switching.
    let cur = &app.prompt.workspace.1;
    if !cur.as_os_str().is_empty() {
        let enabled = app.prompt.agents_md.0;
        app.settings.set_recent_workspace_enabled(cur, enabled);
    }

    // Move the new workspace to the front of recents.
    let (exists, content) = load_agents_md(&path);
    let enabled = app
        .settings
        .recent_workspaces
        .iter()
        .find_map(|(p, e)| (p == &path).then_some(*e))
        .unwrap_or(true)
        && exists;
    app.settings.recent_workspaces.retain(|(p, _)| p != &path);
    app.settings
        .recent_workspaces
        .insert(0, (path.clone(), enabled));
    app.settings.recent_workspaces.truncate(10);

    // Apply workspace.
    let tree = workspace::build_files_tree(&path);
    app.prompt.files.content = text_editor::Content::with_text(&tree);
    app.prompt.agents_md_exists = exists;
    app.prompt.agents_md = (enabled, content);
    app.prompt.workspace.1 = path.clone();
    app.prompt.workspace_options =
        crate::views::build_workspace_options(&app.settings.recent_workspaces);
    crate::app::conversation::refresh_session_list(path)
}

/// Read a prompt file (preamble or rules) from disk and return a task
/// that produces the appropriate `FileResult` message.
pub(crate) fn select_prompt_file(
    entry: FilepathEntry,
    selected: &mut String,
    on_load: fn(Result<String, String>) -> Message,
) -> Task<Message> {
    let FilepathEntry { display, path } = entry;
    *selected = display;
    Task::perform(
        async move { std::fs::read_to_string(&path).map_err(|e| e.to_string()) },
        on_load,
    )
}

/// Read `AGENTS.md` from the workspace root, returning (exists, content).
pub(crate) fn load_agents_md(workspace: &std::path::Path) -> (bool, String) {
    if !workspace.as_os_str().is_empty() {
        let path = workspace.join("AGENTS.md");
        if path.is_file() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            return (true, content);
        }
    }
    (false, String::new())
}
