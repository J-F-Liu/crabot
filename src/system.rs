use std::path::PathBuf;

pub const PREAMBLE: &str = "Preamble";
pub const RULES: &str = "Rules";
pub const TOOLS: &str = "Tools";
pub const WORKSPACE: &str = "Workspace";
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
    pub files: (bool, String),
    pub date: (bool, String),
}

impl SystemPrompt {
    pub fn get_mut(&mut self, name: &str) -> Option<&mut (bool, String)> {
        match name {
            PREAMBLE => Some(&mut self.preamble),
            RULES => Some(&mut self.rules),
            TOOLS => Some(&mut self.tools),
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

pub fn build_preamble_options() -> Vec<FilepathEntry> {
    let dir = home::home_dir()
        .unwrap_or_default()
        .join(".crabot")
        .join("preamble");
    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                let display = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                entries.push(FilepathEntry { display, path });
            }
        }
    }
    entries
}

pub fn build_workspace_options(recent: &[PathBuf]) -> Vec<FilepathEntry> {
    use std::collections::HashMap;

    let mut entries: Vec<FilepathEntry> = recent
        .iter()
        .map(|path| {
            let display = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            FilepathEntry {
                display,
                path: path.clone(),
            }
        })
        .collect();

    // Disambiguate duplicate folder names by prepending parent
    let mut counts: HashMap<String, usize> = HashMap::new();
    for e in &entries {
        *counts.entry(e.display.clone()).or_default() += 1;
    }
    for e in &mut entries {
        if counts[&e.display] > 1
            && let Some(parent) = e.path.parent()
            && let Some(parent_name) = parent.file_name().and_then(|n| n.to_str())
        {
            e.display = format!("{}/{}", parent_name, e.display);
        }
    }

    entries.push(FilepathEntry {
        display: "📁 Select new...".to_string(),
        path: PathBuf::new(),
    });

    entries
}
