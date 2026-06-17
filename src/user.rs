use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkMode {
    Plan,
    Code,
    Review,
}

impl fmt::Display for WorkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkMode::Plan => write!(f, "Plan"),
            WorkMode::Code => write!(f, "Code"),
            WorkMode::Review => write!(f, "Review"),
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
        if self.mode != WorkMode::Code {
            prompt.push_str(&format!("You are in {} mode.\n", self.mode));
        }
        prompt.push_str(&self.content);
        prompt
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Reasoning / chain-of-thought content (thinking mode).
    pub reasoning: Option<String>,
    pub timestamp: String,
}
