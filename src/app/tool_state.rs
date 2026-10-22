use std::collections::HashSet;

use crate::app::{App, Message, ToolEvent};
use crabot::HashSetExt;
use crabot::tools;
use iced::Task;

/// Drop `removed` tool names from `enabled` and add `added` ones.
pub(crate) fn apply_names(enabled: &mut HashSet<String>, removed: &[String], added: &[String]) {
    enabled.retain(|name| !removed.contains(name));
    enabled.extend(added.iter().cloned());
}

/// Apply a tool-name change to every tab's option snapshot.
pub(crate) fn sync_tab_tool_names(app: &mut App, removed: &[String], added: &[String]) {
    for tab in &mut app.conversation.session_tabs {
        apply_names(&mut tab.pane.enabled_tools, removed, added);
    }
}

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
                    return app.tools.start_discovery(config);
                }
            } else {
                // Keep the connection while another tab enables the server
                // (they are process-wide); bump the epoch so an in-flight
                // discovery can't re-add the tools.
                let still_used =
                    app.conversation
                        .session_tabs
                        .iter()
                        .enumerate()
                        .any(|(pos, tab)| {
                            pos != app.conversation.viewing
                                && tab.pane.enabled_mcp_servers.contains(&server)
                        });
                if !still_used {
                    tools::mcp::drop_connection(&server);
                }
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
                    // A reconnect re-lists known tools: keep their per-tab state.
                    let known: HashSet<String> = app
                        .tools
                        .tool_registry
                        .mcp
                        .iter()
                        .find(|(name, _)| name == &server)
                        .map(|(_, group)| group.iter().map(|t| t.name.clone()).collect())
                        .unwrap_or_default();
                    let added: Vec<String> = discovered
                        .iter()
                        .map(|t| t.name.clone())
                        .filter(|n| !known.contains(n) && app.settings.is_tool_enabled(n))
                        .collect();
                    app.tools
                        .tool_registry
                        .register_mcp_group(server, discovered);
                    app.tools.enabled_tools.extend(added.iter().cloned());
                    sync_tab_tool_names(app, &[], &added);
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
