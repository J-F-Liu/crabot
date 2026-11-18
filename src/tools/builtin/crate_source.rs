//! Resolve a crate's source directory from `Cargo.lock` and the cargo cache.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use semver::Version;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::tools::{
    CANCEL_REASON, Tool, convert_path_to_unix_style, detach_child, required_str, sanitize_child_env,
};

/// Find the local source directory of a dependency in the cargo cache.
pub struct CrateSourceTool;

impl Tool for CrateSourceTool {
    fn name(&self) -> &str {
        "crate_source"
    }

    fn description(&self) -> &str {
        "Find the local source directory of a crate locked in the workspace's Cargo.lock. Resolves registry, git, and path dependencies without scanning the cache. Missing crates.io sources include a download URL."
    }

    fn instruction(&self) -> &str {
        "Look up a Rust crate's locked source directory. Before inspecting a dependency's source code, use crate_source to find its local path; run `cargo fetch` if the source is missing. Don't grep Rust source files across the whole .cargo/registry."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "crate": {
                    "type": "string",
                    "description": "Name of the crate to locate (e.g. 'bevy', 'serde', 'nalgebra'); 'name@version' picks an exact or partial locked version. Without a version, the crate must be locked exactly once."
                }
            },
            "required": ["crate"]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let spec = required_str(args, "crate")?.trim();
        if spec.is_empty() {
            return Err("Missing 'crate' argument".into());
        }
        let path = resolve(spec, workspace, &cargo_home(workspace), cancel)?;
        // Keep the Unix-style path the previous custom tool returned.
        Ok(convert_path_to_unix_style(&path))
    }
}

/// A requested crate and an optional exact or partial version.
struct CrateSpec<'a> {
    name: &'a str,
    version: Option<&'a str>,
}

impl<'a> CrateSpec<'a> {
    fn parse(spec: &'a str) -> Self {
        if let Some((name, version)) = spec.split_once('@')
            && !name.is_empty()
        {
            return Self {
                name,
                version: (!version.is_empty()).then_some(version),
            };
        }
        Self {
            name: spec,
            version: None,
        }
    }

    fn accepts(&self, version: &Version) -> bool {
        self.version
            .is_none_or(|wanted| version_matches(version, wanted))
    }
}

fn resolve(
    spec: &str,
    workspace: &Path,
    cargo_home: &Path,
    cancel: &CancellationToken,
) -> Result<PathBuf, String> {
    let requested = CrateSpec::parse(spec);
    check_cancel(cancel)?;

    let lock_path = find_lock(workspace)
        .ok_or_else(|| "crate_source: no Cargo.lock found; run `cargo fetch` first".to_string())?;
    let lock = fs::read_to_string(&lock_path)
        .map_err(|e| format!("crate_source: failed to read {}: {e}", lock_path.display()))?;
    let locked: Vec<LockedPackage> = parse_lock(&lock)
        .into_iter()
        .filter(|package| names_equal(&package.name, requested.name))
        .filter(|package| {
            Version::parse(&package.version).is_ok_and(|version| requested.accepts(&version))
        })
        .collect();
    if locked.is_empty() {
        return Err(format!("crate_source: {spec} is not in Cargo.lock"));
    }
    if locked.len() > 1 {
        let mut versions: Vec<&str> = locked
            .iter()
            .map(|package| package.version.as_str())
            .collect();
        versions.sort_by_key(|version| Version::parse(version).ok());
        return Err(format!(
            "crate_source: {spec} matches multiple locked versions ({}); specify crate@version",
            versions.join(", ")
        ));
    }

    let package = &locked[0];
    let path = locate_locked(package, workspace, cargo_home, cancel);
    check_cancel(cancel)?;
    path.ok_or_else(|| source_unavailable(package))
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err(CANCEL_REASON.into())
    } else {
        Ok(())
    }
}

fn source_unavailable(package: &LockedPackage) -> String {
    let message = format!(
        "crate_source: {} {} is locked but its source directory was not found",
        package.name, package.version
    );
    match package.source.as_deref() {
        Some(source) if is_crates_io(source) => format!(
            "{message}; run `cargo fetch` or download https://crates.io/api/v1/crates/{}/{}/download",
            package.name, package.version
        ),
        Some(_) => format!("{message}; run `cargo fetch` first"),
        None => format!(
            "{message}; it is a path dependency, so check that its `path` in Cargo.toml points to an existing crate"
        ),
    }
}

