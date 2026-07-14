use regex::Regex;
use std::sync::LazyLock;

/// A work mode parsed from `assets/workmode.md` at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkMode {
    pub name: String,
}

impl WorkMode {
    /// All work modes extracted from the embedded `workmode.md`.
    pub fn all() -> &'static [WorkMode] {
        static MODES: LazyLock<Vec<WorkMode>> = LazyLock::new(|| {
            let content = crate::setup::ASSETS
                .get_file("workmode.md")
                .and_then(|f| f.contents_utf8())
                .unwrap_or("");
            // Match: ## {Name} Mode (`<work-mode>{tag}</work-mode>`)
            let re = Regex::new(r"## (\w+) Mode \(`<work-mode>\w+</work-mode>`\)").unwrap();
            re.captures_iter(content)
                .map(|cap| WorkMode {
                    name: cap[1].to_string(),
                })
                .collect()
        });
        &MODES
    }

    /// The default mode (the one whose lowercase name is `"code"`, or the first mode).
    pub fn default_mode() -> &'static WorkMode {
        static FALLBACK: LazyLock<WorkMode> = LazyLock::new(|| WorkMode {
            name: "Code".into(),
        });
        WorkMode::all()
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case("code"))
            .or_else(|| WorkMode::all().first())
            .unwrap_or(&FALLBACK)
    }
}

pub struct UserPrompt {
    pub mode: Option<WorkMode>,
    pub content: String,
}

impl UserPrompt {
    pub fn new(mode: Option<WorkMode>, content: String) -> Self {
        Self { mode, content }
    }

    pub fn get_prompt(&self) -> String {
        let mut prompt = String::new();
        if let Some(ref mode) = self.mode {
            prompt.push_str(&format!(
                "<work-mode>{}</work-mode>\n",
                mode.name.to_lowercase()
            ));
        }
        prompt.push_str(&format!("{}\n", &self.content));
        prompt
    }

    /// Strip the leading `<work-mode>…</work-mode>\n` tag
    pub fn strip_mode_tag(prompt: &str) -> &str {
        prompt
            .find("</work-mode>\n")
            .map(|idx| &prompt[idx + "</work-mode>\n".len()..])
            .unwrap_or(prompt)
    }
}
