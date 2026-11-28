//! UI commands for the notice shown before `send_input`.
//! [`Notifier`] closes the notice when the actions end, fail, or are cancelled.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures::stream::BoxStream;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use xcap::Monitor;

use crate::lock;
use crate::tools::{CANCEL_REASON, Rect};

/// How long the frame stays on screen before the actions start.
pub const COUNTDOWN: Duration = Duration::from_secs(2);
/// Frame border thickness, in logical pixels.
pub const BORDER: f32 = 4.0;
/// Size of the label capsule, in logical pixels.
pub const LABEL_SIZE: (f32, f32) = (520.0, 64.0);
/// The text announced in the middle of the frame.
pub const LABEL: &str = "Crabot is about to control the mouse and keyboard";
/// Smallest gap between the label capsule and the frame border.
const LABEL_MARGIN: f32 = 16.0;
/// Longest wait for the UI to run the countdown; a stuck UI must not hang the tool.
const SHOW_TIMEOUT: Duration = Duration::from_secs(5);
/// Cancel poll interval while waiting for the UI.
const POLL: Duration = Duration::from_millis(25);

/// One frame window: a border around `rect`, optionally announcing the input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Window geometry, in logical pixels.
    pub rect: Rect,
    /// Physical pixels per logical pixel on this frame's monitor.
    pub scale: f32,
    /// Whether this frame shows the label; only one frame per notice does.
    pub announce: bool,
}

impl Frame {
    /// The centered, clamped label rectangle relative to the frame window.
    pub fn label(&self) -> Rect {
        let width = LABEL_SIZE
            .0
            .min(self.rect.width - 2.0 * LABEL_MARGIN)
            .max(0.0);
        let height = LABEL_SIZE
            .1
            .min(self.rect.height - 2.0 * LABEL_MARGIN)
            .max(0.0);
        Rect::new(
            (self.rect.width - width) / 2.0,
            (self.rect.height - height) / 2.0,
            width,
            height,
        )
    }

    /// The frame window's rectangle in physical pixels.
    pub fn physical(&self) -> Rect {
        self.rect.scaled(self.scale)
    }
}

/// A request for the UI thread.
pub enum Command {
    /// Frame the given targets and release `go` after [`COUNTDOWN`].
    Show {
        frames: Vec<Frame>,
        /// Released when the countdown ends; a closed channel also ends the wait.
        go: Sender<()>,
    },
    /// Stop framing; `refocus` brings Crabot back once the actions are over.
    Finish { refocus: bool },
}

static COMMANDS: LazyLock<Mutex<Vec<Command>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static PING: LazyLock<broadcast::Sender<()>> = LazyLock::new(|| broadcast::channel(16).0);
/// Whether a UI is running to show the notice at all.
static UI_READY: AtomicBool = AtomicBool::new(false);
/// Probed from the main window; native Wayland cannot clip notice frames.
static OVERLAY_SUPPORTED: AtomicBool = AtomicBool::new(!cfg!(target_os = "linux"));

/// Declare that a UI is running and will drain [`take_commands`].
pub fn activate() {
    UI_READY.store(true, Ordering::Relaxed);
}

/// Record whether the window backend can place and clip a frame window.
pub fn set_overlay_supported(supported: bool) {
    OVERLAY_SUPPORTED.store(supported, Ordering::Relaxed);
    if !supported {
        tracing::info!("the window backend cannot clip the screen notice");
    }
}

/// Stream yielding one tick on subscribe plus one tick per queued command.
pub fn events() -> BoxStream<'static, ()> {
    crate::broadcast_ticks(&PING)
}

fn push(command: Command) {
    lock(&COMMANDS).push(command);
    let _ = PING.send(());
}

/// Take the queued commands, oldest first.
pub fn take_commands() -> Vec<Command> {
    std::mem::take(&mut *lock(&COMMANDS))
}

/// Closes the notice when the actions finish, fail, or are cancelled.
pub struct Notifier {
    /// Whether the main window should return once the actions are over.
    refocus: bool,
}

impl Drop for Notifier {
    fn drop(&mut self) {
        push(Command::Finish {
            refocus: self.refocus,
        });
    }
}

