//! Version-check banner shown at the top of the window when a newer Crabot
//! release is available on GitHub.

use std::path::PathBuf;
use std::sync::Mutex;
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

/// Latest update-extraction dir: keeps the returned exe path alive until
/// restart; replaced (and cleaned up) by the next update.
static EXTRACT_DIR: Mutex<Option<tempfile::TempDir>> = Mutex::new(None);

/// Download state machine for the "Install New Version" button.
#[derive(Clone)]
pub(crate) enum UpdateDownloadState {
    /// Not started.
    Idle,
    /// Download in progress; `total` is `None` without a Content-Length.
    InProgress { downloaded: u64, total: Option<u64> },
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

/// Download percentage when the total is known and non-zero.
pub(crate) fn progress_percent(downloaded: u64, total: Option<u64>) -> Option<u32> {
    total
        .filter(|t| *t > 0)
        .map(|t| (downloaded.saturating_mul(100) / t).min(100) as u32)
}

/// Check for a newer version, then download and extract it, calling
/// `on_progress` with `(downloaded_bytes, total_bytes)` per body chunk.
pub(crate) async fn check_and_download(
    on_progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<PathBuf, String> {
    let version = check_for_updates()
        .await
        .ok_or_else(|| "No newer version available".to_string())?;
    download_and_extract(version, on_progress).await
}

/// Download `version`, extract it to a temp dir, and return the executable path.
pub(crate) async fn download_and_extract(
    version: String,
    mut on_progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<PathBuf, String> {
    tracing::info!(version = %version, "downloading crabot update");
    let asset_name = platform_asset_name()?;
    let url = asset_download_url(&version, &asset_name);

    // Stream the body chunk-by-chunk so the UI can report download progress.
    let mut response = reqwest::Client::builder()
        .user_agent(crabot::app_title())
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let total = response.content_length();
    let mut downloaded = 0;
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(64 * 1024 * 1024) as usize);
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        downloaded += chunk.len() as u64;
        bytes.extend_from_slice(&chunk);
        on_progress(downloaded, total);
    }
    tracing::debug!(version = %version, bytes = bytes.len(), "update downloaded");

    // Extraction is blocking; the unique temp dir auto-cleans on failure
    // and when the next update replaces it.
    let extract_dir = tokio::task::spawn_blocking(move || {
        let dir = tempfile::Builder::new()
            .prefix("crabot-update-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let archive_path = dir.path().join(&asset_name);
        std::fs::write(&archive_path, &bytes).map_err(|e| e.to_string())?;
        if asset_name.ends_with(".zip") {
            extract_zip(&archive_path, dir.path())?;
        } else {
            extract_tar_gz(&archive_path, dir.path())?;
        }
        let _ = std::fs::remove_file(&archive_path);
        Ok::<_, String>(dir)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Keep the dir alive: the exe path must stay valid until restart.
    let exe = find_executable(extract_dir.path())?;
    *crabot::lock(&EXTRACT_DIR) = Some(extract_dir);
    Ok(exe)
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
        UpdateDownloadState::InProgress { downloaded, total } => {
            let label = match progress_percent(*downloaded, *total) {
                Some(pct) => format!("⏳ Downloading… {pct}%"),
                None => format!("⏳ Downloading… {:.1} MB", *downloaded as f64 / 1_048_576.0),
            };
            banner_button_disabled(label, 13.0)
        }
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

fn banner_button_disabled(label: String, size: f32) -> button::Button<'static, OverlayEvent> {
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
