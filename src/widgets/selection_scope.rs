//! Restricts iced's global text selection to the region where the drag began.

use std::any::Any;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::operation::{
    Focusable, Operation, Outcome, Scrollable, Text, TextInput,
};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::{Meta, Widget};
use iced::advanced::{Shell, overlay, renderer};
use iced::mouse;
use iced::widget::Id;
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, Widget as _};

/// Restricts text selection to the region containing `origin`.
struct SelectionScope<'a, Message> {
    content: Element<'a, Message>,
    origin: Option<Point>,
}

pub(crate) fn selection_scope<'a, Message: 'a>(
    content: impl Widget<Message, Theme, Renderer> + 'a,
    origin: Option<Point>,
) -> Element<'a, Message> {
    SelectionScope {
        content: content.boxed(),
        origin,
    }
    .boxed()
}

impl<Message> Meta for SelectionScope<'_, Message> {
    fn is_void(&self) -> bool {
        self.content.is_void()
    }
}

impl<Message> Widget<Message, Theme, Renderer> for SelectionScope<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.tag()
    }

    fn state(&self) -> tree::State {
        self.content.state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.content.layout(&mut tree.children[0], renderer, limits);
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
        self.content.draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        match self.origin {
            Some(origin) if !layout.bounds().contains(origin) => {
                let mut scoped = DeselectText { inner: operation };
                self.content.operate(
                    &mut tree.children[0],
                    layout,
                    viewport,
                    renderer,
                    &mut scoped,
                );
            }
            _ => {
                self.content
                    .operate(&mut tree.children[0], layout, viewport, renderer, operation);
            }
        }
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
        self.content.update(
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
        self.content
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
        self.content.overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

/// Forwards every operation except text operations, which clear the selection.
struct DeselectText<'a> {
    inner: &'a mut dyn Operation,
}

impl Operation for DeselectText<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        self.inner
            .traverse(&mut |inner| operate(&mut DeselectText { inner }));
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, viewport: &Rectangle) {
        self.inner.container(id, bounds, viewport);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content: Size,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        self.inner
            .scrollable(id, bounds, content, translation, state);
    }

    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        self.inner.focusable(id, bounds, state);
    }

    fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
        self.inner.text_input(id, bounds, state);
    }

    fn text(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Text) {
        state.deselect();
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Any) {
        self.inner.custom(id, bounds, state);
    }

    fn finish(&self) -> Outcome<()> {
        self.inner.finish()
    }
}
