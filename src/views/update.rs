//! Version-check banner shown at the top of the window when a newer Crabot
//! release is available on GitHub.

use std::path::PathBuf;
use std::time::Duration;

use iced::{
    Alignment, Border, Color, Element, Length, Theme,
    widget::{Space, button, container, row, text},
};
use semver::Version;

use crate::OverlayEvent;

/// Version of the running binary.
pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// GitHub releases page opened from the banner.
pub(crate) const RELEASES_URL: &str = "https://github.com/J-F-Liu/crabot/releases";

/// Download state machine for the "Install New Version" button.
#[derive(Clone)]
pub(crate) enum UpdateDownloadState {
    /// Not started.
    Idle,
    /// Download / extraction in progress.
    InProgress,
    /// Ready — button changes to "Restart to Update".
    ReadyToRestart(PathBuf),
    /// Failed — check stderr for details.
    Failed,
}

/// Query GitHub for the latest stable release of Crabot.
/// Returns `Some(version)` if a newer version exists, `None` otherwise.
pub(crate) async fn check_for_updates() -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent(crabot::app_title())
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    // `/releases/latest` redirects (302) to `/releases/tag/v{version}`.
    let response = client
        .get(format!("{RELEASES_URL}/latest"))
        .send()
        .await
        .ok()?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    // GitHub tags are prefixed with "v" (e.g. "v0.4.2"); strip it before parsing.
    let tag = location.rsplit('/').next()?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let latest = Version::parse(version).ok()?;
    let current = Version::parse(CURRENT_VERSION).ok()?;
    (latest > current).then(|| latest.to_string())
}

/// Compare two semver strings (e.g. "0.4.2" > "0.4.1").
/// Returns true if `a > b`.
pub(crate) fn version_gt(a: &str, b: &str) -> bool {
    match (Version::parse(a), Version::parse(b)) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

// ── Self-update helpers ──────────────────────────────────────────

/// Asset file name for the current platform, e.g. "crabot-windows-x86_64.zip".
fn platform_asset_name() -> Result<String, String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    // GitHub release asset naming: crabot-{os}-{arch}.{ext}
    match (os, arch) {
        ("windows", "x86_64")
        | ("linux", "x86_64")
        | ("macos", "x86_64")
        | ("macos", "aarch64") => Ok(format!("crabot-{os}-{arch}.{ext}")),
        _ => Err(format!("Unsupported platform: {os}-{arch}")),
    }
}

/// Full download URL for a release asset.
fn asset_download_url(version: &str, asset_name: &str) -> String {
    format!("{RELEASES_URL}/download/v{version}/{asset_name}")
}

/// Re-check GitHub for the latest version, then download and extract it.
/// Used by the "Install New Version" button so the version is always fresh.
pub(crate) async fn check_and_download() -> Result<PathBuf, String> {
    let version = check_for_updates()
        .await
        .ok_or_else(|| "No newer version available".to_string())?;
    download_and_extract(version).await
}

/// Download and extract it to a temp directory, then return the path of extracted executable.
pub(crate) async fn download_and_extract(version: String) -> Result<PathBuf, String> {
    tracing::info!(version = %version, "downloading crabot update");
    let asset_name = platform_asset_name()?;
    let url = asset_download_url(&version, &asset_name);

    let bytes = reqwest::Client::builder()
        .user_agent(crabot::app_title())
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    tracing::debug!(version = %version, bytes = bytes.len(), "update downloaded");

    let extract_dir = std::env::temp_dir().join(format!("crabot-v{version}"));
    let asset_name_clone = asset_name.clone();
    let extract_dir_clone = extract_dir.clone();

    // Extraction is blocking; run it on the blocking thread pool.
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&extract_dir_clone).map_err(|e| e.to_string())?;
        let archive_path = extract_dir_clone.join(&asset_name_clone);
        std::fs::write(&archive_path, &bytes).map_err(|e| e.to_string())?;
        if asset_name_clone.ends_with(".zip") {
            extract_zip(&archive_path, &extract_dir_clone)?;
        } else {
            extract_tar_gz(&archive_path, &extract_dir_clone)?;
        }
        let _ = std::fs::remove_file(&archive_path);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;

    find_executable(&extract_dir)
}

