use arrayvec::ArrayString;
use genai::chat::ContentPart;
use regex::Regex;
use std::sync::LazyLock;

/// A work mode parsed from `assets/workmode.md` at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkMode {
    pub name: ArrayString<16>,
}

impl WorkMode {
    /// All work modes extracted from the embedded `workmode.md`.
    pub fn all() -> &'static [WorkMode] {
        static MODES: LazyLock<Vec<WorkMode>> = LazyLock::new(|| {
            let content = crate::setup::ASSETS
                .get_file("workmode.md")
                .and_then(|f| f.contents_utf8())
                .unwrap_or("");
            // Match: ## {Name} Mode (`work-mode: {tag}`)
            let re = Regex::new(r"## (\w+) Mode \(`work-mode: \w+`\)").unwrap();
            re.captures_iter(content)
                .filter_map(|cap| {
                    ArrayString::from(&cap[1])
                        .ok()
                        .map(|name| WorkMode { name })
                })
                .collect()
        });
        &MODES
    }

    /// The default mode (the one whose lowercase name is `"code"`, or the first mode).
    pub fn default_mode() -> WorkMode {
        WorkMode::all()
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case("code"))
            .or_else(|| WorkMode::all().first())
            .copied()
            .unwrap_or(WorkMode {
                name: ArrayString::from("Code").unwrap(),
            })
    }
}

pub struct UserPrompt {
    pub mode: Option<WorkMode>,
    pub workspace_tree: Option<String>,
    pub content: String,
}

impl UserPrompt {
    pub fn new(mode: Option<WorkMode>, content: String, workspace_tree: Option<String>) -> Self {
        Self {
            mode,
            content,
            workspace_tree,
        }
    }

    /// Build multi-part content parts for sending to the LLM.
    pub fn to_content_parts(&self) -> Vec<ContentPart> {
        let mut parts: Vec<ContentPart> = Vec::with_capacity(3);

        if let Some(mode) = self.mode {
            let mut lower = mode.name;
            lower.make_ascii_lowercase();
            parts.push(ContentPart::Text(format!("work-mode: {}", lower)));
        }

        if let Some(tree) = &self.workspace_tree
            && !tree.is_empty()
        {
            parts.push(ContentPart::Text(format!(
                "Working directory layout (sorted by mtime, recent first; depth ≤ 3):\n{}",
                tree
            )));
        }

        parts.push(ContentPart::Text(self.content.clone()));
        parts
    }
}
