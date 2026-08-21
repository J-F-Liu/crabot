//! The [`ToolRegistry`]: owned registry of all tools (built-in, custom, and
//! MCP-discovered), plus genai declaration and unknown-tool helpers.

use genai::chat::Tool as GenaiTool;
use std::collections::HashSet;
use std::sync::Arc;

use super::builtin::{
    ask, bash, edit, fetch, find, process, read, renew, search, task, todo, write,
};
use super::tool::{Tool, ToolRef};
use super::{custom, mcp};

/// Owned registry of all tools (built-in, custom, and MCP-discovered).
pub struct ToolRegistry {
    pub builtin: Vec<ToolRef>,
    pub custom: Vec<custom::CustomTool>,
    /// MCP tools grouped by server name: `(server_name, tools)`.
    pub mcp: Vec<(String, Vec<mcp::McpTool>)>,
    pub builtin_names: Vec<String>,
    pub custom_names: Vec<String>,
    pub mcp_servers: Vec<mcp::McpServer>,
    /// Shared todo list — written by the `todo` tool, read by the right pane.
    pub todo_items: todo::TodoList,
}

impl ToolRegistry {
    /// Create a new registry pre-populated with the twelve built-in tools.
    pub fn new() -> Self {
        let todo_items: todo::TodoList = todo::create_todo_list(Vec::new());
        let builtin: Vec<ToolRef> = vec![
            Arc::new(read::ReadTool),
            Arc::new(write::WriteTool),
            Arc::new(edit::EditTool),
            Arc::new(find::FindTool),
            Arc::new(search::SearchTool),
            Arc::new(bash::BashTool),
            Arc::new(process::ProcessTool),
            Arc::new(ask::AskTool),
            Arc::new(todo::TodoTool::new(Arc::clone(&todo_items))),
            Arc::new(task::TaskTool),
            Arc::new(renew::RenewTool),
            Arc::new(fetch::FetchTool),
        ];
        Self {
            builtin_names: builtin.iter().map(|t| t.name().to_string()).collect(),
            builtin,
            custom: Vec::new(),
            mcp: Vec::new(),
            custom_names: Vec::new(),
            mcp_servers: Vec::new(),
            todo_items,
        }
    }

    /// Replace the custom tools in the registry.
    pub fn register_custom(&mut self, tool_list: custom::ToolList) {
        self.custom_names = tool_list
            .custom_tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        self.custom = tool_list.custom_tools;
    }

    /// Add or replace one MCP server's tools in the registry.
    pub fn register_mcp_group(&mut self, server_name: String, tools: Vec<mcp::McpTool>) {
        upsert_group(&mut self.mcp, server_name, tools);
    }

    /// Remove a server's tools from the registry, returning the removed tool names.
    pub fn unregister_mcp_group(&mut self, server_name: &str) -> Vec<String> {
        remove_group(&mut self.mcp, server_name)
            .into_iter()
            .flatten()
            .map(|t| t.name)
            .collect()
    }

    /// Look up an MCP server config by name.
    pub fn find_mcp_server(&self, server: &str) -> Option<mcp::McpServer> {
        self.mcp_servers.iter().find(|s| s.name == server).cloned()
    }

    /// Return a clone of all custom tool names.
    pub fn custom_names(&self) -> Vec<String> {
        self.custom_names.to_vec()
    }

    /// Return names of all registered tools (built-in + custom + MCP).
    pub fn all_names(&self) -> impl Iterator<Item = &String> {
        self.builtin_names
            .iter()
            .chain(self.custom_names.iter())
            .chain(
                self.mcp
                    .iter()
                    .flat_map(|(_s, tools)| tools.iter().map(|t| &t.name)),
            )
    }

    /// Whether any enabled tool belongs to the given MCP server.
    pub fn mcp_server_has_enabled_tool(&self, server: &str, enabled: &HashSet<String>) -> bool {
        self.mcp
            .iter()
            .any(|(s, tools)| s == server && tools.iter().any(|t| enabled.contains(&t.name)))
    }

    /// Return a snapshot of the current todo list.
    pub fn snapshot_todo(&self) -> Vec<todo::TodoItem> {
        self.todo_items
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default()
    }

    /// Clear all todo items.
    pub fn clear_todo(&self) {
        if let Ok(mut items) = self.todo_items.lock() {
            items.clear();
        }
    }

    /// Collect every tool whose name appears in `enabled`.
    /// MCP tools are further filtered by `enabled_servers` (server name must be present).
    pub fn enabled_tools(
        &self,
        enabled: &HashSet<String>,
        enabled_servers: &HashSet<String>,
    ) -> Vec<ToolRef> {
        let mut tools: Vec<ToolRef> = Vec::new();
        for tool in self.builtin.iter() {
            if enabled.contains(tool.name()) {
                tools.push(Arc::clone(tool));
            }
        }
        for t in &self.custom {
            if enabled.contains(&t.name) {
                tools.push(Arc::new(t.clone()));
            }
        }
        for (server, group) in &self.mcp {
            if enabled_servers.contains(server) {
                for t in group {
                    if enabled.contains(&t.name) {
                        tools.push(Arc::new(t.clone()));
                    }
                }
            }
        }
        tools
    }

    /// Look up a tool by name across builtin, custom, and MCP groups.
    /// Returns a reference-counted tool for execution.
    pub fn find_tool(&self, name: &str) -> Option<ToolRef> {
        // Search builtin tools.
        for tool in self.builtin.iter() {
            if tool.name() == name {
                return Some(Arc::clone(tool));
            }
        }
        // Search custom tools.
        for tool in &self.custom {
            if tool.name() == name {
                return Some(Arc::new(tool.clone()));
            }
        }
        // Search MCP tools.
        for (_server, tools) in &self.mcp {
            for tool in tools {
                if tool.name() == name {
                    return Some(Arc::new(tool.clone()));
                }
            }
        }
        None
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Replace the group named `name` in `groups`, or append it.
fn upsert_group<T>(groups: &mut Vec<(String, Vec<T>)>, name: String, items: Vec<T>) {
    match groups.iter_mut().find(|(n, _)| *n == name) {
        Some(existing) => *existing = (name, items),
        None => groups.push((name, items)),
    }
}

/// Remove the group named `name` from `groups`, returning its items.
fn remove_group<T>(groups: &mut Vec<(String, Vec<T>)>, name: &str) -> Option<Vec<T>> {
    let pos = groups.iter().position(|(n, _)| n == name)?;
    Some(groups.remove(pos).1)
}

/// Build the genai tools list from a set of tool refs.
pub fn build_tools(tools: &[ToolRef], strict: bool) -> Vec<GenaiTool> {
    tools.iter().map(|t| t.tool_declaration(strict)).collect()
}

/// Build a helpful error message when an unknown tool is requested.
pub fn unknown_tool_message(name: &str) -> String {
    let hint = match name {
        "grep" => Some("use the search tool instead"),
        "cat" => Some("use the read tool instead"),
        "ls" | "dir" => Some("use the find or bash tool instead"),
        "mv" | "cp" | "rm" | "mkdir" => Some("use the bash tool instead"),
        "curl" | "wget" => Some("use the fetch tool instead"),
        "git" => Some("use the bash tool instead"),
        _ => None,
    };

    match hint {
        Some(suggestion) => format!("Unknown tool: {name} — {suggestion}"),
        None => format!("Unknown tool: {name}"),
    }
}
