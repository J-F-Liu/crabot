//! UI state for the frames shown before `send_input`.
//! Closing the frames and releasing the waiting tool happen in one update.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

use iced::window;
use iced::{Element, Point, Size, Task, Widget as _};

use crabot::tools::notice::{self, Command, Frame};

use super::attention::{self, Attention, Owner};
use super::{App, Message};

/// How long the closed frames get to disappear before the main window returns.
const RESTORE_SETTLE: Duration = Duration::from_millis(80);
/// Re-place frames after monitor/DPI changes settle.
const SETTLE_DELAYS: [Duration; 2] = [Duration::from_millis(300), Duration::from_millis(1500)];

/// Messages routed to the notice state machine.
#[derive(Debug, Clone)]
pub(crate) enum NoticeEvent {
    /// One tick per queued command, plus one at startup.
    Tick,
    /// An overlay window finished opening.
    Opened(window::Id),
    /// The visible countdown ended.
    Countdown(u64),
    /// Re-apply the frame geometry after the window manager settled it.
    Settle(u64),
    /// Give the main window the foreground after the frames close.
    FocusMain(u64),
}

#[derive(Default)]
pub(crate) struct NoticeState {
    /// Open frame windows, with the frame each one shows.
    frames: HashMap<window::Id, Frame>,
    /// Frame windows still being created.
    opening: usize,
    /// Invalidates stale countdowns and focus requests.
    generation: u64,
    /// Releases the waiting tool call when the countdown ends.
    go: Option<Sender<()>>,
}

impl NoticeState {
    /// The frame `window` shows, when it is one of ours.
    fn frame(&self, window: window::Id) -> Option<Frame> {
        self.frames.get(&window).copied()
    }

    /// Whether `generation` is the notice still waiting for its actions.
    fn is_live(&self, generation: u64) -> bool {
        generation == self.generation && self.go.is_some()
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.frames.is_empty()
    }
}

fn after(delay: Duration, event: NoticeEvent) -> Task<Message> {
    Task::perform(tokio::time::sleep(delay), move |()| Message::Notice(event))
}

/// Apply one notice event.
pub(crate) fn update(app: &mut App, event: NoticeEvent) -> Task<Message> {
    match event {
        NoticeEvent::Tick => {
            notice::take_commands()
                .into_iter()
                .fold(Task::none(), |task, command| {
                    let next = match command {
                        Command::Show { frames, go } => show(app, frames, go),
                        Command::Finish { refocus } => finish(app, refocus),
                    };
                    task.chain(next)
                })
        }
        NoticeEvent::Opened(id) => opened(app, id),
        NoticeEvent::Countdown(generation) => countdown(app, generation),
        NoticeEvent::Settle(generation) => settle(app, generation),
        NoticeEvent::FocusMain(generation) => focus_main(app, generation),
    }
}

/// Open a frame window per target and hold them for the countdown.
fn show(app: &mut App, frames: Vec<Frame>, go: Sender<()>) -> Task<Message> {
    app.notice.generation += 1;
    app.notice.go = Some(go);
    let mut tasks = vec![close(app)];
    for frame in frames {
        let (id, opened) = window::open(overlay_settings(&frame));
        app.notice.frames.insert(id, frame);
        app.notice.opening += 1;
        tasks.push(opened.map(move |_| Message::Notice(NoticeEvent::Opened(id))));
    }
    if app.notice.opening == 0 {
        // Nothing to frame on this display: run the actions right away.
        tasks.push(release(app));
    }
    Task::batch(tasks)
}

/// Shape the window and enable click-through before showing it.
fn opened(app: &mut App, id: window::Id) -> Task<Message> {
    let Some(frame) = app.notice.frame(id) else {
        // The notice closed before this window finished opening.
        tracing::debug!(%id, "closing a notice frame that outlived its notice");
        return window::close(id);
    };
    let mut tasks = vec![present(id, frame)];
    app.notice.opening = app.notice.opening.saturating_sub(1);
    if app.notice.opening == 0 {
        let generation = app.notice.generation;
        tasks.push(after(notice::COUNTDOWN, NoticeEvent::Countdown(generation)));
        for delay in SETTLE_DELAYS {
            tasks.push(after(delay, NoticeEvent::Settle(generation)));
        }
    }
    Task::batch(tasks)
}

