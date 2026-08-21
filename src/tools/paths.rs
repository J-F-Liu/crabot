//! Text and path helpers shared by the built-in tools.

use serde_json::Value;

/// Convert Windows-style `\r\n` line endings to Unix `\n`.
pub fn normalize_newlines(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\r') {
        return std::borrow::Cow::Borrowed(s);
    }
    std::borrow::Cow::Owned(s.replace("\r\n", "\n"))
}

pub(crate) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// Like `arg_str` but accepts common aliases for a path parameter.
pub fn arg_path(args: &Value) -> Option<&str> {
    const KEYS: &[&str] = &[
        "path",
        "file",
        "filename",
        "file_path",
        "filepath",
        "filePath",
    ];
    KEYS.iter().find_map(|k| arg_str(args, k))
}

pub(crate) fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

/// Strip the workspace prefix and convert to Unix‑style display path.
pub(crate) fn make_workspace_relative(
    path: &std::path::Path,
    workspace: &std::path::Path,
) -> String {
    let rel = path.strip_prefix(workspace).unwrap_or(path);
    convert_path_to_unix_style(rel)
}

/// Convert a path to Unix‑style representation (reverse of `resolve_path`).
///
/// On Windows this turns `C:\Users\...` into `/c/Users/...`.
/// On Unix this is a no‑op (just ensures forward slashes).
pub fn convert_path_to_unix_style(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();

    #[cfg(windows)]
    {
        // If it already looks like a Unix‑style path, just normalise slashes.
        if s.starts_with('/') {
            return s.replace('\\', "/");
        }
        // Match a Windows absolute path like C:\...  or C:/...
        let mut comps = path.components();
        if let Some(std::path::Component::Prefix(p)) = comps.next()
            && let std::path::Prefix::Disk(d) | std::path::Prefix::VerbatimDisk(d) = p.kind()
        {
            let drive_letter = (d as char).to_ascii_lowercase();
            let rest: String = comps
                .filter(|c| {
                    !matches!(
                        c,
                        std::path::Component::RootDir | std::path::Component::CurDir
                    )
                })
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            return format!("/{drive_letter}/{rest}");
        }
    }

    // On non-Windows (or non‑absolute Windows), just normalise backslashes.
    s.replace('\\', "/")
}

/// Build the (non‑canonicalized) target path for `path` relative to `workspace`.
///
/// Handles native absolute paths, Windows Unix‑style paths such as
/// `/c/Users/...`, and workspace‑relative paths.
fn candidate_path(path: &str, workspace: &std::path::Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }

    // On Windows a path like "/c/Users/..." is Unix‑style absolute, but
    // `Path::is_absolute()` returns false without a drive prefix.
    #[cfg(windows)]
    if let Some(native) = convert_path_to_windows_style(path) {
        return native;
    }

    workspace.join(p)
}

/// On Windows, convert a Unix‑style path like `/c/Users/...` into a native
/// `C:\Users\...` `PathBuf`. Returns `None` when `path` is not Unix‑style
/// absolute (i.e. does not start with `/`).
#[cfg(windows)]
pub(crate) fn convert_path_to_windows_style(path: &str) -> Option<std::path::PathBuf> {
    let stripped = path.strip_prefix('/')?;
    let native = drive_style_to_windows(stripped).unwrap_or_else(|| path.replace('/', "\\"));
    Some(std::path::PathBuf::from(native))
}

/// Convert the drive-letter form `d/rest` (of a stripped `/d/rest` path) to
/// `D:\rest`; `None` when `d` is not a single ASCII letter.
#[cfg(windows)]
pub(crate) fn drive_style_to_windows(stripped: &str) -> Option<String> {
    let (drive, rest) = stripped.split_once('/')?;
    (drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic()).then(|| {
        format!(
            "{}:\\{}",
            drive.to_ascii_uppercase(),
            rest.replace('/', "\\")
        )
    })
}

pub fn resolve_path(
    path: &str,
    workspace: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    dunce::canonicalize(candidate_path(path, workspace))
}

/// Like [`resolve_path`] but does not require the final path to exist.
///
/// Canonicalizes the nearest existing ancestor, then appends the remaining
/// (possibly non‑existent) tail components.
pub fn resolve_path_partial(
    path: &str,
    workspace: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    let candidate = candidate_path(path, workspace);

    // Walk up from the candidate until we find an existing ancestor, then
    // re‑attach the missing tail components. The first iteration covers the
    // common case where the full path already exists.
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = candidate.as_path();
    loop {
        if let Ok(canon) = dunce::canonicalize(current) {
            let mut result = canon;
            for seg in missing.iter().rev() {
                result.push(seg);
            }
            return Ok(result);
        }
        match current.parent() {
            Some(parent) => {
                if let Some(name) = current.file_name() {
                    missing.push(name);
                }
                current = parent;
            }
            // Reached the root without finding an existing ancestor — fall
            // back to the un‑canonicalized candidate.
            None => return Ok(candidate),
        }
    }
}
