//! Minimal UI internationalization.
//!
//! Design: languages are threaded **explicitly** through the view layer (no
//! global mutable locale). English source strings are the translation keys,
//! so untranslated code stays readable and new strings work in English
//! immediately; the Chinese table only adds entries for keys that need it.
//!
//! Usage from a view function that has access to a `Lang`:
//!
//! ```ignore
//! text(lang.tr("System Prompt"))
//! text(&format!(lang.tr("{} tokens"), n))
//! ```
//!
//! Translation tables live in [`zh`] and are split per UI area so that
//! per-file translation work never touches the same table file.

use serde::{Deserialize, Serialize};

pub mod zh;

/// Supported UI languages.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    /// English. Source strings are the keys, so this is the default.
    #[default]
    En,
    /// Simplified Chinese.
    Zh,
}

impl Lang {
    /// All supported languages, in display order.
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Zh];

    /// Native name of the language, for language pickers.
    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Zh => "中文",
        }
    }

    /// The other supported language (for quick-toggle buttons).
    pub fn toggle(self) -> Lang {
        match self {
            Lang::En => Lang::Zh,
            Lang::Zh => Lang::En,
        }
    }

    /// Translate an English UI string into the active language.
    ///
    /// `En` returns the key unchanged; unknown keys fall back to English in
    /// every language, so a missing translation degrades gracefully.
    pub fn tr(self, key: &str) -> &str {
        match self {
            Lang::En => key,
            Lang::Zh => zh::lookup(key),
        }
    }
}

/// Native name (see [`Lang::label`]), so `Lang` works directly as a picker option.
impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
