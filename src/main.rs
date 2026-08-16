// Hide the console window in release builds. Debug builds keep the console
// for `println!`/`eprintln!` output during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fonts;
mod llm;
mod views;
mod widgets;

use app::App;
use crabot::{model, setup, tools};
use iced::{Point, Size};

// Re-export items that view modules access via `crate::*`.
pub(crate) use app::prompt::{
    AGENTS_MD, DATE, FilepathEntry, PREAMBLE, RULES, TOOLS, WORKSPACE, WORKSPACE_TREE,
};
pub(crate) use app::session_state::{AskAction, AskRequest};
pub(crate) use app::{
    CenterPaneEvent, ConversationEvent, FocusedTarget, LeftPaneEvent, OverlayEvent, PromptEvent,
    RightPaneEvent, ToolEvent,
};

use crate::views::theme::MIN_W;

pub fn main() -> iced::Result {
    let _log_guard = setup::init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "crabot starting");
    setup::ensure_default_files();
    fonts::load_system_fonts();
    let saved = crabot::settings::Settings::load();
    let size = Size::new(
        saved.window_size.0.max(MIN_W),
        saved.window_size.1.max(200.0),
    );
    let position =
        iced::window::Position::Specific(Point::new(saved.window_pos.0, saved.window_pos.1));
    let icon = setup::ASSETS.get_file("images/icon.ico").and_then(|f| {
        iced::window::icon::from_file_data(f.contents(), Some(image::ImageFormat::Ico)).ok()
    });
    iced::application(move || App::boot(saved.clone()), App::update, App::view)
        .subscription(App::subscription)
        .theme(|state: &App| state.layout.theme.clone())
        .window(iced::window::Settings {
            size,
            position,
            exit_on_close_request: false,
            icon,
            ..Default::default()
        })
        .title(crabot::app_title())
        .antialiasing(true)
        .run()
}
