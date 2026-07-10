//! MCP (Model Context Protocol) client manager.
//!
//! Auto-discovers tools from configured MCP servers defined in
//! `~/.crabot/mcp.ron` and makes them available as `Tool` implementations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientInfo, ContentBlock, Implementation,
    ResourceContents,
};
use rmcp::service::{Peer, RoleClient, RunningService, ServiceExt as _};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransport;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use shell_words::split;
use tokio::process::Command;

use super::Tool;

/// Timeout for establishing an MCP connection (process spawn + JSON-RPC
/// initialize handshake). `npx`-based servers may need to download packages on
/// first run, so this is deliberately generous.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout for a single MCP tool invocation. Long-running tools (web fetch,
/// browser automation, …) can take a while, but the call is still bounded so a
/// hung server can't block an agent turn forever.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);

// ── Configuration types ──────────────────────────────────────────────

/// Transport method for an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransport {
    /// Stdio transport (spawns a child process).
    Stdio {
        /// The command to spawn (e.g. "npx -y @org/server").
        cmd: String,
        /// Extra environment variables for the child process.
        #[serde(default)]
        env_vars: HashMap<String, String>,
    },
    /// Streamable HTTP transport.
    Http {
        /// The server URL (e.g. "http://localhost:8000/mcp").
        url: String,
        /// Custom HTTP headers to include with every request.
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

/// A single MCP server definition in `~/.crabot/mcp.ron`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    /// Human-readable identifier for this server.
    pub name: String,
    /// Transport method used to communicate with the MCP server.
    pub transport: McpTransport,
    /// If true, tool names are prefixed with `{server_name}_`.
    pub qualify_tool_names: bool,
    /// Prompt text injected into the system prompt when the server is enabled.
    #[serde(default)]
    pub prompt: String,
}

/// Persistable list of configured MCP servers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpList {
    pub servers: Vec<McpServer>,
}

impl McpList {
    /// Path to `~/.crabot/mcp.ron`.
    pub fn path() -> PathBuf {
        home::home_dir()
            .unwrap_or_default()
            .join(".crabot")
            .join("mcp.ron")
    }

    /// Load from disk, returning empty list if missing or malformed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => ron::from_str::<McpList>(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Return all server names.
    #[allow(dead_code)]
    pub fn server_names(&self) -> Vec<String> {
        self.servers.iter().map(|s| s.name.clone()).collect()
    }
}

// ── McpTool: wraps a remote MCP tool as a local `Tool` ───────────────

/// A tool discovered from an MCP server, implementing the local `Tool` trait.
///
/// Tool calls are dispatched to the remote server via the retained `Peer`.
#[derive(Clone)]
pub struct McpTool {
    /// Qualified name (e.g. `"filesystem_read_file"` or bare name).
    /// This is what the LLM sees and uses when calling the tool.
    pub name: String,
    /// Original tool name, used in execute requests to the remote server.
    pub remote_name: String,
    /// A human-readable title for the tool, if provided by the server.
    pub title: Option<String>,
    /// Description from the remote tool.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub schema: Value,
    /// Handle for calling the remote server.
    peer: Peer<RoleClient>,
}

