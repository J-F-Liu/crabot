use iced::Task;

use crate::app::{App, Message, ToolEvent};
use crabot::HashSetExt;
use crabot::tools;

pub(crate) fn update(app: &mut App, event: ToolEvent) -> Task<Message> {
    match event {
        ToolEvent::ToggleMcpServer(server, enabled) => {
            if enabled {
                app.tools.enabled_mcp_servers.set(server.clone(), true);
                if !tools::mcp::has_connection(&server)
                    && let Some(server_config) = app.tools.tool_registry.find_mcp_server(&server)
                {
                    return Task::perform(
                        async move { tools::mcp::discover_mcp_server(server_config).await },
                        |result| Message::Tools(crate::app::ToolEvent::McpToolsDiscovered(result)),
                    );
                }
            } else {
                tools::mcp::drop_connection(&server);
                app.tools.enabled_mcp_servers.set(server.clone(), false);
            }
            app.refresh_tools_summary();
        }
        ToolEvent::ToggleAgentTool(tool_name, enabled) => {
            app.tools.enabled_tools.set(tool_name, enabled);
            app.refresh_tools_summary();
        }
        ToolEvent::McpToolsDiscovered((server_name, discovered_tools)) => {
            if discovered_tools.is_empty() {
                app.tools.enabled_mcp_servers.remove(&server_name);
                tools::mcp::drop_connection(&server_name);
            } else {
                if !app.tools.enabled_mcp_servers.contains(&server_name) {
                    tools::mcp::drop_connection(&server_name);
                }
                let new_names: Vec<_> = discovered_tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect();
                app.tools
                    .tool_registry
                    .register_mcp_group(server_name, discovered_tools);
                app.tools.enabled_tools.extend(
                    new_names
                        .into_iter()
                        .filter(|name| app.settings.is_tool_enabled(name)),
                );
            }
            app.refresh_tools_summary();
        }
    }
    Task::none()
}
