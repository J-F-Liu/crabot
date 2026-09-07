use crate::app::{App, Message, ToolEvent, ToolState};
use crabot::HashSetExt;
use crabot::tools;
use iced::Task;

pub(crate) fn update(app: &mut App, event: ToolEvent) -> Task<Message> {
    match event {
        ToolEvent::ToggleMcpServer(server, enabled) => {
            // Keep the failure label while retrying: checked + reason means "trying".
            let failed = app.tools.mcp_errors.contains_key(&server);
            if enabled {
                app.tools.enabled_mcp_servers.set(server.clone(), true);
                // Re-discover after a failure even if a stale connection lingers.
                if (failed || !tools::mcp::has_connection(&server))
                    && let Some(config) = app.tools.tool_registry.find_mcp_server(&server)
                {
                    let epoch = app.tools.next_epoch(&server);
                    app.tools.mcp_pending.insert(server.clone());
                    return ToolState::discover_task(config, epoch);
                }
            } else {
                // Turning off drops the connection and clears any failure.
                // Bump the epoch so an in-flight discovery can't re-add tools.
                tools::mcp::drop_connection(&server);
                app.tools.enabled_mcp_servers.set(server.clone(), false);
                app.tools.mcp_errors.remove(&server);
                app.tools.mcp_pending.remove(&server);
                app.tools.next_epoch(&server);
            }
            app.refresh_tools_summary();
        }
        ToolEvent::ToggleAgentTool(tool_name, enabled) => {
            app.tools.enabled_tools.set(tool_name, enabled);
            app.refresh_tools_summary();
        }
        ToolEvent::McpToolsDiscovered((server, epoch, result)) => {
            // Drop superseded results or servers removed meanwhile.
            let stale = app
                .tools
                .mcp_discovery_epoch
                .get(&server)
                .copied()
                .unwrap_or(0)
                > epoch
                || app.tools.tool_registry.find_mcp_server(&server).is_none();
            if stale {
                return Task::none();
            }
            app.tools.mcp_pending.remove(&server);
            match result {
                Ok(discovered) => {
                    // Success clears failures; keep the connection only while enabled.
                    app.tools.mcp_errors.remove(&server);
                    if !app.tools.enabled_mcp_servers.contains(&server) {
                        tools::mcp::drop_connection(&server);
                    }
                    let names = discovered
                        .iter()
                        .map(|t| t.name.clone())
                        .collect::<Vec<_>>();
                    app.tools
                        .tool_registry
                        .register_mcp_group(server, discovered);
                    app.tools.enabled_tools.extend(
                        names
                            .into_iter()
                            .filter(|n| app.settings.is_tool_enabled(n)),
                    );
                }
                Err(failure) => {
                    app.tools.enabled_mcp_servers.remove(&server);
                    tools::mcp::drop_connection(&server);
                    // Keep the old group visible, annotated with the failure.
                    app.tools.mcp_errors.insert(server, failure);
                }
            }
            app.refresh_tools_summary();
        }
    }
    Task::none()
}
