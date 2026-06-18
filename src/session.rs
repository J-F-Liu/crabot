#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Reasoning / chain-of-thought content (thinking mode).
    pub reasoning: Option<String>,
    pub timestamp: String,
}
