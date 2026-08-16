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
                tracing::warn!("Failed to open release notes: {error}");
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
                    tracing::error!("Update download failed: {e}");
                    UpdateDownloadState::Failed
                }
            };
        }
        OverlayEvent::RestartFromUpdate => {
            if let UpdateDownloadState::ReadyToRestart(path) = &app.overlay.download_state {
                let path = path.clone();
                // Snapshot exe path before rename — on Linux /proc/self/exe follows the rename.
                let current_exe = std::env::current_exe().ok();
                app.settings.last_update_version = None;
                app.save_settings();
                if let Err(e) = update::replace_current_exe(&path) {
                    tracing::error!("Failed to replace executable: {e}");
                    app.overlay.download_state = UpdateDownloadState::Failed;
                    return Task::none();
                }
                // Spawn the new binary (now at the original path) and exit.
                match current_exe {
                    Some(exe) => {
                        tracing::info!(exe = %exe.display(), "restarting into updated crabot");
                        if let Err(e) = std::process::Command::new(&exe).spawn() {
                            tracing::error!("failed to spawn updated executable: {e}");
                        }
                    }
                    None => {
                        tracing::error!("cannot determine current exe for restart after update")
                    }
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
        OverlayEvent::RevertAllConfirm(confirmed) => {
            app.overlay.show_revert_all_confirm = false;
            if confirmed {
                return crate::app::snapshot::revert_all(app);
            }
        }
    }
    Task::none()
}
