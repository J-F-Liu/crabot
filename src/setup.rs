use std::io::Write;
use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets");

/// Embedded default models configuration.
pub fn default_models() -> &'static str {
    ASSETS
        .get_file("models.ron")
        .and_then(|f| f.contents_utf8())
        .unwrap_or("")
}

/// The crabot config directory (`~/.crabot`), falling back to a cwd-relative
/// `.crabot` when no home directory is available.
pub fn config_dir() -> PathBuf {
    home::home_dir().unwrap_or_default().join(".crabot")
}

/// The default workspace path (`~/.crabot`) used when no workspace is set.
pub fn default_workspace_path() -> PathBuf {
    config_dir()
}

/// Mirror panics to `~/.crabot/logs/panic.log` so GUI crashes stay diagnosable
/// when stderr is hidden; file-write failures are ignored.
fn install_panic_hook(log_dir: &Path) {
    let panic_path = log_dir.join("panic.log");
    std::panic::set_hook(Box::new(move |info| {
        let name = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_owned();
        let mut record = format!("thread '{name}' {info}");
        if std::env::var_os("RUST_BACKTRACE").is_some() {
            record.push_str(&format!(
                "\n{:?}",
                std::backtrace::Backtrace::force_capture()
            ));
        }
        eprintln!("{record}");
        if let Ok(mut file) = std::fs::File::create(&panic_path) {
            let _ = writeln!(
                file,
                "{} {record}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
        }
    }));
}

/// Initialize the tracing logger: daily-rolling files under `~/.crabot/logs/`.
/// In debug builds output is also mirrored to stderr for development.
///
/// The returned guard must be kept alive for the process lifetime so that
/// buffered log lines are flushed to disk on exit.
pub fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
    let log_dir = default_workspace_path().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    install_panic_hook(&log_dir);

    // Daily-rolling file appender, e.g. `crabot.log.2026-08-13`.
    let file_appender = tracing_appender::rolling::daily(&log_dir, "crabot.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,iced_winit=error,iced_wgpu=error,genai::adapter::adapters::openai::streamer=error,iced_futures::subscription::tracker=error",
        )
    });

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false) // No ANSI color codes in log files.
        .with_writer(non_blocking);

    let registry = tracing_subscriber::registry().with(filter).with(file_layer);

    // Debug builds keep the console visible during development.
    #[cfg(debug_assertions)]
    let registry = registry.with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr));

    // `try_init` keeps a duplicate call from panicking; the first subscriber wins.
    let _ = registry.try_init();
    guard
}

/// On first boot, seed `~/.crabot/` with compiled-in default assets.
pub fn ensure_default_files() {
    let crabot_dir = default_workspace_path();
    let _ = std::fs::create_dir_all(&crabot_dir);
    tracing::debug!(dir = %crabot_dir.display(), "ensuring default files");

    seed_bundled_dir(&crabot_dir, "preamble");
    seed_bundled_dir(&crabot_dir, "skills");
    seed_bundled_file(&crabot_dir, "tools.ron");
    seed_bundled_file(&crabot_dir, "mcp.ron");
    seed_bundled_file(&crabot_dir, "settings.ron");
}

/// Copy missing files from a bundled `assets/<name>` directory into
/// `~/.crabot/<name>` — never overwrite user edits.
fn seed_bundled_dir(crabot_dir: &Path, name: &str) {
    let dir_path = crabot_dir.join(name);
    let _ = std::fs::create_dir_all(&dir_path);
    if let Some(dir) = ASSETS.get_dir(name) {
        for file in dir.files() {
            if let Some(file_name) = file.path().file_name() {
                let dest = dir_path.join(file_name);
                if !dest.is_file() {
                    let _ = std::fs::write(&dest, file.contents());
                }
            }
        }
    }
}

/// Copy a bundled asset file into `~/.crabot/` if missing.
fn seed_bundled_file(crabot_dir: &Path, name: &str) {
    let dest = crabot_dir.join(name);
    if !dest.is_file()
        && let Some(file) = ASSETS.get_file(name)
    {
        let _ = std::fs::write(&dest, file.contents());
    }
}