/// Re-place every frame, in case the window manager resized it on a DPI change.
fn settle(app: &mut App, generation: u64) -> Task<Message> {
    if !app.notice.is_live(generation) {
        return Task::none();
    }
    Task::batch(
        app.notice
            .frames
            .iter()
            .map(|(id, frame)| shape(*id, *frame).discard()),
    )
}

/// Close the frame and release the waiting actions.
fn countdown(app: &mut App, generation: u64) -> Task<Message> {
    if !app.notice.is_live(generation) {
        return Task::none();
    }
    Task::batch([close(app), release(app)])
}

/// Start the taskbar flash and release the waiting call.
fn release(app: &mut App) -> Task<Message> {
    if let Some(go) = app.notice.go.take()
        && go.send(()).is_err()
    {
        // The cancelled call's notifier already queued `Finish`.
        tracing::debug!("the screen notice was released after the call stopped waiting");
    }
    // Do not replace a session's outstanding attention request.
    if attention::owned_by(Owner::Session) {
        return Task::none();
    }
    attention::raise(
        app.main_window,
        app.layout.window_focused,
        Attention::UntilFocused,
        Owner::Notice,
    )
}

/// The actions ended: stop flashing and bring the main window back.
fn finish(app: &mut App, refocus: bool) -> Task<Message> {
    // Invalidate a countdown still pending for this call.
    app.notice.generation += 1;
    app.notice.go = None;
    let generation = app.notice.generation;
    let main = app.main_window;
    let mut tasks = vec![close(app), attention::clear(main, Owner::Notice)];
    if refocus {
        // Let the frames disappear before the main window comes back.
        tasks.push(window::minimize(main, false));
        tasks.push(after(RESTORE_SETTLE, NoticeEvent::FocusMain(generation)));
    }
    Task::batch(tasks)
}

/// Bring the main window back; Windows needs its native foreground path.
fn focus_main(app: &App, generation: u64) -> Task<Message> {
    // A newer notice owns the screen; leave its target in front.
    if generation != app.notice.generation {
        return Task::none();
    }
    let id = app.main_window;
    #[cfg(windows)]
    {
        window::run(id, activate).discard()
    }
    #[cfg(not(windows))]
    {
        window::gain_focus(id)
    }
}

/// Activate the window from its native handle, logging a refusal.
#[cfg(windows)]
fn activate(window: &dyn window::Window) {
    use iced::window::raw_window_handle::RawWindowHandle;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    if let RawWindowHandle::Win32(raw) = handle.as_raw()
        && let Err(error) = crabot::tools::foreground(raw.hwnd.get(), false)
    {
        tracing::debug!("could not bring the main window back: {error}");
    }
}

/// Close every frame window, keeping the state ready for the next notice.
fn close(app: &mut App) -> Task<Message> {
    app.notice.opening = 0;
    Task::batch(
        std::mem::take(&mut app.notice.frames)
            .into_keys()
            .map(window::close),
    )
}

/// Native Wayland cannot clip frames; X11/XWayland and other backends can.
pub(crate) fn supports_overlay(window: &dyn window::Window) -> bool {
    #[cfg(target_os = "linux")]
    {
        use iced::window::raw_window_handle::RawWindowHandle;

        matches!(
            window.window_handle().map(|handle| handle.as_raw()),
            Ok(RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_))
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        true
    }
}

