mod bash;
mod edit;
mod glob;
mod grep;
mod read;
mod write;

use genai::chat::Tool;

use indexmap::IndexMap;
use serde_json::Value;
// ── DevTools ────────────────────────────────────────────────────────

/// The six coding-agent devtools exposed to the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DevTool {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Bash,
}

impl DevTool {
    pub const ALL: &[DevTool] = &[
        Self::Read,
        Self::Write,
        Self::Edit,
        Self::Glob,
        Self::Grep,
        Self::Bash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Glob => "glob",
            Self::Grep => "grep",
            Self::Bash => "bash",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Read => "Read a file from the filesystem.",
            Self::Write => "Write content to a file.",
            Self::Edit => "Replace an exact string in a file with another.",
            Self::Glob => "Find files matching a glob pattern.",
            Self::Grep => "Search for a regular expression in files.",
            Self::Bash => "Execute a shell command.",
        }
    }

    /// Full tool declaration suitable for genai ChatRequest.
    pub fn tool_declaration(self) -> Tool {
        Tool::new(self.name())
            .with_description(self.description())
            .with_schema(schema(self))
    }

    /// Build the tools list for genai ChatRequest from selected tools.
    pub fn build_tools(selected: &IndexMap<DevTool, bool>) -> Vec<Tool> {
        selected
            .iter()
            .filter(|(_, enabled)| **enabled)
            .map(|(tool, _)| tool.tool_declaration())
            .collect()
    }

    /// Execute this tool with the given JSON arguments and workspace root.
    pub fn execute(self, args: &Value, workspace: &std::path::Path) -> Result<String, String> {
        match self {
            Self::Read => read::execute(args, workspace),
            Self::Write => write::execute(args, workspace),
            Self::Edit => edit::execute(args, workspace),
            Self::Glob => glob::execute(args, workspace),
            Self::Grep => grep::execute(args, workspace),
            Self::Bash => bash::execute(args, workspace),
        }
    }

    /// Parse tool call from name string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "edit" => Some(Self::Edit),
            "glob" => Some(Self::Glob),
            "grep" => Some(Self::Grep),
            "bash" => Some(Self::Bash),
            _ => None,
        }
    }
}

// ── schema dispatch ────────────────────────────────────────────────

fn schema(tool: DevTool) -> Value {
    match tool {
        DevTool::Read => read::schema(),
        DevTool::Write => write::schema(),
        DevTool::Edit => edit::schema(),
        DevTool::Glob => glob::schema(),
        DevTool::Grep => grep::schema(),
        DevTool::Bash => bash::schema(),
    }
}

// ── shared helpers ─────────────────────────────────────────────────

pub(crate) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

pub(crate) fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

pub(crate) fn resolve_path(path: &str, workspace: &std::path::Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // On Windows, a path like "/c/Users/..." is Unix‑style absolute but
    // Path::is_absolute() returns false without a prefix. Convert it.
    #[cfg(windows)]
    if path.starts_with('/') {
        let mut components = path[1..].splitn(2, '/');
        if let Some(drive) = components.next() {
            if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
                let rest = components.next().unwrap_or("");
                return std::path::PathBuf::from(format!(
                    "{}:\\{}",
                    drive.to_ascii_uppercase(),
                    rest.replace('/', "\\")
                ));
            }
        }
        // Normalise slashes on Windows – Path handles both, but tools may not.
        return std::path::PathBuf::from(path.replace('/', "\\"));
    }
    workspace.join(p)
}