fn is_crates_io(source: &str) -> bool {
    source.starts_with("registry+https://github.com/rust-lang/crates.io-index")
        || source.starts_with("sparse+https://index.crates.io")
}

fn locate_locked(
    package: &LockedPackage,
    workspace: &Path,
    cargo_home: &Path,
    cancel: &CancellationToken,
) -> Option<PathBuf> {
    match package.source.as_deref() {
        Some(source) if source.starts_with("registry+") || source.starts_with("sparse+") => {
            find_registry_dir(cargo_home, &package.name, &package.version)
        }
        Some(source) if source.starts_with("git+") => find_git_checkout(cargo_home, source)
            .and_then(|root| find_locked_package(&root, &package.name, &package.version, 8)),
        Some(source) if !source.starts_with("path+") => None,
        _ => find_path_dependency(workspace, package, cancel),
    }
}

fn find_lock(workspace: &Path) -> Option<PathBuf> {
    workspace
        .ancestors()
        .map(|dir| dir.join("Cargo.lock"))
        .find(|path| path.is_file())
}

#[derive(Default)]
struct LockedPackage {
    name: String,
    version: String,
    source: Option<String>,
}

fn parse_lock(text: &str) -> Vec<LockedPackage> {
    text.split("[[package]]")
        .skip(1)
        .map(|section| {
            let mut package = LockedPackage::default();
            for line in section.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    break;
                }
                if let Some(value) = lock_value(line, "name") {
                    package.name = value;
                } else if let Some(value) = lock_value(line, "version") {
                    package.version = value;
                } else if let Some(value) = lock_value(line, "source") {
                    package.source = Some(value);
                }
            }
            package
        })
        .filter(|package| !package.name.is_empty() && !package.version.is_empty())
        .collect()
}

fn lock_value(line: &str, key: &str) -> Option<String> {
    let value = line.strip_prefix(key)?.strip_prefix(" = ")?.trim();
    Some(value.strip_prefix('"')?.strip_suffix('"')?.to_string())
}

/// Match an exact version or a partial `1` / `1.2` prefix.
fn version_matches(version: &Version, wanted: &str) -> bool {
    if let Ok(exact) = Version::parse(wanted) {
        return version == &exact;
    }
    let parts: Vec<&str> = wanted.split('.').collect();
    if parts.len() > 3 {
        return false;
    }
    let mut numbers = [0_u64; 3];
    for (index, part) in parts.iter().enumerate() {
        let Ok(number) = part.parse::<u64>() else {
            return false;
        };
        numbers[index] = number;
    }
    version.major == numbers[0]
        && (parts.len() < 2 || version.minor == numbers[1])
        && (parts.len() < 3 || version.patch == numbers[2])
}

fn names_equal(left: &str, right: &str) -> bool {
    left == right || left.replace('-', "_") == right.replace('-', "_")
}

/// Immediate subdirectories of `path`.
fn subdirs(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn registry_src_roots(cargo_home: &Path) -> Vec<PathBuf> {
    subdirs(&cargo_home.join("registry").join("src"))
}

fn find_registry_dir(cargo_home: &Path, name: &str, version: &str) -> Option<PathBuf> {
    registry_src_roots(cargo_home)
        .into_iter()
        .map(|root| root.join(format!("{name}-{version}")))
        .find(|path| path.join("Cargo.toml").is_file())
}

fn git_checkout_dirs(cargo_home: &Path) -> Vec<PathBuf> {
    subdirs(&cargo_home.join("git").join("checkouts"))
        .into_iter()
        .flat_map(|repo| subdirs(&repo))
        .collect()
}

fn find_git_checkout(cargo_home: &Path, source: &str) -> Option<PathBuf> {
    let short = source.rsplit_once('#')?.1.get(..7)?;
    git_checkout_dirs(cargo_home).into_iter().find(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(short))
    })
}

