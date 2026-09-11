//! Crash-safe config file I/O: writes land in a temp file renamed over the
//! target, unparsable files are moved aside, and a lock serializes
//! read-merge-write cycles between instances.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Longest a caller waits for the advisory lock before writing unlocked.
const LOCK_WAIT: Duration = Duration::from_millis(200);

/// Poll interval while waiting for a contended lock.
const LOCK_POLL: Duration = Duration::from_millis(10);

/// Directory holding `path`, never empty (`.` for a bare file name).
fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Sibling of `path` with `suffix` appended to its file name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Sibling backup path (`<name>.bak`) holding the previous file version.
fn backup_path(path: &Path) -> PathBuf {
    sibling(path, ".bak")
}

/// Atomically replace `path` with `contents`, keeping the previous version as
/// `<name>.bak` (best effort).
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = parent_dir(path);
    std::fs::create_dir_all(parent)?;
    if path.is_file() {
        let _ = std::fs::copy(path, backup_path(path));
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(contents)?;
    temp.as_file().sync_all()?;
    // Temp files are created 0600; keep the permissions of the replaced file.
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = temp.as_file().set_permissions(meta.permissions());
    }
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Serialize `value` as RON text and atomically write it to `path`.
pub fn save_ron<T: serde::Serialize>(
    path: &Path,
    value: &T,
    config: ron::ser::PrettyConfig,
) -> std::io::Result<()> {
    let text = ron::ser::to_string_pretty(value, config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_atomic(path, text.as_bytes())
}

/// Load a RON file; a malformed file is moved aside and its last known good
/// `<name>.bak` is used instead, so a save cannot destroy it.
pub fn load_ron<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(path = %path.display(), "failed to read file: {e}");
            return None;
        }
    };
    match ron::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(path = %path.display(), "failed to parse file: {e}");
            quarantine(path);
            let backup = backup_path(path);
            let value = ron::from_str(&std::fs::read_to_string(&backup).ok()?).ok()?;
            tracing::info!(path = %backup.display(), "recovered file from backup");
            Some(value)
        }
    }
}

/// Run `f` while holding an exclusive advisory lock on `<path>.lock`, so
/// concurrent instances serialize their read-merge-write cycles. The wait is
/// bounded: when the lock is held too long, `f` runs unlocked rather than
/// stalling behind another instance's disk I/O.
pub fn with_lock<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    let _ = std::fs::create_dir_all(parent_dir(path));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(sibling(path, ".lock"));
    let file = match file {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!(path = %path.display(), "cannot open lock file: {e}");
            return f();
        }
    };
    // Only the advisory lock matters; the file's contents are unused.
    let mut lock = fd_lock::RwLock::new(file);
    let deadline = Instant::now() + LOCK_WAIT;
    // Named binding: the guard must outlive `f`.
    let _guard = loop {
        match lock.try_write() {
            Ok(guard) => break Some(guard),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    tracing::warn!(path = %path.display(), "lock still held, writing unlocked");
                    break None;
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), "cannot lock file: {e}");
                break None;
            }
        }
    };
    f()
}

/// Move an unparsable file aside so the next save cannot destroy it.
fn quarantine(path: &Path) {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut dest = path.with_file_name(format!("{stem}.invalid-{stamp}.ron"));
    let mut n = 1;
    // Two rescues within the same second must not overwrite each other.
    while dest.exists() {
        dest = path.with_file_name(format!("{stem}.invalid-{stamp}-{n}.ron"));
        n += 1;
    }
    match std::fs::rename(path, &dest) {
        Ok(()) => tracing::error!(file = %dest.display(), "unparsable file moved aside"),
        Err(e) => {
            tracing::error!(path = %path.display(), "failed to move unparsable file aside: {e}")
        }
    }
}