/// A hidden, borderless, always-on-top window shaped by [`present`].
fn overlay_settings(frame: &Frame) -> window::Settings {
    window::Settings {
        size: Size::new(frame.rect.width.max(1.0), frame.rect.height.max(1.0)),
        position: window::Position::Specific(Point::new(frame.rect.x, frame.rect.y)),
        visible: false,
        resizable: false,
        closeable: false,
        minimizable: false,
        decorations: false,
        // Only macOS needs per-pixel alpha; Windows and X11 clip the window.
        transparent: cfg!(target_os = "macos"),
        level: window::Level::AlwaysOnTop,
        exit_on_close_request: false,
        // `shape` also prevents the frame from taking focus.
        #[cfg(windows)]
        platform_specific: window::settings::PlatformSpecific {
            skip_taskbar: true,
            drag_and_drop: false,
            ..Default::default()
        },
        // An unmanaged frame cannot take the focus from the target window.
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            override_redirect: true,
            ..Default::default()
        },
        ..window::Settings::default()
    }
}

/// Apply the window region on the window thread.
fn shape(id: window::Id, frame: Frame) -> Task<()> {
    window::run(id, move |window| shape::apply(window, frame))
}

/// Shape the frame, let clicks through, and show it once it is ready.
fn present(id: window::Id, frame: Frame) -> Task<Message> {
    shape(id, frame)
        .discard::<Message>()
        .chain(mouse_passthrough(id))
        // `iced` has no show action; switching back to windowed maps the window.
        .chain(window::set_mode(id, window::Mode::Windowed))
}

/// Windows only exposes the clipped ring, so it needs no explicit passthrough.
fn mouse_passthrough(id: window::Id) -> Task<Message> {
    #[cfg(not(windows))]
    {
        window::enable_mouse_passthrough(id)
    }
    #[cfg(windows)]
    {
        let _ = id;
        Task::none()
    }
}

/// Render one overlay, or nothing if it is already closing.
pub(crate) fn view(
    state: &NoticeState,
    window: window::Id,
    lang: crabot::i18n::Lang,
) -> Element<'static, Message> {
    match state.frame(window) {
        Some(frame) => crate::views::screen_notice::frame(frame, lang),
        None => iced::widget::Space::new().boxed(),
    }
}

/// Window clipping, the one place where the notice needs platform code.
#[cfg(windows)]
mod shape {
    use iced::window;
    use iced::window::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        CombineRgn, CreateRectRgn, DeleteObject, RGN_DIFF, RGN_OR, SetWindowRgn,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, SWP_NOACTIVATE,
        SetWindowLongPtrW, SetWindowPos, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    use crabot::tools::notice::{BORDER, Frame};

    pub(super) fn apply(window: &dyn window::Window, frame: Frame) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(raw) = handle.as_raw() else {
            return;
        };
        let hwnd = raw.hwnd.get() as HWND;
        // SAFETY: the handle belongs to this window.
        unsafe {
            // Use physical pixels because the toolkit may assume another DPI.
            let physical = frame.physical();
            let wanted = (
                pixel(physical.x),
                pixel(physical.y),
                pixel(physical.width),
                pixel(physical.height),
            );
            let mut current = RECT::default();
            GetWindowRect(hwnd, &mut current);
            let placed = (
                current.left,
                current.top,
                current.right - current.left,
                current.bottom - current.top,
            );
            if placed != wanted {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    wanted.0,
                    wanted.1,
                    wanted.2,
                    wanted.3,
                    SWP_NOACTIVATE,
                );
            }
            // Hide from Alt+Tab/taskbar and preserve the target's focus.
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let wanted = style | WS_EX_TOOLWINDOW as isize | WS_EX_NOACTIVATE as isize;
            if wanted != style {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
            }

            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) == 0 {
                return;
            }
            let scale = client.right as f32 / frame.rect.width.max(1.0);
            let thickness = pixel(BORDER * scale).max(1);
            let ring = CreateRectRgn(0, 0, client.right, client.bottom);
            if ring.is_null() {
                return;
            }
            let hole = CreateRectRgn(
                thickness,
                thickness,
                client.right - thickness,
                client.bottom - thickness,
            );
            if !hole.is_null() {
                CombineRgn(ring, ring, hole, RGN_DIFF);
                DeleteObject(hole);
            }
            if frame.announce {
                let label = frame.label();
                let capsule = CreateRectRgn(
                    pixel(label.x * scale),
                    pixel(label.y * scale),
                    pixel((label.x + label.width) * scale),
                    pixel((label.y + label.height) * scale),
                );
                if !capsule.is_null() {
                    CombineRgn(ring, ring, capsule, RGN_OR);
                    DeleteObject(capsule);
                }
            }
            // A window owns its region only after a successful call.
            if SetWindowRgn(hwnd, ring, 1) == 0 {
                DeleteObject(ring);
            }
        }
    }

    fn pixel(value: f32) -> i32 {
        value.round() as i32
    }
}

