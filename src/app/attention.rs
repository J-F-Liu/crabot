//! Taskbar attention for background sessions: a finished turn flashes once,
//! a pending `ask` or a failed run keeps flashing until the window is focused.

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

/// Raise attention for an unfocused window.
pub(crate) fn raise(window_focused: bool, attention: Attention) -> Task<Message> {
    if window_focused {
        return Task::none();
    }
    // Windows flashes only the taskbar button: once, or repeatedly until focus.
    #[cfg(windows)]
    return match attention {
        Attention::Flash => {
            win32::flash_once();
            Task::none()
        }
        Attention::UntilFocused => request(Some(window::UserAttention::Informational)),
    };
    // Elsewhere the dock or taskbar bounces once, or repeats until focus.
    #[cfg(not(windows))]
    request(Some(match attention {
        Attention::Flash => window::UserAttention::Informational,
        Attention::UntilFocused => window::UserAttention::Critical,
    }))
}

/// Drop an outstanding request nothing needs any more.
pub(crate) fn clear() -> Task<Message> {
    request(None)
}

fn request(attention: Option<window::UserAttention>) -> Task<Message> {
    window::latest().then(move |id| match id {
        Some(id) => window::request_user_attention(id, attention),
        None => Task::none(),
    })
}

/// Single taskbar flash; the button stays highlighted until the window is activated.
#[cfg(windows)]
mod win32 {
    use std::ffi::{c_int, c_void};

    /// Flash only the taskbar button.
    const FLASHW_TRAY: u32 = 0x0000_0002;

    #[repr(C)]
    struct FlashWInfo {
        cb_size: u32,
        hwnd: *mut c_void,
        dw_flags: u32,
        u_count: u32,
        dw_timeout: u32,
    }

    unsafe extern "system" {
        fn FlashWindowEx(info: *mut FlashWInfo) -> c_int;
        fn EnumWindows(
            callback: extern "system" fn(*mut c_void, *mut c_void) -> c_int,
            param: *mut c_void,
        ) -> c_int;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
        fn GetCurrentProcessId() -> u32;
        fn IsWindowVisible(hwnd: *mut c_void) -> c_int;
    }

    // First visible top-level window of this process.
    extern "system" fn own_window(hwnd: *mut c_void, param: *mut c_void) -> c_int {
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId() && IsWindowVisible(hwnd) != 0 {
                *(param as *mut *mut c_void) = hwnd;
                return 0; // stop enumerating
            }
        }
        1
    }

    pub(crate) fn flash_once() {
        let mut hwnd: *mut c_void = std::ptr::null_mut();
        unsafe { EnumWindows(own_window, &mut hwnd as *mut _ as *mut c_void) };
        if hwnd.is_null() {
            return;
        }
        let mut info = FlashWInfo {
            cb_size: std::mem::size_of::<FlashWInfo>() as u32,
            hwnd,
            dw_flags: FLASHW_TRAY,
            u_count: 1,
            dw_timeout: 0,
        };
        unsafe { FlashWindowEx(&mut info) };
    }
}
