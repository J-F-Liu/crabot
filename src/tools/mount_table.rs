//! Shared VFS-to-host mount lookup for the `bash` tool.
//!
//! The host-command bridge and the path policy must agree on which host
//! directory a VFS spelling reaches, so both resolve through this table.

use std::path::{Component, Path, PathBuf};

/// A VFS mount point and the host directory behind it.
pub(crate) trait MountPoint {
    fn vfs_root(&self) -> &Path;
    fn host_root(&self) -> &Path;
}

/// Host path reached by `vfs` through the deepest matching mount.
///
/// `vfs` must be POSIX-absolute. Matching is component-wise; `fold_case`
/// compares VFS component names ASCII-case-insensitively (Windows hosts).
pub(crate) fn host_path_of<M: MountPoint>(
    vfs: &Path,
    mounts: &[M],
    fold_case: bool,
) -> Option<PathBuf> {
    let mut components = vfs.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let rest: Vec<Component<'_>> = components.collect();

    let mut best: Option<(&M, usize)> = None;
    for mount in mounts {
        let Some(depth) = matches_prefix(&rest, mount.vfs_root(), fold_case) else {
            continue;
        };
        if best.is_none_or(|(_, best_depth)| depth > best_depth) {
            best = Some((mount, depth));
        }
    }

    let (mount, depth) = best?;
    let mut host = mount.host_root().to_path_buf();
    for component in &rest[depth..] {
        host.push(component.as_os_str());
    }
    Some(host)
}

/// Number of leading `rest` components covered by `root`; `None` on a mismatch.
fn matches_prefix(rest: &[Component<'_>], root: &Path, fold_case: bool) -> Option<usize> {
    let mut depth = 0;
    for expected in root
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
    {
        let actual = rest.get(depth)?;
        if !component_eq(actual, &expected, fold_case) {
            return None;
        }
        depth += 1;
    }
    Some(depth)
}

fn component_eq(a: &Component<'_>, b: &Component<'_>, fold_case: bool) -> bool {
    match (a, b) {
        (Component::Normal(a), Component::Normal(b)) if fold_case => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
        (a, b) => a == b,
    }
}
