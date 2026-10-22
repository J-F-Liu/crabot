use std::collections::{HashMap, HashSet};

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

/// The new name of `name`, or itself when it is not renamed.
fn renamed(name: &str, renames: &HashMap<String, String>) -> String {
    renames
        .get(name)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

/// Move `enabled` entries onto their renamed names.
fn rename_tools(enabled: &mut HashSet<String>, renames: &HashMap<String, String>) {
    *enabled = enabled.iter().map(|name| renamed(name, renames)).collect();
}

/// Apply a tool-name mapping to every tab's option snapshot.
fn sync_tab_tool_renames(app: &mut App, renames: &HashMap<String, String>) {
    for tab in &mut app.conversation.session_tabs {
        rename_tools(&mut tab.pane.enabled_tools, renames);
    }
}

/// Whether the saved config needs a fresh connection and tool listing.
fn is_reconfigured(old: &tools::mcp::McpServer, new: &tools::mcp::McpServer) -> bool {
    old.transport != new.transport || old.qualify_tool_names != new.qualify_tool_names
}

/// Reload a reconfigured server, keeping its enabled state and tool picks.
fn reload_mcp_server(
    app: &mut App,
    old: &tools::mcp::McpServer,
    new: &tools::mcp::McpServer,
) -> Task<Message> {
    let tool_name = |tool: &tools::mcp::McpTool| {
        tools::mcp::tool_name(&new.name, new.qualify_tool_names, &tool.remote_name)
    };
    let group = app.tools.tool_registry.take_mcp_group(&old.name);
    let known = group.iter().map(&tool_name).collect();
    // Never move an enabled flag onto or off a name another tool still claims.
    let renames: HashMap<String, String> = group
        .iter()
        .map(|tool| (tool.name.clone(), tool_name(tool)))
        .filter(|(from, to)| {
            from != to
                && !app.tools.tool_registry.owns_name(from)
                && !app.tools.tool_registry.owns_name(to)
        })
        .collect();
    rename_tools(&mut app.tools.enabled_tools, &renames);
    sync_tab_tool_renames(app, &renames);
    tools::mcp::drop_connection(&old.name);
    app.tools.mcp_errors.remove(&old.name);
    app.tools.mcp_known_tools.insert(old.name.clone(), known);
    tracing::debug!(server = %old.name, renamed = renames.len(), "reloading MCP server");
    app.tools.start_discovery(new.clone())
}

/// Apply a saved MCP server list: drop removed servers, reload reconfigured
/// ones, and discover new ones. Refreshes the tools summary afterwards.
pub(crate) fn apply_saved_mcp_servers(
    app: &mut App,
    old_servers: &[tools::mcp::McpServer],
    servers: &[tools::mcp::McpServer],
) -> Task<Message> {
    let mut tasks: Vec<Task<Message>> = Vec::new();
    for old in old_servers {
        match servers.iter().find(|s| s.name == old.name) {
            Some(new) if is_reconfigured(old, new) => {
                tasks.push(reload_mcp_server(app, old, new));
            }
            Some(_) => {}
            None => {
                let removed = app.tools.drop_mcp_server(&old.name);
                sync_tab_tool_names(app, &removed, &[]);
            }
        }
    }
    // New servers get listed like at boot; an empty card would only fail.
    for new in servers {
        let is_new = old_servers.iter().all(|old| old.name != new.name);
        if is_new && new.transport.has_target() {
            tracing::debug!(server = %new.name, "discovering new MCP server");
            tasks.push(app.tools.start_discovery(new.clone()));
        }
    }
    app.refresh_tools_summary();
    Task::batch(tasks)
}

pub(crate) fn update(app: &mut App, event: ToolEvent) -> Task<Message> {
    match event {
        ToolEvent::ToggleMcpServer(server, enabled) => {
            // Keep the failure label while retrying: checked + reason means "trying".
            let failed = app.tools.mcp_errors.contains_key(&server);
            if enabled {
                app.tools.enabled_mcp_servers.set(server.clone(), true);
                // Re-discover after a failure even if a stale connection lingers.
                // A discovery already in flight for this server is awaited instead.
                if !app.tools.mcp_pending.contains(&server)
                    && (failed || !tools::mcp::has_connection(&server))
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
        ToolEvent::SetMcpServerTools(server, enabled) => {
            let names = app.tools.tool_registry.mcp_tool_names(&server);
            if enabled {
                app.tools.enabled_tools.extend(names);
            } else {
                // Never clear a name a built-in, custom, or other server still claims.
                for name in names {
                    if !app.tools.tool_registry.owns_name_except(&name, &server) {
                        app.tools.enabled_tools.remove(&name);
                    }
                }
            }
            app.refresh_tools_summary();
        }
        ToolEvent::CycleMcpColumns(server) => app.tools.tool_list_state.cycle_mcp_columns(&server),
        ToolEvent::ToggleMcpServerExpanded(server) => app
            .tools
            .tool_list_state
            .toggle_mcp_server_expanded(&server),
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
                // A superseded attempt may still have retained its connection.
                tools::mcp::discard_connection(&server, epoch);
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
                    // Known tools keep their state; only new ones use saved defaults.
                    let known = app
                        .tools
                        .mcp_known_tools
                        .remove(&server)
                        .unwrap_or_else(|| app.tools.tool_registry.mcp_tool_names(&server));
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
                    app.tools.mcp_known_tools.remove(&server);
                    // Keep the old group visible, annotated with the failure.
                    app.tools.mcp_errors.insert(server, failure);
                }
            }
            app.refresh_tools_summary();
        }
    }
    Task::none()
}
