// Hide the console window in release builds. Debug builds keep the console
// for `println!`/`eprintln!` output during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod acp;
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
    AGENTS_MD, DATE, FilepathEntry, PREAMBLE, SKILLS, TOOLS, WORKSPACE, WORKSPACE_TREE,
};
pub(crate) use app::session_state::{AskAction, AskRequest};
pub(crate) use app::{
    CenterPaneEvent, ConversationEvent, FocusedTarget, LeftPaneEvent, OverlayEvent, PromptEvent,
    RightPaneEvent, ToolEvent,
};

use crate::views::theme::MIN_W;

/// Apply the stored backend choice to `ICED_BACKEND` before iced starts:
/// auto/empty leaves it unset, anything else (trimmed) is set verbatim.
fn apply_iced_backend(backend: &str) {
    let value = backend.trim();
    // SAFETY: single-threaded startup, before iced reads the var in `run()`.
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        unsafe { std::env::remove_var("ICED_BACKEND") };
    } else {
        unsafe { std::env::set_var("ICED_BACKEND", value) };
    }
}

pub fn main() -> iced::Result {
    let _log_guard = setup::init_logging();
    let saved = crabot::settings::Settings::load();
    // Iced reads `ICED_BACKEND` once in `run()`, so apply it beforehand.
    apply_iced_backend(&saved.iced_backend);
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        iced_backend = %saved.iced_backend,
        "crabot starting"
    );
    setup::ensure_default_files();
    fonts::load_system_fonts();
    // Apply system-proxy settings before any HTTP client is built.
    tools::configure_proxy(
        saved.use_system_proxy_for_llm,
        saved.use_system_proxy_for_tools,
    );
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
