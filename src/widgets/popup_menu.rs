//! A popup menu widget that positions an overlay above or below a trigger
//! element, automatically choosing the direction that fits within the
//! viewport.  Unlike `iced_aw`'s `DropDown`, this widget always leaves a
//! visible gap between the trigger and the menu in both directions.

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Clipboard, Shell, Widget};
use iced::keyboard;
use iced::mouse;
use iced::touch;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// A popup menu that appears adjacent to its trigger button, flipping
/// automatically to stay within the viewport.
///
/// # Example
/// ```ignore
/// PopupMenu::new(trigger_button, menu_content, expanded)
///     .width(Length::Fixed(360.0))
///     .height(Length::Fixed(180.0))
///     .offset_x(-10.0)
///     .gap(4.0)
///     .on_dismiss(Message::Close)
/// ```
pub struct PopupMenu<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Message: Clone,
{
    trigger: Element<'a, Message, Theme, Renderer>,
    menu: Element<'a, Message, Theme, Renderer>,
    expanded: bool,
    on_dismiss: Option<Message>,
    width: Length,
    height: Length,
    /// Maximum height of the popup menu; content scrolls beyond it.
    max_height: Option<f32>,
    /// Horizontal offset from the trigger's left edge (0 = flush).
    offset_x: f32,
    /// Align the menu's right edge with the trigger's right edge.
    right_aligned: bool,
    /// Vertical gap (logical pixels) between the trigger and the menu in
    /// both directions.
    gap: f32,
}

impl<'a, Message, Theme, Renderer> PopupMenu<'a, Message, Theme, Renderer>
where
    Message: Clone,
{
    /// Create a new [`PopupMenu`].
    pub fn new(
        trigger: impl Into<Element<'a, Message, Theme, Renderer>>,
        menu: impl Into<Element<'a, Message, Theme, Renderer>>,
        expanded: bool,
    ) -> Self {
        Self {
            trigger: trigger.into(),
            menu: menu.into(),
            expanded,
            on_dismiss: None,
            width: Length::Shrink,
            height: Length::Shrink,
            max_height: None,
            offset_x: 0.0,
            right_aligned: false,
            gap: 4.0,
        }
    }

    /// Width of the popup menu.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Height of the popup menu.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Maximum height of the popup menu; content scrolls beyond it.
    /// Pairs with [`height`](Self::height) set to `Length::Shrink` to hug
    /// content up to this cap.
    #[must_use]
    pub fn max_height(mut self, max_height: f32) -> Self {
        self.max_height = Some(max_height);
        self
    }

    /// Horizontal offset from the trigger's left edge (0 = flush).
    /// Negative pulls left; positive pushes right.
    #[must_use]
    pub fn offset_x(mut self, offset: f32) -> Self {
        self.offset_x = offset;
        self
    }

    /// Align the menu's right edge with the trigger's right edge
    /// (default: left edge with left edge).
    #[must_use]
    pub fn right_aligned(mut self) -> Self {
        self.right_aligned = true;
        self
    }

    /// Vertical gap between the trigger and the menu.
    #[must_use]
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// Message fired when a click occurs outside the menu while expanded,
    /// or when Escape is pressed.
    #[must_use]
    pub fn on_dismiss(mut self, message: Message) -> Self {
        self.on_dismiss = Some(message);
        self
    }
}

// ── Widget impl ────────────────────────────────────────────────────

/// Widget-tree state for PopupMenu (no persistent state needed).
struct PopupMenuState;

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PopupMenu<'_, Message, Theme, Renderer>
where
    Message: 'static + Clone,
    Renderer: 'static + renderer::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<PopupMenuState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(PopupMenuState)
    }

    fn size(&self) -> Size<Length> {
        self.trigger.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.trigger
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        state: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.trigger.as_widget().draw(
            &state.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.trigger), Tree::new(&self.menu)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.trigger, &self.menu]);
    }

    fn update(
        &mut self,
        state: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.trigger.as_widget_mut().update(
            &mut state.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        state: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.trigger.as_widget().mouse_interaction(
            &state.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        state: &'b mut Tree,
        layout: Layout<'b>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        if !self.expanded {
            return None;
        }

        let trigger_bounds = layout.bounds();
        let trigger_position = layout.position() + translation;

        Some(overlay::Element::new(Box::new(PopupOverlay {
            menu: &mut self.menu,
            menu_state: &mut state.children[1],
            on_dismiss: self.on_dismiss.as_ref(),
            width: &self.width,
            height: &self.height,
            max_height: self.max_height,
            offset_x: self.offset_x,
            right_aligned: self.right_aligned,
            gap: self.gap,
            trigger_bounds,
            trigger_position,
        })))
    }
}

