//! Version-check banner shown at the top of the window when a newer Crabot
//! release is available on crates.io.

use std::time::Duration;

use iced::{
    Alignment, Border, Color, Element, Length, Theme,
    widget::{Space, button, container, row, text},
};
use semver::Version;
use serde::Deserialize;

use crate::Message;

/// Version of the running binary.
pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// GitHub releases page opened from the banner.
pub(crate) const RELEASES_URL: &str = "https://github.com/J-F-Liu/crabot/releases";
/// crates.io endpoint listing the most recent Crabot versions, newest first.
const CRATES_IO_URL: &str = "https://crates.io/api/v1/crates/crabot/versions?per_page=5";

#[derive(Deserialize)]
struct VersionsResponse {
    versions: Vec<CrateVersion>,
}

#[derive(Deserialize)]
struct CrateVersion {
    num: String,
    yanked: bool,
}

/// Query crates.io for the latest stable version of Crabot.
/// Returns `Some(version)` if a newer version exists, `None` otherwise.
pub(crate) async fn check_for_updates() -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent(crate::crabot_title())
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let response: VersionsResponse = client
        .get(CRATES_IO_URL)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    // Pick the newest stable release, skipping yanked and pre-release versions.
    let latest = response
        .versions
        .into_iter()
        .filter(|v| !v.yanked)
        .filter_map(|v| Version::parse(&v.num).ok())
        .filter(|v| v.pre.is_empty())
        .max()?;
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

/// Renders the "new version available" banner at the top of the window.
pub(crate) fn update_banner(latest: &str) -> Element<'static, Message> {
    container(
        row![
            text(format!(
                "🆕  Crabot v{latest} is available! (current: v{CURRENT_VERSION})"
            ))
            .size(13)
            .color(Color::WHITE),
            Space::new().width(Length::Fill),
            banner_button("View Release Notes", 13.0, Message::OpenReleaseNotes),
            Space::new().width(8),
            banner_button("✕", 14.0, Message::DismissUpdateBanner),
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
    on_press: Message,
) -> button::Button<'static, Message> {
    button(text(label).size(size).color(Color::WHITE))
        .style(update_banner_link_style)
        .on_press(on_press)
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
