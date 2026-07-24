use iced::Task;

use crate::app::{App, Message, OverlayEvent};

pub(crate) fn update(app: &mut App, event: OverlayEvent) -> Task<Message> {
    match event {
        OverlayEvent::VersionCheckResult(latest) => {
            app.overlay.update_available = latest;
        }
        OverlayEvent::DismissUpdateBanner => {
            app.overlay.update_available = None;
        }
        OverlayEvent::OpenReleaseNotes => {
            if let Err(error) = open::that(crate::views::update::RELEASES_URL) {
                eprintln!("Failed to open release notes: {error}");
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