/// Frame the pointer's screen and, when known, the target window.
pub fn frames_for(window: Option<Rect>, pointer: Option<(i32, i32)>) -> Vec<Frame> {
    if !OVERLAY_SUPPORTED.load(Ordering::Relaxed) {
        return Vec::new();
    }
    frames_from(&screens(), window, pointer)
}

/// The frames for `window` and `pointer` on the known `screens`.
fn frames_from(
    screens: &[Screen],
    window: Option<Rect>,
    pointer: Option<(i32, i32)>,
) -> Vec<Frame> {
    let pointer = pointer.map(|(x, y)| (x as f32, y as f32));
    let current = pointer
        .and_then(|pointer| screens.iter().position(|s| s.physical.contains(pointer)))
        .or_else(|| screens.iter().position(|screen| screen.primary))
        .unwrap_or(0);
    // Only the pointer's screen announces, even when the target sits elsewhere.
    let mut frames = match screens.get(current) {
        Some(screen) => vec![Frame {
            rect: screen.logical,
            scale: screen.scale,
            announce: true,
        }],
        None => Vec::new(),
    };
    // The target window gets a second frame on top of the screen one.
    if let Some(rect) = window.filter(|rect| rect.is_usable())
        && let Some(screen) = screens
            .iter()
            .find(|screen| screen.physical.contains(rect.center()))
    {
        frames.push(Frame {
            rect: rect.scaled(1.0 / screen.scale).expand(BORDER),
            scale: screen.scale,
            announce: false,
        });
    }
    frames
}

/// Block until the countdown ends; return `None` if no frame can be shown.
pub fn show(frames: Vec<Frame>, cancel: &CancellationToken) -> Result<Option<Notifier>, String> {
    if frames.is_empty() || !UI_READY.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let (go, released) = std::sync::mpsc::channel();
    push(Command::Show { frames, go });
    // Any early return drops the notifier, which queues `Finish`.
    let mut notifier = Notifier { refocus: true };
    let deadline = Instant::now() + SHOW_TIMEOUT;
    loop {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        match released.recv_timeout(POLL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return Ok(Some(notifier)),
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                // Late frames must not cover the actions or steal their focus.
                notifier.refocus = false;
                tracing::warn!("the screen notice timed out; starting the actions");
                return Ok(None);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// A monitor in physical pixels and `iced` logical pixels.
struct Screen {
    physical: Rect,
    logical: Rect,
    /// Physical pixels per logical pixel.
    scale: f32,
    primary: bool,
}

fn screens() -> Vec<Screen> {
    let monitors = match Monitor::all() {
        Ok(monitors) => monitors,
        Err(error) => {
            tracing::warn!("failed to enumerate monitors for the screen notice: {error}");
            return Vec::new();
        }
    };
    monitors.iter().filter_map(screen).collect()
}

fn screen(monitor: &Monitor) -> Option<Screen> {
    let rect = Rect::from_geometry(
        monitor.x().unwrap_or_default(),
        monitor.y().unwrap_or_default(),
        monitor.width().unwrap_or_default(),
        monitor.height().unwrap_or_default(),
    );
    if !rect.is_usable() {
        return None;
    }
    // XCap uses physical pixels on Windows, logical pixels on X11, and points on macOS.
    #[cfg(windows)]
    let (physical, logical, scale) = {
        let scale = monitor_scale(monitor);
        (rect, rect.scaled(1.0 / scale), scale)
    };
    #[cfg(target_os = "linux")]
    let (physical, logical, scale) = {
        let scale = monitor_scale(monitor);
        (rect.scaled(scale), rect, scale)
    };
    #[cfg(target_os = "macos")]
    let (physical, logical, scale) = (rect, rect, 1.0);
    Some(Screen {
        physical,
        logical,
        scale,
        primary: monitor.is_primary().unwrap_or(false),
    })
}

/// Physical pixels per logical pixel.
#[cfg(not(target_os = "macos"))]
fn monitor_scale(monitor: &Monitor) -> f32 {
    monitor
        .scale_factor()
        .ok()
        .filter(|scale| *scale > 0.0)
        .unwrap_or(1.0)
}