#[cfg(target_os = "linux")]
mod shape {
    use iced::window;
    use iced::window::raw_window_handle::RawWindowHandle;
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
    use x11rb::protocol::xproto::{
        ClipOrdering, ConfigureWindowAux, ConnectionExt as _, Rectangle,
    };
    use x11rb::rust_connection::RustConnection;

    use crabot::tools::notice::{BORDER, Frame};

    /// One connection for every frame, instead of one per placement.
    static CONNECTION: std::sync::Mutex<Option<RustConnection>> = std::sync::Mutex::new(None);

    pub(super) fn apply(window: &dyn window::Window, frame: Frame) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let id = match handle.as_raw() {
            RawWindowHandle::Xlib(raw) => raw.window as u32,
            RawWindowHandle::Xcb(raw) => raw.window.get(),
            // Wayland cannot shape a window; the notice is skipped there.
            _ => return,
        };
        with_connection(|connection| {
            // Use physical pixels because the toolkit may assume another DPI.
            let physical = frame.physical();
            let _ = connection.configure_window(
                id,
                &ConfigureWindowAux::new()
                    .x(physical.x as i32)
                    .y(physical.y as i32)
                    .width(physical.width as u32)
                    .height(physical.height as u32),
            );
            let _ = connection.flush();
            let Ok(cookie) = connection.get_geometry(id) else {
                return;
            };
            let Ok(geometry) = cookie.reply() else {
                return;
            };
            let scale = f32::from(geometry.width) / frame.rect.width.max(1.0);
            let (width, height) = (geometry.width as i16, geometry.height as i16);
            let thickness = ((BORDER * scale).round() as i16)
                .max(1)
                .min(width / 2)
                .min(height / 2);
            let bar = |x: i16, y: i16, width: i16, height: i16| Rectangle {
                x,
                y,
                width: width as u16,
                height: height as u16,
            };
            let mut rects = vec![
                bar(0, 0, width, thickness),
                bar(0, height - thickness, width, thickness),
                bar(0, 0, thickness, height),
                bar(width - thickness, 0, thickness, height),
            ];
            if frame.announce {
                let label = frame.label();
                let span = |value: f32| (value * scale).round().max(0.0) as u16;
                rects.push(Rectangle {
                    x: (label.x * scale).round() as i16,
                    y: (label.y * scale).round() as i16,
                    width: span(label.width),
                    height: span(label.height),
                });
            }
            let _ = connection.shape_rectangles(
                SO::SET,
                SK::BOUNDING,
                ClipOrdering::UNSORTED,
                id,
                0,
                0,
                &rects,
            );
            let _ = connection.flush();
        });
    }

    /// Run `f` with the shared X11 connection, opening it on first use.
    fn with_connection(f: impl FnOnce(&RustConnection)) {
        let mut connection = crabot::lock(&CONNECTION);
        if connection.is_none() {
            *connection = x11rb::connect(None).ok().map(|(connection, _)| connection);
        }
        if let Some(connection) = connection.as_ref() {
            f(connection);
        }
    }
}

/// macOS has no window region: the transparent frame draws its own border.
#[cfg(target_os = "macos")]
mod shape {
    use iced::window;

    use crabot::tools::notice::Frame;

    pub(super) fn apply(_window: &dyn window::Window, _frame: Frame) {}
}
