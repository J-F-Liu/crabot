//! Chinese (Simplified) translation tables, split per UI area.
//!
//! Each area table is a flat `&[(&str, &str)]` slice mapping an English UI
//! string (the key) to its Chinese translation. `lookup` merges all areas.
//!
//! Conventions for contributors:
//! - Keys are the exact English string used in code, including punctuation
//!   and leading/trailing spaces.
//! - Format templates keep their `{}` placeholders in the translation.
//! - The same English key may appear more than once only with the *same*
//!   translation; the first occurrence wins and later duplicates are ignored,
//!   so conflicting repeats would silently keep the earlier value.
//! - Keep key strings in plain ASCII English; translations are UTF-8.

pub mod center;
pub mod left;
pub mod main;
pub mod right;
pub mod settings;

use std::collections::HashMap;
use std::sync::OnceLock;

/// All area tables, in lookup-priority order (earlier wins).
const TABLES: &[&[(&str, &str)]] = &[
    main::TABLE,
    left::TABLE,
    center::TABLE,
    right::TABLE,
    settings::TABLE,
];

/// Translate `key` to Chinese, falling back to the key itself when unknown.
pub fn lookup(key: &str) -> &str {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for table in TABLES {
            for &(en, zh) in *table {
                // First area wins; later duplicates are ignored.
                m.entry(en).or_insert(zh);
            }
        }
        m
    });
    map.get(key).copied().unwrap_or(key)
}