// ── Overlay ────────────────────────────────────────────────────────

struct PopupOverlay<'a, 'b, Message, Theme, Renderer>
where
    Message: Clone,
{
    menu: &'b mut Element<'a, Message, Theme, Renderer>,
    menu_state: &'b mut Tree,
    on_dismiss: Option<&'b Message>,
    width: &'b Length,
    height: &'b Length,
    max_height: Option<f32>,
    offset_x: f32,
    right_aligned: bool,
    gap: f32,
    trigger_bounds: Rectangle,
    trigger_position: Point,
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for PopupOverlay<'_, '_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: renderer::Renderer,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let mut limits = layout::Limits::new(Size::ZERO, bounds)
            .width(*self.width)
            .height(*self.height);

        if let Some(max_height) = self.max_height {
            limits = limits.max_height(max_height);
        }

        let node = self
            .menu
            .as_widget_mut()
            .layout(self.menu_state, renderer, &limits);

        let menu_size = node.bounds();

        // Place below the trigger if it fits, otherwise above.
        let space_below =
            bounds.height - (self.trigger_position.y + self.trigger_bounds.height + self.gap);

        // X: flush with the trigger's left edge, or right edge if right-aligned.
        let x = (self.trigger_position.x
            + if self.right_aligned {
                self.trigger_bounds.width - menu_size.width
            } else {
                0.0
            }
            + self.offset_x)
            .clamp(0.0, (bounds.width - menu_size.width).max(0.0));

        let y = if space_below >= menu_size.height {
            // Place below the trigger.
            self.trigger_position.y + self.trigger_bounds.height + self.gap
        } else {
            // Place above the trigger.
            (self.trigger_position.y - self.gap - menu_size.height).max(0.0)
        };

        node.move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        self.menu.as_widget().draw(
            self.menu_state,
            renderer,
            theme,
            style,
            layout,
            cursor,
            &bounds,
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<Message>,
    ) {
        // Dismiss on Escape or click outside.
        if let Some(on_dismiss) = self.on_dismiss {
            // The trigger bounds are stored origin-relative; translate them to
            // absolute coordinates for hit-testing (the menu layout is
            // already absolute).
            let trigger_bounds =
                self.trigger_bounds + Vector::new(self.trigger_position.x, self.trigger_position.y);
            match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, .. })
                    if key == &keyboard::Key::Named(keyboard::key::Named::Escape) =>
                {
                    shell.publish(on_dismiss.clone());
                    shell.capture_event();
                    return;
                }
                Event::Mouse(mouse::Event::ButtonPressed(
                    mouse::Button::Left | mouse::Button::Right,
                ))
                | Event::Touch(touch::Event::FingerPressed { .. })
                    if !cursor.is_over(layout.bounds()) && !cursor.is_over(trigger_bounds) =>
                {
                    shell.publish(on_dismiss.clone());
                    shell.capture_event();
                    return;
                }
                _ => {}
            }
        }

        // Forward events to the menu.
        self.menu.as_widget_mut().update(
            self.menu_state,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.menu.as_widget().mouse_interaction(
            self.menu_state,
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        )
    }
}

// ── Element conversion ─────────────────────────────────────────────

impl<'a, Message, Theme: 'a, Renderer> From<PopupMenu<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'static + Clone,
    Renderer: 'static + renderer::Renderer,
{
    fn from(menu: PopupMenu<'a, Message, Theme, Renderer>) -> Self {
        Element::new(menu)
    }
}
