//! Path policy for the `bash` tool's in-process VFS.
//!
//! `.git` and `.agent/sessions` live on the host, so a writable workspace mount
//! lets builtins and redirects destroy them; readonly sub-mounts do not help
//! because `rm -rf .agent` reaches them through the writable parent mount.
//!
//! Paths are mapped through the shared mount table, resolved on the host
//! (symlinked parents included), and compared as host paths so every VFS
//! spelling matches. Protected directories, their descendants, and their
//! ancestors are refused. Bridged host commands, the real `bash -c` fallback,
//! and the file tools bypass this VFS by design: this is a damage guard, not a
//! sandbox.

use std::io::{Error as IoError, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bashkit::{
    DirEntry, DirListing, FileSystem, FileSystemExt, FsLimits, FsUsage, Metadata, Result,
    SearchCapable, VfsSnapshot, WalkOptions, async_trait, normalize_path,
};

use super::mount_table::{self, MountPoint};

/// A VFS mount point and the real host directory it maps to.
pub(crate) struct MountRule {
    pub(crate) vfs: PathBuf,
    pub(crate) host: PathBuf,
}

impl MountPoint for MountRule {
    fn vfs_root(&self) -> &Path {
        &self.vfs
    }

    fn host_root(&self) -> &Path {
        &self.host
    }
}

/// Normalize a host path for comparison: forward slashes, Windows-insensitive case.
fn host_key(path: impl AsRef<Path>) -> String {
    let text = path.as_ref().to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text
    }
}

/// Rejects mutations of workspace metadata from shell builtins and redirects.
pub(crate) struct ProtectedFs {
    inner: Arc<dyn FileSystem>,
    mounts: Arc<[MountRule]>,
    /// Host paths of protected directories, `host_key` form.
    protected: Arc<[String]>,
}

/// Least to most restrictive, so `max` picks the stricter zone.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Zone {
    /// Neither protected nor an ancestor.
    Other,
    /// Creating, deleting, or moving the path would take protected dirs with it.
    Ancestor,
    /// The path is protected or below it.
    Inside,
}

impl ProtectedFs {
    /// `workspace` must be the canonical host path of the workspace mount —
    /// the same value as its `MountRule::host` — so the protected keys always
    /// compare equal to resolved host paths. Re-canonicalizing here (with a
    /// fallback) could diverge from the mount rules on Windows verbatim paths
    /// and silently disable the policy.
    pub(crate) fn new(
        inner: Arc<dyn FileSystem>,
        mounts: Vec<MountRule>,
        workspace: &Path,
    ) -> Self {
        let protected = [".git", ".agent/sessions"]
            .into_iter()
            .map(|relative| host_key(workspace.join(relative)))
            .collect();
        Self {
            inner,
            mounts: Arc::from(mounts),
            protected,
        }
    }

    /// Deepest VFS mount match, mapped back to a host path.
    fn host_path(&self, path: &Path) -> Option<PathBuf> {
        let normalized = normalize_path(path);
        // VFS names are case-sensitive; only a Windows host is not.
        mount_table::host_path_of(&normalized, &self.mounts, cfg!(windows))
    }

    /// Resolve host links; a leaf link is followed only when `follow_last`.
    async fn resolve_host(host: &Path, follow_last: bool) -> PathBuf {
        match host.file_name() {
            Some(name) if !follow_last => {
                let parent = host.parent().unwrap_or(host);
                Self::canonical_or_nearest(parent).await.join(name)
            }
            _ => Self::canonical_or_nearest(host).await,
        }
    }

    /// Canonicalize the deepest existing ancestor, keeping the missing suffix.
    async fn canonical_or_nearest(path: &Path) -> PathBuf {
        for ancestor in path.ancestors() {
            if let Ok(canonical) = tokio::fs::canonicalize(ancestor).await {
                let mut resolved = canonical;
                if let Ok(rest) = path.strip_prefix(ancestor) {
                    resolved.push(rest);
                }
                return resolved;
            }
        }
        path.to_path_buf()
    }

    /// Zone of one host spelling.
    fn zone_of_host(&self, host: &Path) -> Zone {
        let host = host_key(host);
        let host = host.trim_end_matches('/');
        if self
            .protected
            .iter()
            .any(|p| host == p || has_prefix(host, p))
        {
            return Zone::Inside;
        }
        if self.protected.iter().any(|p| has_prefix(p, host)) {
            return Zone::Ancestor;
        }
        Zone::Other
    }