impl McpTool {
    fn new(
        server_name: &str,
        qualify: bool,
        remote: rmcp::model::Tool,
        peer: Peer<RoleClient>,
    ) -> Self {
        let remote_name = remote.name.to_string();
        let name = if qualify {
            format!("{server_name}_{remote_name}")
        } else {
            remote_name.clone()
        };
        let description = remote.description.unwrap_or_else(|| "MCP tool".into());
        // Convert `Arc<JsonObject>` → `Value`
        let schema = Value::Object(remote.input_schema.as_ref().clone());

        Self {
            name,
            remote_name,
            title: remote.title,
            description: description.into_owned(),
            schema,
            peer,
        }
    }
    /// Display label: the MCP `title` if provided, otherwise the bare name.
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.remote_name)
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn instruction(&self) -> &str {
        ""
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn execute(&self, args: &Value, _workspace: &Path) -> Result<String, String> {
        let arguments: Map<String, Value> = args.as_object().cloned().unwrap_or_default();

        let params = CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);

        let peer = self.peer.clone();
        let handle = tokio::runtime::Handle::current();
        let name = self.name.clone();

        let result: Result<rmcp::model::CallToolResult, String> =
            tokio::task::block_in_place(move || {
                handle.block_on(async move {
                    match tokio::time::timeout(CALL_TIMEOUT, peer.call_tool(params)).await {
                        Ok(Ok(result)) => Ok(result),
                        Ok(Err(e)) => Err(e.to_string()),
                        Err(_) => Err(format!(
                            "MCP tool '{name}' timed out after {}s",
                            CALL_TIMEOUT.as_secs()
                        )),
                    }
                })
            });

        let result = result?;

        if result.is_error == Some(true) {
            let text = result
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Err(if text.is_empty() {
                format!("Tool '{}' returned an error", self.name)
            } else {
                text
            });
        }

        Ok(format_call_tool_result(&result))
    }
}

/// Format a `CallToolResult` into a human-readable string.
fn format_call_tool_result(result: &rmcp::model::CallToolResult) -> String {
    let mut out = String::new();
    for block in &result.content {
        match block {
            ContentBlock::Text(t) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&t.text);
            }
            ContentBlock::Image(_) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("[image content]");
            }
            ContentBlock::Audio(_) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("[audio content]");
            }
            ContentBlock::Resource(r) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                let mime = match &r.resource {
                    ResourceContents::TextResourceContents { mime_type, .. } => {
                        mime_type.as_deref().unwrap_or("text/plain")
                    }
                    ResourceContents::BlobResourceContents { mime_type, .. } => {
                        mime_type.as_deref().unwrap_or("application/octet-stream")
                    }
                    _ => "unknown",
                };
                let _ = std::fmt::Write::write_fmt(
                    &mut out,
                    format_args!("[embedded resource: mime={mime}]"),
                );
            }
            ContentBlock::ResourceLink(r) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                let _ = std::fmt::Write::write_fmt(
                    &mut out,
                    format_args!("[resource link: {}]", r.uri),
                );
            }
            _ => {}
        }
    }

    if out.is_empty()
        && let Some(ref sc) = result.structured_content
    {
        out = sc.to_string();
    }

    if out.is_empty() {
        out = "(empty result)".to_string();
    }

    super::truncate_output(out)
}

// ── Connection manager ───────────────────────────────────────────────

/// Retained MCP connections, kept alive for the lifetime of the process.
///
/// Each [`McpConnection`] owns a `RunningService` whose `DropGuard` cancels the
/// background JSON-RPC task and closes the transport (killing stdio child
/// processes) when dropped. The [`Peer`] handles stored inside each [`McpTool`]
/// are *not* enough to keep a connection alive — they only hold an `mpsc`
/// sender whose receiver lives in the dropped service task. Without retaining
/// the connections here, every discovered tool would be dead on arrival, with
/// all `call_tool` requests failing as `TransportClosed`.
static MCP_CONNECTIONS: LazyLock<Mutex<Vec<McpConnection>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// State for an MCP connection.
pub struct McpConnection {
    /// Keep the service alive so the connection persists.
    _service: RunningService<RoleClient, ClientInfo>,
    /// Peer handle for making tool calls / listing tools.
    pub peer: Peer<RoleClient>,
    /// Name of the server.
    pub server_name: String,
    /// Whether to qualify tool names with the server name.
    pub qualify: bool,
}

impl McpConnection {
    pub fn peer(&self) -> Peer<RoleClient> {
        self.peer.clone()
    }

    /// List all tools from this server's peer, handling pagination automatically.
    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, rmcp::service::ServiceError> {
        self.peer.list_all_tools().await
    }
}

/// Helper to build a `ClientInfo` for the MCP handshake.
fn make_client_info() -> ClientInfo {
    ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::from_build_env(),
    )
}

