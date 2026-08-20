//! About page: logo, version, homepage link, and update check.

use std::sync::LazyLock;

use iced::{
    Alignment, Color, Element, Length, padding,
    widget::{Space, button, column, image, row, scrollable, text, toggler},
};

use super::{BOLD, SettingsEvent, SettingsState};
use crate::views::styles;
use crate::views::theme::{CRABOT_PRIMARY, color_muted, color_text_strong};

/// Image handle for the embedded logo, loaded once from the compiled-in assets.
fn logo_handle() -> image::Handle {
    static LOGO: LazyLock<image::Handle> = LazyLock::new(|| {
        image::Handle::from_bytes(
            crate::setup::ASSETS
                .get_file("images/logo.png")
                .map(|f| f.contents())
                .unwrap_or(b""),
        )
    });
    LOGO.clone()
}

/// The heading/display name of the application.
const APP_NAME: &str = "Crabot";
/// Current version from Cargo.toml.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// GitHub repository URL.
pub(crate) const HOMEPAGE: &str = "https://github.com/J-F-Liu/crabot";

// ── Events ──────────────────────────────────────────────────────────

/// Tracks the state of the update check on the About tab.
#[derive(Debug, Clone)]
pub(crate) enum UpdateCheck {
    /// No check has been performed and no cached result is known.
    Idle,
    /// An update check is in progress.
    Checking,
    /// The latest check found no newer version.
    UpToDate,
    /// A newer version is available.
    Available(String),
}

/// Events for the About tab.
#[derive(Debug, Clone)]
pub(crate) enum AboutEvent {
    /// User manually requested an update check.
    CheckForUpdate,
    /// Result of a manual update check.
    UpdateCheckResult(Option<String>),
    /// Open the project homepage in the browser.
    OpenHomepage,
    /// Toggle auto-check-updates preference.
    ToggleAutoCheckUpdates(bool),
}

/// Renders the About page with logo, version info, homepage link,
/// update check button, and auto-check toggle.
pub(crate) fn about_page<'a>(state: &'a super::SettingsState) -> Element<'a, SettingsEvent> {
    let logo = image(logo_handle())
        .width(80)
        .height(80)
        .content_fit(iced::ContentFit::Contain);

    let name = text(APP_NAME).size(24).font(BOLD).color(CRABOT_PRIMARY);

    let version = text(format!("Version {VERSION}"))
        .size(16)
        .color(color_text_strong());

    // Homepage link button.
    let homepage_btn = button(text(HOMEPAGE).size(13))
        .style(link_button_style)
        .on_press(SettingsEvent::About(AboutEvent::OpenHomepage));

    // Update check section.
    let check_update_btn = button(text("Check for Updates").size(13))
        .style(styles::secondary_button)
        .on_press_maybe(if matches!(state.update_check, UpdateCheck::Checking) {
            None
        } else {
            Some(SettingsEvent::About(AboutEvent::CheckForUpdate))
        });

    let update_status = match &state.update_check {
        UpdateCheck::Checking => text("Checking…").size(13).color(color_muted()),
        UpdateCheck::UpToDate => text("You're up to date!").size(13).color(color_muted()),
        UpdateCheck::Available(version) => text(format!("v{version} available"))
            .size(13)
            .color(Color::from_rgb(0.2, 0.7, 0.3)),
        UpdateCheck::Idle => text(""),
    };

    let update_row =
        row![check_update_btn, Space::new().width(12), update_status,].align_y(Alignment::Center);

    // Auto-check toggle.
    let auto_check = toggler(state.auto_check_updates)
        .on_toggle(move |v| SettingsEvent::About(AboutEvent::ToggleAutoCheckUpdates(v)))
        .size(18);

    let auto_check_label = text("Automatically check for new versions on startup")
        .size(13)
        .color(color_text_strong());

    let auto_check_row =
        row![auto_check, Space::new().width(8), auto_check_label].align_y(Alignment::Center);

    // ── Layout ─────────────────────────────────────────────────────
    scrollable(
        column![
            logo,
            Space::new().height(16),
            name,
            Space::new().height(8),
            version,
            Space::new().height(8),
            homepage_btn,
            Space::new().height(24),
            update_row,
            Space::new().height(16),
            auto_check_row,
        ]
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .spacing(0)
        .padding(padding::right(24)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Link button style: muted by default, primary color on hover.
fn link_button_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        text_color: if hovered {
            CRABOT_PRIMARY
        } else {
            color_muted()
        },
        border: iced::Border {
            radius: 0.0.into(),
            ..iced::Border::default()
        },
        ..button::Style::default()
    }
}

// ── Update ─────────────────────────────────────────────────────────

/// Handle an About tab event, mutating the update-check state.
pub(super) fn update(state: &mut SettingsState, event: AboutEvent) {
    match event {
        AboutEvent::CheckForUpdate => {
            state.update_check = UpdateCheck::Checking;
        }
        AboutEvent::UpdateCheckResult(latest) => {
            state.update_check = match latest {
                Some(version) => UpdateCheck::Available(version),
                None => UpdateCheck::UpToDate,
            };
        }
        AboutEvent::ToggleAutoCheckUpdates(v) => {
            state.auto_check_updates = v;
        }
        AboutEvent::OpenHomepage => {} // handled in app/settings.rs
    }
}
