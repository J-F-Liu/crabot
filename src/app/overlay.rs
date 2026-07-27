use iced::Task;

use crate::app::{App, Message, OverlayEvent};
use crate::views::update::{self, UpdateDownloadState};

pub(crate) fn update(app: &mut App, event: OverlayEvent) -> Task<Message> {
    match event {
        OverlayEvent::VersionCheckResult(latest) => {
            if latest.is_some() {
                app.settings.last_update_version = latest.clone();
                app.save_settings();
            }
            app.overlay.update_available = latest;
        }
        OverlayEvent::DismissUpdateBanner => {
            app.overlay.update_available = None;
            app.overlay.download_state = UpdateDownloadState::Idle;
        }
        OverlayEvent::OpenReleaseNotes => {
            if let Err(error) = open::that(update::RELEASES_URL) {
                eprintln!("Failed to open release notes: {error}");
            }
        }
        OverlayEvent::InstallUpdate => {
            app.overlay.download_state = UpdateDownloadState::InProgress;
            return Task::perform(update::check_and_download(), |result| {
                Message::Overlay(OverlayEvent::UpdateReady(result))
            });
        }
        OverlayEvent::UpdateReady(result) => {
            app.overlay.download_state = match result {
                Ok(path) => UpdateDownloadState::ReadyToRestart(path),
                Err(e) => {
                    eprintln!("Update download failed: {e}");
                    UpdateDownloadState::Failed
                }
            };
        }
        OverlayEvent::RestartFromUpdate => {
            if let UpdateDownloadState::ReadyToRestart(path) = &app.overlay.download_state {
                let path = path.clone();
                app.settings.last_update_version = None;
                app.save_settings();
                if let Err(e) = update::replace_current_exe(&path) {
                    eprintln!("Failed to replace executable: {e}");
                    app.overlay.download_state = UpdateDownloadState::Failed;
                    return Task::none();
                }
                // Spawn the replaced binary and exit.
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(&exe).spawn();
                }
                return iced::exit();
            }
        }
        OverlayEvent::EmptyWorkspaceConfirm(path) => {
            app.overlay.show_workspace_dialog = false;
            let Some(path) = path else {
                return Task::none();
            };
            return Task::batch([
                crate::app::prompt::set_workspace(app, path),
                crate::app::conversation::send_prompt(app),
            ]);
        }
    }
    Task::none()
}
