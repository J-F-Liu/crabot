//! Shared XCap window facts and screen rectangles.

use xcap::Window;

/// A screen rectangle in pixels; callers name the coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// A rectangle from XCap's integer window or monitor geometry.
    pub fn from_geometry(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self::new(x as f32, y as f32, width as f32, height as f32)
    }

    pub fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Whether the point lies inside this rectangle.
    pub fn contains(self, (x, y): (f32, f32)) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    /// Whether the rectangle is finite and big enough to place a window at.
    pub fn is_usable(self) -> bool {
        self.width >= 1.0
            && self.height >= 1.0
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
    }

    /// Grow the rectangle by `by` on every side.
    pub fn expand(self, by: f32) -> Self {
        Self::new(
            self.x - by,
            self.y - by,
            self.width + 2.0 * by,
            self.height + 2.0 * by,
        )
    }

    /// Scale the rectangle's position and size.
    pub fn scaled(self, factor: f32) -> Self {
        Self::new(
            self.x * factor,
            self.y * factor,
            self.width * factor,
            self.height * factor,
        )
    }
}

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

    pub(crate) fn rect(&self) -> Option<Rect> {
        let (x, y) = self.position?;
        let rect = Rect::from_geometry(x, y, self.width, self.height);
        rect.is_usable().then_some(rect)
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
