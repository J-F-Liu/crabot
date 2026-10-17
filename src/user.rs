use arrayvec::ArrayString;
use genai::chat::ContentPart;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

use crate::i18n::Lang;

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

    /// Localized display name; falls back to the English name when untranslated.
    pub fn label(&self, lang: Lang) -> &str {
        lang.tr(self.name.as_str())
    }

    /// The default mode (the one named `"Code"`, or the first mode).
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

impl From<&str> for WorkMode {
    /// Look up a work mode by name (case-insensitive).
    fn from(name: &str) -> Self {
        WorkMode::all()
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .copied()
            .unwrap_or_else(WorkMode::default_mode)
    }
}

/// How a user-selected attachment reaches the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// File content is inserted as text in the request.
    Text,
    /// File bytes are sent as an image content part.
    Image,
}

impl AttachmentKind {
    /// Marker label stored in history.
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
        }
    }

    /// Parse the marker label written by [`marker`](Self::marker).
    pub fn from_marker(label: &str) -> Option<Self> {
        match label {
            "text" => Some(Self::Text),
            "image" => Some(Self::Image),
            _ => None,
        }
    }

    /// Compact icon used by the attachment row and message block.
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Text => "📄",
            Self::Image => "🖼️",
        }
    }
}

/// A file attached by the user to the next prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAttachment {
    pub kind: AttachmentKind,
    /// Absolute path as returned by the file picker; history stores only this.
    pub path: String,
}

impl UserAttachment {
    pub fn new(kind: AttachmentKind, path: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }

    /// File name for display, falling back to the full path.
    pub fn display_name(&self) -> &str {
        Path::new(&self.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone)]
pub struct UserPrompt {
    pub mode: Option<WorkMode>,
    pub workspace_tree: Option<String>,
    pub content: String,
    /// Files whose contents ride along with this prompt.
    pub attachments: Vec<UserAttachment>,
}

impl UserPrompt {
    pub fn new(mode: Option<WorkMode>, content: String, workspace_tree: Option<String>) -> Self {
        Self {
            mode,
            content,
            workspace_tree,
            attachments: Vec::new(),
        }
    }

    /// Attach files to this prompt.
    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<UserAttachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Build multi-part content parts for sending to the LLM. Attachments are
    /// materialized from their history markers by the request builder.
    pub fn to_content_parts(&self) -> Vec<ContentPart> {
        let mut parts: Vec<ContentPart> = Vec::with_capacity(3);

        if let Some(mode) = self.mode {
            parts.push(ContentPart::Text(format!(
                "work-mode: {}",
                mode.name.to_ascii_lowercase()
            )));
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
