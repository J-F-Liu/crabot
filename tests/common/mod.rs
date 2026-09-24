//! Shared helpers for the integration tests.
#![allow(dead_code)] // every test binary compiles the full API but uses a slice

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{env, fs, io, process};

/// A per-instance-unique temp dir, recursively removed on drop.
///
/// The unique suffix keeps parallel tests that share a prefix from clearing each
/// other's tree; `path` is public so call sites stay terse.
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    /// Create `<temp>/crabot_test_<prefix>_<pid>_<n>`.
    pub fn new(prefix: &str) -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let mut path = env::temp_dir();
        path.push(format!(
            "crabot_test_{}_{}_{}",
            prefix,
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path); // clean any left-over
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    /// Join `name` onto the directory (no filesystem access).
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Create `name` with `body`, making parent dirs as needed; returns its path.
    pub fn write(&self, name: &str, body: &[u8]) -> io::Result<PathBuf> {
        let p = self.join(name);
        fs::create_dir_all(p.parent().unwrap())?;
        fs::write(&p, body)?;
        Ok(p)
    }

    /// Create an empty file `name`.
    pub fn mkfile(&self, name: &str) -> io::Result<PathBuf> {
        self.write(name, b"")
    }

    /// Create directory `name` (and any missing parents); returns its path.
    pub fn mkdir(&self, name: &str) -> io::Result<PathBuf> {
        let p = self.join(name);
        fs::create_dir_all(&p)?;
        Ok(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
