use arrayvec::ArrayString;
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
            // Match: ## {Name} Mode (`<work-mode>{tag}</work-mode>`)
            let re = Regex::new(r"## (\w+) Mode \(`<work-mode>\w+</work-mode>`\)").unwrap();
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
    pub content: String,
}

impl UserPrompt {
    pub fn new(mode: Option<WorkMode>, content: String) -> Self {
        Self { mode, content }
    }

    pub fn get_prompt(&self) -> String {
        let mut prompt = String::new();
        if let Some(mode) = self.mode {
            let mut lower = mode.name;
            lower.make_ascii_lowercase();
            prompt.push_str(&format!("<work-mode>{}</work-mode>\n", lower));
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
