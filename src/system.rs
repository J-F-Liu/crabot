use std::path::PathBuf;

pub const PREAMBLE: &str = "Preamble";
pub const RULES: &str = "Rules";
pub const TOOLS: &str = "Tools";
pub const WORKSPACE: &str = "Workspace";
pub const AGENTS_MD: &str = "AGENTS.md";
pub const WORKSPACE_TREE: &str = "Workspace tree";
pub const DATE: &str = "Date";

#[derive(Debug, Clone)]
pub struct FilepathEntry {
    pub display: String,
    pub path: PathBuf,
}

impl std::fmt::Display for FilepathEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display)
    }
}

impl PartialEq for FilepathEntry {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemPrompt {
    pub preamble: (bool, String),
    pub rules: (bool, String),
    pub tools: (bool, String),
    pub workspace: (bool, PathBuf),
    pub agents_md: (bool, String),
    pub files: (bool, String),
    pub date: (bool, String),
}

impl SystemPrompt {
    pub fn get_mut(&mut self, name: &str) -> Option<&mut (bool, String)> {
        match name {
            PREAMBLE => Some(&mut self.preamble),
            RULES => Some(&mut self.rules),
            TOOLS => Some(&mut self.tools),
            AGENTS_MD => Some(&mut self.agents_md),
            WORKSPACE_TREE => Some(&mut self.files),
            DATE => Some(&mut self.date),
            _ => None,
        }
    }

    /// Concatenate all enabled components, returning the full prompt string.
    pub fn get_prompt(&self) -> String {
        let mut prompt = String::new();
        if let (true, content) = &self.preamble
            && !content.is_empty()
        {
            prompt.push_str(content);
            prompt.push('\n');
        }
        if let (true, content) = &self.rules
            && !content.is_empty()
        {
            prompt.push_str(content);
            prompt.push('\n');
        }
        if let (true, tools) = &self.tools
            && !tools.is_empty()
        {
            prompt.push_str(tools);
            prompt.push('\n');
        }
        if let (true, workspace) = &self.workspace
            && workspace.is_dir()
        {
            let path = crate::tools::convert_path_to_unix_style(workspace);
            prompt.push_str(&format!("Current Workspace: {}\n", path));
        }
        if let (true, agents_md) = &self.agents_md
            && !agents_md.is_empty()
        {
            prompt.push_str(agents_md);
            prompt.push('\n');
        }
        if let (true, files) = &self.files
            && !files.is_empty()
        {
            prompt.push_str("<workspace-tree>\nWorking directory layout (sorted by mtime, recent first; depth ≤ 3):\n");
            prompt.push_str(files);
            prompt.push_str("\n</workspace-tree>\n");
            prompt.push_str("Use relative paths for files inside the workspace.\n");
        }
        if let (true, date) = &self.date
            && !date.is_empty()
        {
            prompt.push_str(&format!("Current Date: {}\n", date));
        }
        prompt
    }
}

/// Generate an XML-formatted summary of enabled tools.
pub fn tools_summary(all_tools: &[crate::tools::ToolRef]) -> String {
    let mut result = String::new();
    result.push_str("<available-tools>\n");

    for tool in all_tools {
        let inst = tool.instruction();
        if inst.is_empty() {
            continue;
        }
        result.push_str(&format!("<tool name=\"{}\">{}</tool>\n", tool.name(), inst));
    }

    result.push_str("</available-tools>\n");
    result.push_str("Tools can be enabled or disabled at any time. A tool used earlier in the conversation may no longer be available. Before using a tool, verify that it is currently available. You may also have access to additional tools not listed here.\n");
    result
}
