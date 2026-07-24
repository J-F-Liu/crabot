use iced::Task;
use std::collections::HashSet;

use crate::app::{App, Message, ModelSettingsEvent};
use crate::tools;
use crate::views::model_config;

/// Open the settings dialog, loading working copies of all state.
pub(crate) fn open_settings(app: &mut App) -> Task<Message> {
    app.model_settings.settings_state.working_models = app.model_settings.provided_models.clone();
    app.model_settings
        .settings_state
        .load_tools(tools::custom::ToolList::load());
    app.model_settings
        .settings_state
        .load_mcp(tools::mcp::McpList::load());
    app.model_settings.settings_state.select_first_provider();
    if app.model_settings.settings_state.selected_tab
        == crate::views::settings::SettingsTab::ToolPlayground
    {
        app.model_settings.settings_state.load_playground_tools(
            crate::views::settings::tool_playground::build_playground_tools(
                &app.tools.tool_registry,
            ),
        );
    }
    app.model_settings.show_settings_dialog = true;
    maybe_fetch_models(&app.model_settings.settings_state).unwrap_or(Task::none())
}

/// Handle model-config events that don't open settings (selection changes).
pub(crate) fn handle_model_config(app: &mut App, event: model_config::Event) -> Task<Message> {
    if model_config::update(
        event,
        &mut app.model_settings.provided_models,
        &mut app.settings.selected_model,
    ) {
        app.model_settings.provided_models.save();
    }
    Task::none()
}

/// Handle a `SettingsEvent`, mutating form state and applying side effects.
pub(crate) fn handle_event(app: &mut App, event: crate::views::SettingsEvent) -> Task<Message> {
    match event {
        crate::views::SettingsEvent::Close => {
            app.model_settings.settings_state.update(event);
            app.model_settings.show_settings_dialog = false;
        }
        crate::views::SettingsEvent::SaveModels => {
            app.model_settings.settings_state.update(event);
            app.model_settings.provided_models =
                app.model_settings.settings_state.working_models.clone();
            app.model_settings.provided_models.save();
            // Refresh provider pick-list entries.
            app.model_settings.provider_entries = app
                .model_settings
                .provided_models
                .providers
                .iter()
                .map(|(id, p)| model_config::ProviderEntry {
                    id: id.clone(),
                    name: p.name.clone(),
                })
                .collect();
            // Re-validate the selected model label.
            {
                let model = app
                    .model_settings
                    .provided_models
                    .ensure_valid_name(&app.settings.selected_model);
                app.settings.selected_model = model;
            }
        }
        crate::views::SettingsEvent::SaveTools => {
            app.model_settings.settings_state.update(event);
            // Persist custom tools and sync the tool registry.
            let old_names: HashSet<String> =
                app.tools.tool_registry.custom_names().into_iter().collect();
            app.model_settings.settings_state.working_tools.save();
            app.tools
                .tool_registry
                .register_custom(app.model_settings.settings_state.working_tools.clone());
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
        crate::views::SettingsEvent::SaveMcp => {
            // Snapshot current servers to detect removals / reconfigurations.
            let old_servers = app.tools.tool_registry.mcp_servers.clone();
            app.model_settings.settings_state.update(event);
            // Persist MCP servers and sync the tool registry.
            app.model_settings.settings_state.working_mcp.save();
            app.tools.tool_registry.mcp_servers = app
                .model_settings
                .settings_state
                .working_mcp
                .servers
                .clone();
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
        crate::views::SettingsEvent::SelectProvider(_)
        | crate::views::SettingsEvent::RefreshModels => {
            app.model_settings.settings_state.update(event);

            return maybe_fetch_models(&app.model_settings.settings_state).unwrap_or(Task::none());
        }
        crate::views::SettingsEvent::ExecutePlaygroundTool => {
            return execute_playground_tool(app, event);
        }
        crate::views::SettingsEvent::PlaygroundToolResult(generation, _, is_todo) => {
            if app.model_settings.settings_state.playground_generation == generation {
                app.model_settings.settings_state.update(event);
                if is_todo {
                    app.tools.cached_todo_items = app.tools.tool_registry.snapshot_todo();
                }
            }
        }
        crate::views::SettingsEvent::SelectTab(tab) => {
            app.model_settings.settings_state.update(event);
            if tab == crate::views::settings::SettingsTab::ToolPlayground {
                app.model_settings.settings_state.refresh_playground_tools(
                    crate::views::settings::tool_playground::build_playground_tools(
                        &app.tools.tool_registry,
                    ),
                );
            }
        }
        _ => {
            let focus_new_label = matches!(event, crate::views::SettingsEvent::StartAddLabel);
            let focus_new_provider_name = matches!(event, crate::views::SettingsEvent::NewProvider);
            app.model_settings.settings_state.update(event);
            if focus_new_label {
                return iced::widget::operation::focus(crate::views::NEW_LABEL_INPUT_ID);
            }
            if focus_new_provider_name {
                return iced::widget::operation::focus(crate::views::NEW_PROVIDER_NAME_INPUT_ID);
            }
        }
    }
    Task::none()
}

fn execute_playground_tool(app: &mut App, event: crate::views::SettingsEvent) -> Task<Message> {
    // Clone data needed for execution *before* mutating state.
    let info = app
        .model_settings
        .settings_state
        .playground_selected_index
        .and_then(|i| {
            app.model_settings
                .settings_state
                .playground_tools
                .get(i)
                .cloned()
        });
    let param_values = app
        .model_settings
        .settings_state
        .playground_param_values
        .clone();

    let Some(info) = info else {
        return Task::none();
    };
    let Some(tool) = app.tools.tool_registry.find_tool(&info.name) else {
        return Task::done(Message::ModelSettings(ModelSettingsEvent::Settings(
            crate::views::SettingsEvent::PlaygroundToolResult(
                0,
                Err(format!("Tool '{}' not found in registry", info.name)),
                false,
            ),
        )));
    };

    // Now safe to update state (sets running, bumps generation, resets cancel).
    app.model_settings.settings_state.update(event);

    let generation = app.model_settings.settings_state.playground_generation;
    let args =
        crate::views::settings::tool_playground::build_params_json(&info.schema_raw, &param_values);
    let workspace = app.prompt.system_prompt.workspace.1.clone();
    let cancel = app.model_settings.settings_state.playground_cancel.clone();
    let is_todo = info.name == "todo";
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || tool.execute(&args, &workspace, &cancel))
                .await
                .unwrap_or_else(|e| Err(format!("Tool panicked: {e}")))
        },
        move |result| {
            Message::ModelSettings(ModelSettingsEvent::Settings(
                crate::views::SettingsEvent::PlaygroundToolResult(generation, result, is_todo),
            ))
        },
    )
}

/// Confirm a pending new-label input (Enter or focus loss).
pub(crate) fn confirm_pending_label(app: &mut App) {
    if app.model_settings.settings_state.is_adding_label() {
        app.model_settings
            .settings_state
            .update(crate::views::SettingsEvent::AddLabel);
    }
}

/// Return a model-fetch [`Task`] if provider needs refresh its model list.
fn maybe_fetch_models(state: &crate::views::SettingsState) -> Option<Task<Message>> {
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
            Message::ModelSettings(ModelSettingsEvent::Settings(
                crate::views::SettingsEvent::ModelsFetched(provider_id, result),
            ))
        },
    ))
}