    /// Zone of a VFS path, before and after resolving host links.
    async fn zone(&self, path: &Path, follow_last: bool) -> Zone {
        let Some(host) = self.host_path(path) else {
            return Zone::Other;
        };
        let syntactic = self.zone_of_host(&host);
        if syntactic == Zone::Inside {
            return syntactic;
        }
        let resolved = Self::resolve_host(&host, follow_last).await;
        syntactic.max(self.zone_of_host(&resolved))
    }

    fn deny(op: &str, path: &Path) -> Result<()> {
        Err(IoError::new(
            ErrorKind::PermissionDenied,
            format!(
                "crabot: {op} is read-only for protected workspace metadata: {}",
                path.display()
            ),
        )
        .into())
    }

    /// Entry creation/replacement also rejects ancestors.
    async fn allow_entry_write(&self, op: &str, path: &Path) -> Result<()> {
        if self.zone(path, true).await == Zone::Other {
            return Ok(());
        }
        Self::deny(op, path)
    }

    /// Metadata changes reject the protected paths themselves.
    async fn allow_metadata_write(&self, op: &str, path: &Path) -> Result<()> {
        if self.zone(path, true).await == Zone::Inside {
            return Self::deny(op, path);
        }
        Ok(())
    }
}

/// Whether `path` lies strictly below `prefix`.
fn has_prefix(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('/'))
}

#[async_trait]
impl FileSystemExt for ProtectedFs {
    fn usage(&self) -> FsUsage {
        self.inner.usage()
    }

    async fn mkfifo(&self, path: &Path, mode: u32) -> Result<()> {
        self.allow_entry_write("mkfifo", path).await?;
        self.inner.mkfifo(path, mode).await
    }

    fn limits(&self) -> FsLimits {
        self.inner.limits()
    }

    fn vfs_snapshot(&self) -> Option<VfsSnapshot> {
        self.inner.vfs_snapshot()
    }

    fn vfs_restore(&self, snapshot: &VfsSnapshot) -> Result<()> {
        self.inner.vfs_restore(snapshot)
    }

    fn backend_kind(&self) -> &'static str {
        self.inner.backend_kind()
    }
}

#[async_trait]
impl FileSystem for ProtectedFs {
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        self.inner.read_file(path).await
    }

    async fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.allow_entry_write("write", path).await?;
        self.inner.write_file(path, content).await
    }

    async fn append_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.allow_entry_write("append", path).await?;
        self.inner.append_file(path, content).await
    }

    async fn mkdir(&self, path: &Path, recursive: bool) -> Result<()> {
        self.allow_entry_write("mkdir", path).await?;
        self.inner.mkdir(path, recursive).await
    }

    async fn remove(&self, path: &Path, recursive: bool) -> Result<()> {
        match self.zone(path, false).await {
            Zone::Inside => return Self::deny("remove", path),
            // `rm -rf .agent` / `rm -rf .` would delete protected descendants.
            Zone::Ancestor if recursive => return Self::deny("recursive remove", path),
            _ => self.inner.remove(path, recursive).await,
        }
    }

    async fn stat(&self, path: &Path) -> Result<Metadata> {
        self.inner.stat(path).await
    }

    async fn lstat(&self, path: &Path) -> Result<Metadata> {
        self.inner.lstat(path).await
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        self.inner.read_dir(path).await
    }

    async fn exists(&self, path: &Path) -> Result<bool> {
        self.inner.exists(path).await
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        if self.zone(from, false).await != Zone::Other {
            return Self::deny("rename", from);
        }
        if self.zone(to, true).await != Zone::Other {
            return Self::deny("rename", to);
        }
        self.inner.rename(from, to).await
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        if self.zone(to, true).await != Zone::Other {
            return Self::deny("copy", to);
        }
        self.inner.copy(from, to).await
    }

    async fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
        self.allow_entry_write("symlink", link).await?;
        self.inner.symlink(target, link).await
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        self.inner.read_link(path).await
    }

    async fn chmod(&self, path: &Path, mode: u32) -> Result<()> {
        self.allow_metadata_write("chmod", path).await?;
        self.inner.chmod(path, mode).await
    }

    async fn set_modified_time(&self, path: &Path, time: bashkit::time::SystemTime) -> Result<()> {
        self.allow_metadata_write("touch", path).await?;
        self.inner.set_modified_time(path, time).await
    }

    fn as_search_capable(&self) -> Option<&dyn SearchCapable> {
        self.inner.as_search_capable()
    }

    async fn walk(&self, root: &Path, opts: WalkOptions) -> Result<Vec<DirListing>> {
        self.inner.walk(root, opts).await
    }
}
