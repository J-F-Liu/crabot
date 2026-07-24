use iced::{Task, widget::text_editor};

use std::path::PathBuf;

use crabot::system::{FilepathEntry, TOOLS, WORKSPACE, WORKSPACE_TREE};
use crabot::workspace;

use crate::app::{App, FocusedTarget, Message, PromptEvent};

pub(crate) fn update(app: &mut App, event: PromptEvent) -> Task<Message> {
    match event {
        PromptEvent::ToggleEnabled(name, enabled) => {
            if name == WORKSPACE {
                app.prompt.system_prompt.workspace.0 = enabled;
            } else if name == WORKSPACE_TREE {
                app.settings.files_enabled = enabled;
            } else if let Some(field) = app.prompt.system_prompt.get_mut(name) {
                field.0 = enabled;
            }
        }
        PromptEvent::ToggleExpanded(name) => {
            if name == WORKSPACE_TREE {
                app.prompt.files_expanded = !app.prompt.files_expanded;
            } else if name == TOOLS {
                app.prompt.tools_expanded = !app.prompt.tools_expanded;
            } else {
                app.tools.tool_list_state.update(name);
            }
        }
        PromptEvent::EditTextField(name, value) => {
            if let Some(field) = app.prompt.system_prompt.get_mut(name) {
                field.1 = value;
            }
        }
        PromptEvent::EditTextContent(name, action) => {
            if matches!(action, text_editor::Action::Click(_)) {
                app.layout.focused = Some(FocusedTarget::EditText(name));
            }
            let Some(content) = content_mut(app, name) else {
                return Task::none();
            };
            content.perform(action);
            let text = content.text();
            if let Some(field) = app.prompt.system_prompt.get_mut(name) {
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
            app.prompt.system_prompt.preamble.1 = content;
        }
        PromptEvent::PreambleFileResult(Err(_)) => {}
        PromptEvent::SelectRules(entry) => {
            return select_prompt_file(entry, &mut app.settings.selected_rules, |result| {
                Message::Prompt(PromptEvent::RulesFileResult(result))
            });
        }
        PromptEvent::RulesFileResult(Ok(content)) => {
            app.prompt.system_prompt.rules.1 = content;
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
            if let Some(recipes) = app.settings.prompt_recipe.get(&mode_key)
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
    let cur = &app.prompt.system_prompt.workspace.1;
    if !cur.as_os_str().is_empty() {
        let enabled = app.prompt.system_prompt.agents_md.0;
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
    app.prompt.files_content = text_editor::Content::with_text(&tree);
    app.prompt.agents_md_exists = exists;
    app.prompt.system_prompt.agents_md = (enabled, content);
    app.overlay.show_restart = std::env::current_exe()
        .ok()
        .is_some_and(|exe| exe.starts_with(&path));
    app.prompt.system_prompt.workspace.1 = path.clone();
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

pub(crate) fn content_mut<'a>(
    app: &'a mut App,
    name: &str,
) -> Option<&'a mut text_editor::Content> {
    match name {
        TOOLS => Some(&mut app.prompt.tools_content),
        WORKSPACE_TREE => Some(&mut app.prompt.files_content),
        _ => None,
    }
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