fn extract_zip(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    zip.extract(dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn extract_tar_gz(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Find the extracted executable inside the extraction directory.
fn find_executable(dir: &std::path::Path) -> Result<PathBuf, String> {
    let exe_name = if cfg!(windows) {
        "crabot.exe"
    } else {
        "crabot"
    };
    for entry in walkdir(dir, 0) {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name == exe_name || name == exe_name.trim_end_matches(".exe") {
            return Ok(entry.path());
        }
    }
    Err(format!("Executable '{exe_name}' not found in {dir:?}"))
}

/// Replace the currently running executable with a new one.
///
/// Strategy (works on all platforms including Windows):
/// 1. Rename the current exe to `{{current}}.old` to free the path.
/// 2. Copy the new exe into the original location.
/// 3. The `{{current}}.old` leftover will be removed on the next update.
pub(crate) fn replace_current_exe(new_exe: &std::path::Path) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;

    let mut backup = current.clone();
    let orig_name = backup
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    backup.set_file_name(format!("{orig_name}.old"));
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&current, &backup).map_err(|e| e.to_string())?;
    std::fs::copy(new_exe, &current).map_err(|e| e.to_string())?;
    Ok(())
}

/// Shallow recursive walk of a directory (max depth 3).
fn walkdir(dir: &std::path::Path, depth: u32) -> Vec<Result<std::fs::DirEntry, std::io::Error>> {
    let mut out = Vec::new();
    if depth > 3 {
        return out;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) && depth < 3 {
                out.extend(walkdir(&e.path(), depth + 1));
            }
            out.push(Ok(e));
        }
    }
    out
}

/// Renders the "new version available" banner at the top of the window.
pub(crate) fn update_banner(
    latest: &str,
    download_state: &UpdateDownloadState,
) -> Element<'static, OverlayEvent> {
    let action = match download_state {
        UpdateDownloadState::Idle => {
            banner_button("Install New Version", 13.0, OverlayEvent::InstallUpdate)
        }
        UpdateDownloadState::Failed => {
            banner_button("Download Failed", 13.0, OverlayEvent::InstallUpdate)
        }
        UpdateDownloadState::InProgress => banner_button_disabled("⏳ Installing…", 13.0),
        UpdateDownloadState::ReadyToRestart(_) => {
            banner_button("Restart to Update", 13.0, OverlayEvent::RestartFromUpdate)
        }
    };
    container(
        row![
            text(format!(
                "🆕  Crabot v{latest} is available! (current: v{CURRENT_VERSION})"
            ))
            .size(13)
            .color(Color::WHITE),
            Space::new().width(Length::Fill),
            banner_button("View Release Notes", 13.0, OverlayEvent::OpenReleaseNotes),
            Space::new().width(8),
            action,
            Space::new().width(8),
            banner_button("✕", 14.0, OverlayEvent::DismissUpdateBanner),
        ]
        .align_y(Alignment::Center)
        .padding([4, 12]),
    )
    .width(Length::Fill)
    .style(update_banner_style)
    .into()
}

fn banner_button(
    label: &'static str,
    size: f32,
    on_press: OverlayEvent,
) -> button::Button<'static, OverlayEvent> {
    button(text(label).size(size).color(Color::WHITE))
        .style(update_banner_link_style)
        .on_press(on_press)
}

fn banner_button_disabled(label: &'static str, size: f32) -> button::Button<'static, OverlayEvent> {
    button(
        text(label)
            .size(size)
            .color(Color::from_rgba(1.0, 1.0, 1.0, 0.4)),
    )
    .style(update_banner_disabled_style)
}

fn update_banner_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(super::theme::CRABOT_PRIMARY.into()),
        ..container::Style::default()
    }
}

fn update_banner_link_style(_theme: &Theme, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Hovered => 0.25,
        button::Status::Pressed => 0.35,
        _ => 0.15,
    };
    button::Style {
        background: Some(Color::from_rgba(1.0, 1.0, 1.0, alpha).into()),
        text_color: Color::WHITE,
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

fn update_banner_disabled_style(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.08).into()),
        text_color: Color::from_rgba(1.0, 1.0, 1.0, 0.4),
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}
