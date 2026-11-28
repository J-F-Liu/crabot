//! Bring another application's window to the foreground.

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use xcap::Window;

use crate::tools::{CANCEL_REASON, Rect, WindowFacts, window_facts};

/// How long the window manager gets to hand over the foreground.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(1);
/// Poll interval while waiting for the foreground switch.
const FOCUS_POLL: Duration = Duration::from_millis(50);
/// Pause after restoring a minimized window, before requesting the foreground.
#[cfg(windows)]
const RESTORE_SETTLE: Duration = Duration::from_millis(50);

/// A resolved window that can be focused.
pub(crate) struct TargetWindow {
    id: u32,
    window: Window,
}

/// A window that is now in the foreground.
pub(crate) struct Focused {
    /// True when the window was restored from a minimized state.
    pub(crate) restored: bool,
    /// One-line report naming the window, without repeating the parameters.
    pub(crate) line: String,
    /// The window's screen rectangle, for the notice frame around it.
    pub(crate) rect: Option<Rect>,
}

/// Resolve `id` without changing focus.
pub(crate) fn resolve_window(id: u32, cancel: &CancellationToken) -> Result<TargetWindow, String> {
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let windows = Window::all().map_err(|error| format!("Failed to enumerate windows: {error}"))?;
    windows
        .into_iter()
        .find(|window| window.id().is_ok_and(|window_id| window_id == id))
        .map(|window| TargetWindow { id, window })
        .ok_or_else(|| {
            format!(
                "No window with id {id}; take a screenshot or run `process` `wait_window` to \
                 list the current windows."
            )
        })
}

impl TargetWindow {
    /// Bring the window to the foreground, restoring it when minimized.
    pub(crate) fn focus(&self, cancel: &CancellationToken) -> Result<Focused, String> {
        let before = window_facts(&self.window);
        #[cfg(target_os = "macos")]
        if before.is_minimized() {
            return Err(format!(
                "Window \"{}\" (id {}) is minimized; macOS cannot restore another application's \
                 window — restore it first.",
                window_label(&before),
                self.id
            ));
        }
        self.activate(&before, cancel)?;
        let restored = before.is_minimized();
        let action = if restored {
            "Restored minimized window"
        } else {
            "Focused window"
        };
        Ok(Focused {
            restored,
            line: format!("{action} \"{}\".", window_label(&before)),
            // Focus or restore may have changed the window's geometry.
            rect: window_facts(&self.window).rect(),
        })
    }

    /// Restore focus after the notice closes, before injecting actions.
    pub(crate) fn raise(&self, cancel: &CancellationToken) -> Result<(), String> {
        self.activate(&window_facts(&self.window), cancel)
    }

    fn activate(&self, facts: &WindowFacts, cancel: &CancellationToken) -> Result<(), String> {
        focus_native(self.id, facts.pid)
            .and_then(|()| wait_for_focus(&self.window, cancel))
            .map_err(|error| foreground_error(facts, error))
    }
}

/// Poll until the platform reports the window as focused.
fn wait_for_focus(window: &Window, cancel: &CancellationToken) -> Result<(), String> {
    let deadline = Instant::now() + FOCUS_TIMEOUT;
    loop {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        if window.is_focused().unwrap_or(false) {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("the window manager did not give it the foreground".into());
        }
        std::thread::sleep(remaining.min(FOCUS_POLL));
    }
}

/// Wrap a platform failure, keeping a user cancel bare.
fn foreground_error(facts: &WindowFacts, error: String) -> String {
    if error == CANCEL_REASON {
        error
    } else {
        format!(
            "Could not bring window \"{}\" (id {}) to the foreground: {error}",
            window_label(facts),
            facts.id
        )
    }
}

/// A window's title, falling back to its application name.
fn window_label(facts: &WindowFacts) -> &str {
    if !facts.title.is_empty() {
        &facts.title
    } else if !facts.app.is_empty() {
        &facts.app
    } else {
        "window"
    }
}

/// Restore (when needed) and ask the platform to switch the foreground.
#[cfg(windows)]
fn focus_native(id: u32, _pid: u32) -> Result<(), String> {
    // xcap truncates HWND to u32; sign-extend it back to a handle.
    foreground(id as i32 as isize, true)
}

/// Foreground a native handle; `settle` is skipped on the UI thread.
#[cfg(windows)]
pub fn foreground(hwnd: isize, settle: bool) -> Result<(), String> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SW_RESTORE,
        SetForegroundWindow, ShowWindow,
    };

    let hwnd = hwnd as HWND;
    // SAFETY: calls only read state or request a foreground switch.
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
            if settle {
                // Restoring is asynchronous; let the window become visible first.
                std::thread::sleep(RESTORE_SETTLE);
            }
        }
        if SetForegroundWindow(hwnd) != 0 {
            return Ok(());
        }
        // Windows only honors a switch from the foreground thread.
        let current = GetForegroundWindow();
        if current.is_null() {
            return Err("there is no foreground window to attach to".into());
        }
        let current_thread = GetWindowThreadProcessId(current, std::ptr::null_mut());
        let this_thread = GetCurrentThreadId();
        if current_thread == 0 || current_thread == this_thread {
            return Err("Windows refused the foreground switch".into());
        }
        if AttachThreadInput(current_thread, this_thread, 1) == 0 {
            return Err("Windows refused to attach to the foreground thread".into());
        }
        let raised = SetForegroundWindow(hwnd) != 0 || BringWindowToTop(hwnd) != 0;
        AttachThreadInput(current_thread, this_thread, 0);
        if raised {
            Ok(())
        } else {
            Err("Windows refused the foreground switch".into())
        }
    }
}

/// Ask the X11 window manager to activate the window (`_NET_ACTIVE_WINDOW`).
#[cfg(target_os = "linux")]
fn focus_native(id: u32, _pid: u32) -> Result<(), String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, StackMode,
    };

    let (conn, screen) =
        x11rb::connect(None).map_err(|error| format!("no X11 display: {error}"))?;
    let root = conn.setup().roots[screen].root;
    let active_window = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?
        .atom;
    // source = 2 (pager / direct user action), timestamp = CurrentTime.
    let event = ClientMessageEvent::new(32, id, active_window, [2, x11rb::CURRENT_TIME, 0, 0, 0]);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )
    .map_err(|error| error.to_string())?;
    // Window managers without EWMH support still honor a stacking request.
    conn.configure_window(id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))
        .map_err(|error| error.to_string())?;
    conn.flush().map_err(|error| error.to_string())?;
    Ok(())
}

/// Activate the window's application; multi-window apps may show another window.
#[cfg(target_os = "macos")]
fn focus_native(_id: u32, pid: u32) -> Result<(), String> {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
        .ok_or("the window's application is no longer running")?;
    if app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
        Ok(())
    } else {
        Err("macOS refused to activate the application".into())
    }
}