fn find_path_dependency(
    workspace: &Path,
    package: &LockedPackage,
    cancel: &CancellationToken,
) -> Option<PathBuf> {
    // `--no-deps` covers workspace members without resolving the whole graph.
    if let Some(path) = cargo_metadata_dir(workspace, &package.name, &package.version, true) {
        return Some(path);
    }
    if cancel.is_cancelled() {
        return None;
    }
    // A path dependency outside the workspace needs the full graph.
    cargo_metadata_dir(workspace, &package.name, &package.version, false)
}

fn cargo_metadata_dir(
    workspace: &Path,
    name: &str,
    wanted: &str,
    no_deps: bool,
) -> Option<PathBuf> {
    let mut command = Command::new("cargo");
    command
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if no_deps {
        command.arg("--no-deps");
    }
    sanitize_child_env(&mut command);
    detach_child(&mut command);

    let metadata: Value = serde_json::from_slice(&command.output().ok()?.stdout).ok()?;
    let packages = metadata.get("packages")?.as_array()?;
    metadata_package_dir(packages, name, wanted)
        .or_else(|| metadata_path_dependency_dir(packages, name, wanted))
}

/// Workspace members are listed as full `packages` entries.
fn metadata_package_dir(packages: &[Value], name: &str, wanted: &str) -> Option<PathBuf> {
    packages.iter().find_map(|package| {
        let package_name = package.get("name")?.as_str()?;
        let package_version = package.get("version")?.as_str()?;
        if !names_equal(package_name, name) {
            return None;
        }
        if let Ok(version) = Version::parse(package_version)
            && !wanted.is_empty()
            && !version_matches(&version, wanted)
        {
            return None;
        }
        let manifest = package.get("manifest_path")?.as_str()?;
        Path::new(manifest).parent().map(Path::to_path_buf)
    })
}

/// Path dependencies outside the workspace appear only in members' `dependencies` lists.
fn metadata_path_dependency_dir(packages: &[Value], name: &str, wanted: &str) -> Option<PathBuf> {
    packages
        .iter()
        .filter_map(|package| package.get("dependencies")?.as_array())
        .flatten()
        .filter_map(|dependency| dependency.get("path")?.as_str())
        .find_map(|path| {
            let dir = Path::new(path);
            let (found_name, found_version) = manifest_package(&dir.join("Cargo.toml"))?;
            if !names_equal(&found_name, name) {
                return None;
            }
            let version_ok = found_version
                .as_deref()
                .and_then(|found| Version::parse(found).ok())
                .is_none_or(|found| wanted.is_empty() || version_matches(&found, wanted));
            version_ok.then(|| dir.to_path_buf())
        })
}

/// Locate the locked package inside an exact git checkout.
fn find_locked_package(
    root: &Path,
    name: &str,
    version: &str,
    max_depth: usize,
) -> Option<PathBuf> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .max_depth(Some(max_depth))
        .filter_entry(|entry| {
            !entry.file_type().is_some_and(|kind| kind.is_dir())
                || !matches!(
                    entry.file_name().to_str(),
                    Some("target" | ".git" | ".agent" | "node_modules")
                )
        });
    for entry in builder.build().flatten() {
        if entry.file_name() != "Cargo.toml" {
            continue;
        }
        let Some((found_name, found_version)) = manifest_package(entry.path()) else {
            continue;
        };
        if !names_equal(&found_name, name) {
            continue;
        }
        if found_version
            .as_deref()
            .is_some_and(|found| found != version)
        {
            continue;
        }
        return entry.path().parent().map(Path::to_path_buf);
    }
    None
}

/// Read `name` and an optional `version` from a manifest's `[package]` section.
fn manifest_package(path: &Path) -> Option<(String, Option<String>)> {
    let text = fs::read_to_string(path).ok()?;
    let mut name = String::new();
    let mut version = None;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(value) = toml_string(line, "name") {
            name = value;
        } else if let Some(value) = toml_string(line, "version") {
            version = Some(value);
        }
    }
    (!name.is_empty()).then_some((name, version))
}

fn toml_string(line: &str, key: &str) -> Option<String> {
    let value = line
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })?;
    Some(value.to_string())
}

fn cargo_home(workspace: &Path) -> PathBuf {
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
