//! Shared XCap window facts for the screenshot and process tools.

use xcap::Window;

/// A window's facts, extracted so the XCap handle can stay on the worker.
pub(crate) struct WindowFacts {
    /// Owning process id, or 0 when the platform does not report one.
    pub pid: u32,
    pub id: u32,
    pub title: String,
    pub app: String,
    pub width: u32,
    pub height: u32,
    pub position: Option<(i32, i32)>,
    /// `None` when the platform failed to report the state.
    pub minimized: Option<bool>,
}

impl WindowFacts {
    /// Whether the window is known to be minimized.
    pub(crate) fn is_minimized(&self) -> bool {
        self.minimized.unwrap_or(false)
    }
}

/// Extract `window`'s facts.
pub(crate) fn window_facts(window: &Window) -> WindowFacts {
    let position = window.x().ok().zip(window.y().ok());
    WindowFacts {
        pid: window.pid().unwrap_or_default(),
        id: window.id().unwrap_or_default(),
        title: window.title().unwrap_or_default(),
        app: window.app_name().unwrap_or_default(),
        width: window.width().unwrap_or_default(),
        height: window.height().unwrap_or_default(),
        position,
        minimized: window.is_minimized().ok(),
    }
}
