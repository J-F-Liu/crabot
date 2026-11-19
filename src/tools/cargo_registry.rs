//! Cargo registry root checks shared by the search tools.

use std::path::{Path, PathBuf};

use super::convert_path_to_unix_style;

/// Agent-facing refusal for searching a whole cargo registry.
pub(crate) const WHOLE_CARGO_REGISTRY_MESSAGE: &str = "Never search whole cargo registry";

/// Cargo's home directory (`CARGO_HOME` or `~/.cargo`), resolved against `workspace`.
pub(crate) fn cargo_home(workspace: &Path) -> PathBuf {
    let home = std::env::var_os("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home::home_dir().map(|home| home.join(".cargo")))
        .unwrap_or_else(|| PathBuf::from(".cargo"));
    if home.is_absolute() {
        home
    } else {
        workspace.join(home)
    }
}

/// True for a whole registry root, not a crate directory:
/// `registry`, `registry/src`, or `registry/src/<registry-id>`.
pub(crate) fn is_whole_cargo_registry(path: &Path, cargo_home: &Path) -> bool {
    let registry = cargo_home.join("registry");
    let src = registry.join("src");
    same_path(path, &registry)
        || same_path(path, &src)
        || path.parent().is_some_and(|parent| same_path(parent, &src))
        || standard_registry_path(path)
}

/// Compare canonical paths, ignoring Windows case differences.
fn same_path(left: &Path, right: &Path) -> bool {
    let canonical = |path: &Path| dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let left = convert_path_to_unix_style(&canonical(left));
    let right = convert_path_to_unix_style(&canonical(right));
    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

/// Recognize the standard `.cargo/registry/...` layout, including MSYS paths.
fn standard_registry_path(path: &Path) -> bool {
    let path = convert_path_to_unix_style(path);
    let path = path.trim_end_matches('/');
    let path = if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_string()
    };
    let Some((_, rest)) = path.split_once("/.cargo/registry") else {
        return false;
    };
    rest.is_empty()
        || rest == "/src"
        || rest
            .strip_prefix("/src/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}
