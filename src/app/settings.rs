use std::collections::HashSet;

use iced::Task;

use crate::app::{App, Message, ModelSettingsEvent};
use crate::tools;
use crate::views::model_config;
use crate::views::settings::{self, SettingsTab, UpdateCheck, about::HOMEPAGE};
use crate::views::update;
use crate::views::{NEW_LABEL_INPUT_ID, NEW_PROVIDER_NAME_INPUT_ID, SettingsEvent, SettingsState};

/// Open the settings dialog, loading working copies of all state.
pub(crate) fn open_settings(app: &mut App) -> Task<Message> {
    app.settings_dialog.working_models = app.models.clone();
    app.settings_dialog
        .load_prompt_recipes(app.settings.prompt_recipes.clone());
    app.settings_dialog
        .load_tools(tools::custom::ToolList::load());
    app.settings_dialog.load_mcp(tools::mcp::McpList::load());
    app.settings_dialog.select_first_provider();
    if app.settings_dialog.selected_tab == SettingsTab::ToolPlayground {
        app.settings_dialog.load_playground_tools(
            settings::tool_playground::build_playground_tools(&app.tools.tool_registry),
        );
    }
    // Load About tab state.
    app.settings_dialog.auto_check_updates = app.settings.auto_check_updates;
    app.settings_dialog.update_check = app
        .overlay
        .update_available
        .as_ref()
        .map(|v| UpdateCheck::Available(v.clone()))
        .unwrap_or(UpdateCheck::Idle);
    app.settings_dialog.open = true;
    maybe_fetch_models(&app.settings_dialog).unwrap_or(Task::none())
}

/// Handle model-config events that don't open settings (selection changes).
pub(crate) fn handle_model_config(app: &mut App, event: model_config::Event) -> Task<Message> {
    let tab = app.conversation.viewing_mut();
    if model_config::update(event, &mut app.models, &mut tab.selected_model) {
        app.models.save();
    }
    Task::none()
}

/// Handle a `SettingsEvent`, mutating form state and applying side effects.
pub(crate) fn handle_event(app: &mut App, event: SettingsEvent) -> Task<Message> {
    match event {
        SettingsEvent::Close => {
            app.settings_dialog.update(event);
            app.settings.auto_check_updates = app.settings_dialog.auto_check_updates;
            app.settings_dialog.open = false;
        }
        SettingsEvent::SaveModels => {
            app.settings_dialog.update(event);
            app.models = app.settings_dialog.working_models.clone();
            app.models.save();
            // Re-validate the selected model label.
            {
                let model = app.models.ensure_valid_name(&app.settings.selected_model);
                app.settings.selected_model = model;
            }
            // Validate every tab's selected_model against the updated model list.
            for tab in &mut app.conversation.session_tabs {
                tab.selected_model = app.models.ensure_valid_name(&tab.selected_model);
            }
        }
        SettingsEvent::SavePromptRecipes => {
            app.settings_dialog.update(event);
            app.settings.prompt_recipes = app.settings_dialog.working_prompt_recipes.clone();
            app.settings.save();
        }
        SettingsEvent::SaveTools => {
            app.settings_dialog.update(event);
            // Persist custom tools and sync the tool registry.
            let old_names: HashSet<String> =
                app.tools.tool_registry.custom_names().into_iter().collect();
            app.settings_dialog.working_tools.save();
            app.tools
                .tool_registry
                .register_custom(app.settings_dialog.working_tools.clone());
            let new_names: HashSet<String> =
                app.tools.tool_registry.custom_names().into_iter().collect();
            // Deleted tools lose their enabled state; new tools default to enabled.
            for name in old_names.difference(&new_names) {
                app.tools.enabled_tools.remove(name);
            }
            for name in new_names.difference(&old_names) {
                app.tools.enabled_tools.insert(name.clone());
            }
            app.refresh_tools_summary();
        }
        SettingsEvent::SaveMcp => {
            // Snapshot current servers to detect removals / reconfigurations.
            let old_servers = app.tools.tool_registry.mcp_servers.clone();
            app.settings_dialog.update(event);
            // Persist MCP servers and sync the tool registry.
            app.settings_dialog.working_mcp.save();
            app.tools.tool_registry.mcp_servers = app.settings_dialog.working_mcp.servers.clone();
            // Drop live connections whose server was removed or whose
            // connection-affecting config changed.
            for old in &old_servers {
                let stale = match app
                    .tools
                    .tool_registry
                    .mcp_servers
                    .iter()
                    .find(|s| s.name == old.name)
                {
                    Some(new) => {
                        new.transport != old.transport
                            || new.qualify_tool_names != old.qualify_tool_names
                    }
                    None => true,
                };
                if stale {
                    tools::mcp::drop_connection(&old.name);
                    app.tools.enabled_mcp_servers.remove(&old.name);
                    let stale_names = app.tools.tool_registry.unregister_mcp_group(&old.name);
                    for name in &stale_names {
                        app.tools.enabled_tools.remove(name);
                    }
                }
            }
            app.refresh_tools_summary();
        }
        SettingsEvent::SelectProvider(_) | SettingsEvent::RefreshModels => {
            app.settings_dialog.update(event);

            return maybe_fetch_models(&app.settings_dialog).unwrap_or(Task::none());
        }
        SettingsEvent::ExecutePlaygroundTool => {
            return execute_playground_tool(app, event);
        }
        SettingsEvent::PlaygroundToolResult(generation, _, is_todo) => {
            if app.settings_dialog.playground_generation == generation {
                app.settings_dialog.update(event);
                if is_todo {
                    let snapshot = app.tools.tool_registry.snapshot_todo();
                    if let Ok(mut items) = app.conversation.viewing_mut().todo_items.lock() {
                        *items = snapshot;
                    }
                }
            }
        }
        SettingsEvent::SelectTab(tab) => {
            app.settings_dialog.update(event);
            if tab == SettingsTab::ToolPlayground {
                app.settings_dialog.refresh_playground_tools(
                    settings::tool_playground::build_playground_tools(&app.tools.tool_registry),
                );
            }
        }
        SettingsEvent::CheckForUpdate => {
            app.settings_dialog.update(event);
            return Task::perform(update::check_for_updates(), |result| {
                Message::ModelSettings(ModelSettingsEvent::Settings(
                    SettingsEvent::UpdateCheckResult(result),
                ))
            });
        }
        SettingsEvent::OpenHomepage => {
            if let Err(error) = open::that(HOMEPAGE) {
                eprintln!("Failed to open homepage: {error}");
            }
        }
        SettingsEvent::ToggleAutoCheckUpdates(v) => {
            app.settings_dialog.update(event);
            app.settings.auto_check_updates = v;
        }
        SettingsEvent::UpdateCheckResult(latest) => {
            app.settings_dialog
                .update(SettingsEvent::UpdateCheckResult(latest.clone()));
            if let Some(version) = latest {
                app.settings.last_update_version = Some(version.clone());
                app.save_settings();
                app.overlay.update_available = Some(version);
            }
        }
        _ => {
            let focus_new_label = matches!(event, SettingsEvent::StartAddLabel);
            let focus_new_provider_name = matches!(event, SettingsEvent::NewProvider);
            app.settings_dialog.update(event);
            if focus_new_label {
                return iced::widget::operation::focus(NEW_LABEL_INPUT_ID);
            }
            if focus_new_provider_name {
                return iced::widget::operation::focus(NEW_PROVIDER_NAME_INPUT_ID);
            }
        }
    }
    Task::none()
}

