//! Taskbar attention for background sessions: a finished turn flashes once,
//! a pending `ask` or a failed run keeps flashing until the window is focused.

use std::sync::Mutex;

use iced::{Task, window};

use crate::app::Message;

/// How long the taskbar keeps asking for attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attention {
    /// A single one-shot request.
    Flash,
    /// Repeat flashing until the window regains focus.
    UntilFocused,
}

/// Who asked for the current taskbar attention request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A session turn or a pending question.
    Session,
    /// The screen notice shown before `send_input` acts.
    Notice,
}

/// The owner of the outstanding request, so one feature cannot cancel another's.
static OWNER: Mutex<Option<Owner>> = Mutex::new(None);

/// Raise attention for an unfocused window.
pub(crate) fn raise(
    main: window::Id,
    window_focused: bool,
    attention: Attention,
    owner: Owner,
) -> Task<Message> {
    if window_focused {
        return Task::none();
    }
    *crabot::lock(&OWNER) = Some(owner);
    // Windows flashes only the taskbar button: once, or repeatedly until focus.
    #[cfg(windows)]
    return match attention {
        Attention::Flash => window::run(main, win32::flash).discard(),
        Attention::UntilFocused => {
            window::request_user_attention(main, Some(window::UserAttention::Informational))
        }
    };
    // Elsewhere the dock or taskbar bounces once, or repeats until focus.
    #[cfg(not(windows))]
    window::request_user_attention(
        main,
        Some(match attention {
            Attention::Flash => window::UserAttention::Informational,
            Attention::UntilFocused => window::UserAttention::Critical,
        }),
    )
}

/// Drop the request `owner` raised; anyone else's request is left alone.
pub(crate) fn clear(main: window::Id, owner: Owner) -> Task<Message> {
    {
        let mut current = crabot::lock(&OWNER);
        if *current != Some(owner) {
            return Task::none();
        }
        *current = None;
    }
    window::request_user_attention(main, None)
}

/// Whether `owner` has an outstanding request.
pub(crate) fn owned_by(owner: Owner) -> bool {
    *crabot::lock(&OWNER) == Some(owner)
}

/// Forget the outstanding request after the window regained the focus, which
/// already stopped it.
pub(crate) fn forget() {
    *crabot::lock(&OWNER) = None;
}

/// Single taskbar flash.
#[cfg(windows)]
mod win32 {
    use iced::window;
    use iced::window::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_TRAY, FLASHWINFO, FlashWindowEx};

    /// Flash the main window's taskbar button once, whatever else is on screen.
    pub(super) fn flash(window: &dyn window::Window) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(raw) = handle.as_raw() else {
            return;
        };
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: raw.hwnd.get() as HWND,
            dwFlags: FLASHW_TRAY,
            uCount: 1,
            dwTimeout: 0,
        };
        // SAFETY: the handle belongs to this window; the call only flashes it.
        unsafe { FlashWindowEx(&info) };
    }
}