impl McpServer {
    /// Connect to this MCP server and return a `McpConnection`.
    pub async fn connect(&self) -> Result<McpConnection, String> {
        match &self.transport {
            McpTransport::Stdio { cmd, env_vars } => connect_stdio(self, cmd, env_vars).await,
            McpTransport::Http { url, headers } => connect_http(self, url, headers).await,
        }
    }
}

async fn connect_stdio(
    server: &McpServer,
    command: &str,
    env_vars: &HashMap<String, String>,
) -> Result<McpConnection, String> {
    let parts =
        split(command).map_err(|e| format!("Failed to parse command '{}': {e}", command))?;
    let (exe, args) = parts
        .split_first()
        .ok_or_else(|| format!("Empty command for server '{}'", server.name))?;

    let mut cmd = Command::new(exe);
    cmd.args(args);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    // Prevent a visible console window from flashing on Windows.
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

    // Use the builder so we can force stderr to null.
    // TokioChildProcess::new() defaults stderr to inherit(), which would
    // leak child process errors into the parent console.
    let transport = TokioChildProcess::builder(cmd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|(proc, _stderr)| proc)
        .map_err(|e| format!("Failed to spawn '{}': {e}", server.name))?;

    let service = make_client_info()
        .serve(transport)
        .await
        .map_err(|e| format!("Failed to connect to '{}': {e}", server.name))?;

    let peer = service.peer().clone();
    Ok(McpConnection {
        _service: service,
        peer,
        server_name: server.name.clone(),
        qualify: server.qualify_tool_names,
    })
}

async fn connect_http(
    server: &McpServer,
    url: &str,
    headers: &HashMap<String, String>,
) -> Result<McpConnection, String> {
    use http::{HeaderName, HeaderValue};

    let custom_headers: HashMap<HeaderName, HeaderValue> = headers
        .iter()
        .filter_map(|(k, v)| {
            let name = HeaderName::from_bytes(k.as_bytes()).ok()?;
            let value = HeaderValue::from_str(v).ok()?;
            Some((name, value))
        })
        .collect();

    let config =
        rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url)
            .custom_headers(custom_headers);

    let transport = StreamableHttpClientTransport::from_config(config);

    let service = make_client_info()
        .serve(transport)
        .await
        .map_err(|e| format!("Failed to connect to '{}': {e}", server.name))?;

    let peer = service.peer().clone();
    Ok(McpConnection {
        _service: service,
        peer,
        server_name: server.name.clone(),
        qualify: server.qualify_tool_names,
    })
}

/// Connect to a single MCP server, discover its tools, and return
/// `McpTool` wrappers grouped under the server name.
pub async fn discover_mcp_server(server: McpServer) -> (String, Vec<McpTool>) {
    let server_name = server.name.clone();
    let qualify = server.qualify_tool_names;
    let connect_result = tokio::time::timeout(CONNECT_TIMEOUT, server.connect()).await;
    let conn = match connect_result {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => {
            eprintln!("Failed to connect to MCP server '{server_name}': {e}");
            return (server_name, vec![]);
        }
        Err(_) => {
            eprintln!(
                "Timed out connecting to MCP server '{server_name}' after {}s",
                CONNECT_TIMEOUT.as_secs()
            );
            return (server_name, vec![]);
        }
    };

    let list_result = tokio::time::timeout(CONNECT_TIMEOUT, conn.list_tools()).await;
    match list_result {
        Ok(Ok(tools)) => {
            let peer = conn.peer();
            let mcp_tools: Vec<McpTool> = tools
                .into_iter()
                .map(|remote_tool| McpTool::new(&server_name, qualify, remote_tool, peer.clone()))
                .collect();
            // Keep the connection alive for the lifetime of the tools.
            if let Ok(mut conns) = MCP_CONNECTIONS.lock() {
                conns.push(conn);
            }
            (server_name, mcp_tools)
        }
        Ok(Err(e)) => {
            eprintln!("Failed to list tools from MCP server '{server_name}': {e}");
            (server_name, vec![])
        }
        Err(_) => {
            eprintln!(
                "Timed out listing tools from MCP server '{server_name}' after {}s",
                CONNECT_TIMEOUT.as_secs()
            );
            (server_name, vec![])
        }
    }
}
