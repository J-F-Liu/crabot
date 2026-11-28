// Hide the console window in release builds. Debug builds keep the console
// for `println!`/`eprintln!` output during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod acp;
mod app;
mod fonts;
mod llm;
mod system;
mod views;
mod widgets;

use app::App;
use crabot::{model, setup, tools};
use iced::{Point, Size};

// Re-export items that view modules access via `crate::*`.
pub(crate) use app::prompt::{
    AGENTS_MD, DATE, FilepathEntry, PREAMBLE, SKILLS, WORKSPACE, WORKSPACE_TREE,
};
pub(crate) use app::session_state::{AskAction, AskRequest};
pub(crate) use app::{
    CenterPaneEvent, ConversationEvent, FocusedTarget, LeftPaneEvent, OverlayEvent, PromptEvent,
    RightPaneEvent, ToolEvent,
};

use crate::views::theme::MIN_W;

/// Map a stored backend name to an iced [`Backend`], keeping legacy names working.
fn parse_backend(backend: &str) -> iced::Backend {
    let value = backend.trim();
    match value.to_ascii_lowercase().as_str() {
        "" | "auto" => iced::Backend::Best,
        "wgpu" => iced::Backend::Hardware(iced::backend::Api::Best),
        // Legacy software renderer names.
        "tiny_skia" | "tiny-skia" | "software" | "vello-cpu" | "vello_cpu" => {
            iced::Backend::Software
        }
        _ => iced::Backend::Custom(value.to_owned()),
    }
}

pub fn main() -> iced::Result {
    let _log_guard = setup::init_logging();
    // Must precede any HTTP client: reqwest is built with `rustls-no-provider`.
    setup::install_crypto_provider();
    let saved = crabot::settings::Settings::load();
    let backend = parse_backend(&saved.iced_backend);
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
        // iced's `image` feature is off, so decode the ICO with our own `image` crate.
        let rgba = image::load_from_memory_with_format(f.contents(), image::ImageFormat::Ico)
            .ok()?
            .to_rgba8();
        let (width, height) = (rgba.width(), rgba.height());
        iced::window::icon::from_rgba(rgba.into_raw(), width, height).ok()
    });
    let window = iced::window::Settings {
        size,
        position,
        exit_on_close_request: false,
        icon,
        ..Default::default()
    };
    iced::daemon(
        move || App::boot(saved.clone(), window.clone()),
        App::update,
        App::view,
    )
    .settings(iced::Settings {
        backend,
        ..Default::default()
    })
    .subscription(App::subscription)
    .theme(|state: &App, _window| Some(state.layout.theme.clone()))
    .style(App::style)
    .title(|state: &App, window| {
        if window == state.main_window {
            state.window_title()
        } else {
            String::new()
        }
    })
    .antialiasing(true)
    .run()
}