fn execute_playground_tool(app: &mut App, event: SettingsEvent) -> Task<Message> {
    // Clone data needed for execution *before* mutating state.
    let info = app
        .settings_dialog
        .playground_selected_index
        .and_then(|i| app.settings_dialog.playground_tools.get(i).cloned());
    let param_values = app.settings_dialog.playground_param_values.clone();

    let Some(info) = info else {
        return Task::none();
    };
    let Some(tool) = app.tools.tool_registry.find_tool(&info.name) else {
        return Task::done(Message::ModelSettings(ModelSettingsEvent::Settings(
            SettingsEvent::PlaygroundToolResult(
                0,
                Err(format!("Tool '{}' not found in registry", info.name)),
                false,
            ),
        )));
    };

    // Now safe to update state (sets running, bumps generation, resets cancel).
    app.settings_dialog.update(event);

    let generation = app.settings_dialog.playground_generation;
    let args = settings::tool_playground::build_params_json(&info.schema_raw, &param_values);
    let workspace = app.prompt.workspace.1.clone();
    let cancel = app.settings_dialog.playground_cancel.clone();
    let is_todo = info.name == "todo";
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || tool.execute(&args, &workspace, &cancel))
                .await
                .unwrap_or_else(|e| Err(format!("Tool panicked: {e}")))
        },
        move |result| {
            Message::ModelSettings(ModelSettingsEvent::Settings(
                SettingsEvent::PlaygroundToolResult(generation, result, is_todo),
            ))
        },
    )
}

/// Confirm a pending new-label input (Enter or focus loss).
pub(crate) fn confirm_pending_label(app: &mut App) {
    if app.settings_dialog.is_adding_label() {
        app.settings_dialog.update(SettingsEvent::AddLabel);
    }
}

/// Return a model-fetch [`Task`] if provider needs refresh its model list.
fn maybe_fetch_models(state: &SettingsState) -> Option<Task<Message>> {
    if !state.needs_fetch() {
        return None;
    }
    let base_url = state.provider_base_url().to_string();
    if base_url.is_empty() {
        return None;
    }
    let api_key = state.provider_api_key().to_string();
    let provider_id = state.current_provider_id().to_string();
    Some(Task::perform(
        async move {
            let models = crabot::model::fetch_available_models(&base_url, &api_key).await;
            (provider_id, models)
        },
        |(provider_id, result)| {
            Message::ModelSettings(ModelSettingsEvent::Settings(SettingsEvent::ModelsFetched(
                provider_id,
                result,
            )))
        },
    ))
}
