//! Popup menu overlay that flips above or below its trigger to stay on-screen.

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::tree::Tree;
use iced::advanced::{Shell, Widget};
use iced::keyboard;
use iced::mouse;
use iced::touch;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Popup menu adjacent to its trigger, always leaving a small gap.
///
/// # Example
/// ```ignore
/// PopupMenu::new(trigger_button, menu_content, expanded)
///     .width(Length::Fixed(360.0))
///     .height(Length::Fixed(180.0))
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
        trigger: impl iced::Widget<Message, Theme, Renderer> + 'a,
        menu: impl iced::Widget<Message, Theme, Renderer> + 'a,
        expanded: bool,
    ) -> Self {
        Self {
            trigger: trigger.boxed(),
            menu: menu.boxed(),
            expanded,
            on_dismiss: None,
            width: Length::Shrink,
            height: Length::Shrink,
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

impl<Message, Theme, Renderer> iced::advanced::widget::Meta
    for PopupMenu<'_, Message, Theme, Renderer>
where
    Message: Clone,
{
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PopupMenu<'_, Message, Theme, Renderer>
where
    Message: 'static + Clone,
    Renderer: 'static + renderer::Renderer,
{
    fn size(&self) -> Size<Length> {
        self.trigger.size()
    }

    fn diff(&mut self, tree: &mut Tree) {
        if tree.children.len() != 2 {
            tree.children = vec![Tree::new(&self.trigger), Tree::new(&self.menu)];
        }
        tree.children[0].diff(&mut self.trigger);
        tree.children[1].diff(&mut self.menu);
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.trigger.layout(&mut tree.children[0], renderer, limits);

        tree.size = tree.children[0].size;
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.trigger.draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.trigger.update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.trigger
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        if !self.expanded {
            return Vec::new();
        }

        let trigger_bounds = layout.bounds();
        let trigger_position = layout.position() + translation;

        let menu_state = &mut tree.children[1];

        let limits = layout::Limits::new(Size::ZERO, viewport.size())
            .width(self.width)
            .height(self.height);

        self.menu.layout(menu_state, renderer, &limits);
        let menu_size = menu_state.size;

        // Place below the trigger if it fits, otherwise above.
        let space_below = window.height - (trigger_position.y + trigger_bounds.height + self.gap);

        // X: flush with the trigger's left edge, or right edge if right-aligned.
        let x_offset = if self.right_aligned {
            trigger_bounds.width - menu_size.width
        } else {
            0.0
        };
        let x =
            (trigger_position.x + x_offset).clamp(0.0, (window.width - menu_size.width).max(0.0));

        let y = if space_below >= menu_size.height {
            // Place below the trigger.
            trigger_position.y + trigger_bounds.height + self.gap
        } else {
            // Place above the trigger.
            (trigger_position.y - self.gap - menu_size.height).max(0.0)
        };

        vec![overlay::Element::new(Box::new(PopupOverlay {
            menu: &mut self.menu,
            menu_state,
            layout: Layout::new(menu_size).move_to(Point::new(x, y)),
            trigger_bounds: Rectangle::new(trigger_position, trigger_bounds.size()),
            on_dismiss: self.on_dismiss.as_ref(),
            window,
        }))]
    }
}

// ── Overlay ────────────────────────────────────────────────────────

struct PopupOverlay<'a, 'b, Message, Theme, Renderer>
where
    Message: Clone,
{
    menu: &'b mut Element<'a, Message, Theme, Renderer>,
    menu_state: &'b mut Tree,
    layout: Layout,
    trigger_bounds: Rectangle,
    on_dismiss: Option<&'b Message>,
    window: Size,
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for PopupOverlay<'_, '_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: renderer::Renderer,
{
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        let bounds = self.layout.bounds();

        // Overlays draw after the base tree; a layer keeps the menu above any
        // clipping layers created by the base widgets and inside its bounds.
        renderer.with_layer(bounds, |renderer| {
            self.menu.draw(
                self.menu_state,
                renderer,
                theme,
                style,
                self.layout,
                cursor,
                &bounds,
            );
        });
    }

    fn update(
        &mut self,
        event: &Event,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<Message>,
    ) {
        // Dismiss on Escape or click outside.
        if let Some(on_dismiss) = self.on_dismiss {
            let menu_bounds = self.layout.bounds();
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
                    if !cursor.is_over(menu_bounds) && !cursor.is_over(self.trigger_bounds) =>
                {
                    shell.publish(on_dismiss.clone());
                    shell.capture_event();
                    return;
                }
                _ => {}
            }
        }

        // Forward events to the menu.
        self.menu.update(
            self.menu_state,
            event,
            self.layout,
            cursor,
            renderer,
            shell,
            &Rectangle::with_size(self.window),
        );
    }

    fn mouse_interaction(&self, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        self.menu.mouse_interaction(
            self.menu_state,
            self.layout,
            cursor,
            &self.layout.bounds(),
            renderer,
        )
    }
}
