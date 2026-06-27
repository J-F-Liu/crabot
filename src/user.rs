use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkMode {
    Plan,
    Code,
    Review,
}

impl fmt::Display for WorkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkMode::Plan => write!(f, "plan"),
            WorkMode::Code => write!(f, "code"),
            WorkMode::Review => write!(f, "review"),
        }
    }
}

pub struct UserPrompt {
    pub mode: WorkMode,
    pub content: String,
}

impl UserPrompt {
    pub fn new(mode: WorkMode, content: String) -> Self {
        Self { mode, content }
    }

    pub fn get_prompt(&self) -> String {
        let mut prompt = String::new();
        prompt.push_str(&format!("<work-mode>{}</work-mode>\n", self.mode));
        prompt.push_str(&format!("{}\n", &self.content));
        prompt
    }
}
