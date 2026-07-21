//! A dropdown picker widget with a configurable popup menu width.
//!
//! Unlike iced's built-in `PickList` which constrains the dropdown menu to the
//! widget's own width, this widget allows the popup to be wider for displaying
//! longer option labels (e.g. session titles).
//!
//! The popup menu supports mouse-wheel scrolling, a draggable scrollbar,
//! click-to-page on the scrollbar track, and keyboard navigation
//! (Home/End/Enter/Escape/PageUp/PageDown).

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::text::paragraph;
use iced::advanced::text::{self, Text};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Clipboard, Shell, Widget};
use iced::alignment;
use iced::border;
use iced::keyboard;
use iced::mouse;
use iced::{
    Background, Border, Color, Element, Event, Length, Padding, Pixels, Point, Rectangle, Shadow,
    Size, Vector,
};

use std::borrow::Borrow;

// ── style types ────────────────────────────────────────────────────

/// The appearance of a [`DropDown`] trigger.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub text_color: Color,
    pub placeholder_color: Color,
    pub handle_color: Color,
    pub background: Background,
    pub border: Border,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            text_color: Color::from_rgb(0.2, 0.2, 0.2),
            placeholder_color: Color::from_rgb(0.5, 0.5, 0.5),
            handle_color: Color::from_rgb(0.4, 0.4, 0.4),
            background: Background::Color(Color::from_rgb(0.9, 0.9, 0.9)),
            border: Border::default().rounded(6),
        }
    }
}

/// The appearance of a [`DropDown`] menu popup.
#[derive(Debug, Clone, Copy)]
pub struct MenuStyle {
    pub background: Background,
    pub border: Border,
    pub text_color: Color,
    pub selected_text_color: Color,
    pub selected_background: Background,
    pub shadow: Shadow,
}

impl Default for MenuStyle {
    fn default() -> Self {
        Self {
            background: Background::Color(Color::from_rgb(0.9, 0.9, 0.9)),
            border: Border::default().rounded(6),
            text_color: Color::from_rgb(0.2, 0.2, 0.2),
            selected_text_color: Color::WHITE,
            selected_background: Background::Color(Color::from_rgb(0.1, 0.6, 0.55)),
            shadow: Shadow::default(),
        }
    }
}

// ── scrollbar constants ────────────────────────────────────────────

/// Width of the scrollbar thumb/track in logical pixels.
const SB_WIDTH: f32 = 6.0;
/// Gap between the menu's right inner edge and the scrollbar.
const SB_MARGIN: f32 = 2.0;
/// Minimum thumb height so it stays grabbable even with many options.
const SB_MIN_THUMB: f32 = 20.0;
/// Total horizontal space reserved on the right for the scrollbar area.
const SB_RESERVED: f32 = SB_WIDTH + SB_MARGIN * 2.0;

/// Computed scrollbar geometry for one frame.
struct ScrollbarGeometry {
    track: Rectangle,
    thumb: Rectangle,
    max_scroll: f32,
}

/// Compute the scrollbar geometry for the given menu bounds and content.
fn scrollbar_geometry(
    bounds: Rectangle,
    total_height: f32,
    scroll_offset: f32,
) -> Option<ScrollbarGeometry> {
    let max_scroll = (total_height - bounds.height).max(0.0);
    if max_scroll <= 0.0 {
        return None;
    }

    let track = Rectangle {
        x: bounds.x + bounds.width - SB_WIDTH - SB_MARGIN,
        y: bounds.y,
        width: SB_WIDTH,
        height: bounds.height,
    };
    let thumb_height = (bounds.height / total_height * bounds.height)
        .max(SB_MIN_THUMB)
        .min(bounds.height);
    let thumb_y = track.y + (scroll_offset / max_scroll) * (track.height - thumb_height);
    let thumb = Rectangle {
        y: thumb_y,
        height: thumb_height,
        ..track
    };
    Some(ScrollbarGeometry {
        track,
        thumb,
        max_scroll,
    })
}

/// Ensure `scroll_offset` makes option `idx` visible within the viewport.
fn scroll_to_option(
    scroll_offset: &mut f32,
    idx: usize,
    option_height: f32,
    viewport_height: f32,
    max_scroll: f32,
) {
    let option_top = idx as f32 * option_height;
    let option_bottom = option_top + option_height;
    if option_top < *scroll_offset {
        *scroll_offset = option_top;
    } else if option_bottom > *scroll_offset + viewport_height {
        *scroll_offset = option_bottom - viewport_height;
    }
    *scroll_offset = scroll_offset.clamp(0.0, max_scroll);
}

// ── widget ─────────────────────────────────────────────────────────

/// A dropdown picker widget with a configurable popup menu width.
///
/// # Example
/// ```ignore
/// let picker = DropDown::new(
///     options,
///     selected,
///     |opt| Message::OptionSelected(opt),
/// )
/// .width(Fill)
/// .menu_width(500.0);
/// ```
pub struct DropDown<'a, T, L, V, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    T: ToString + PartialEq + Clone,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Theme: 'a,
    Renderer: text::Renderer + 'a,
{
    options: L,
    selected: Option<V>,
    on_select: Box<dyn Fn(T) -> Message + 'a>,
    placeholder: Option<String>,
    width: Length,
    /// Width of the dropdown popup menu (f32 = fixed pixels).
    menu_width: f32,
    padding: Padding,
    text_size: Option<Pixels>,
    text_line_height: text::LineHeight,
    font: Option<Renderer::Font>,
    style: Box<dyn Fn(&Theme) -> Style + 'a>,
    menu_style: Box<dyn Fn(&Theme) -> MenuStyle + 'a>,
    menu_height: Length,
    on_open: Option<Message>,
    on_close: Option<Message>,
}

/// Internal state stored in the widget tree.
struct State<P: text::Paragraph> {
    is_open: bool,
    hovered_option: Option<usize>,
    /// Pre-laid-out text for the trigger label (selected option or placeholder).
    label: paragraph::Plain<P>,
    /// Pre-laid-out text for each dropdown option.
    option_paragraphs: Vec<paragraph::Plain<P>>,
    /// Persistent scroll offset (pixels from top of content). Stored in the
    /// widget tree so it survives overlay recreation every frame.
    scroll_offset: f32,
    /// Whether the scrollbar thumb is currently being dragged.
    is_dragging_scrollbar: bool,
    /// Y offset between the cursor and the thumb top when a drag started.
    scrollbar_drag_offset: f32,
    /// Set to `true` on open so the overlay scrolls the hovered option into view.
    needs_scroll_to_hovered: bool,
}

impl<P: text::Paragraph> State<P> {
    fn new() -> Self {
        Self {
            is_open: false,
            hovered_option: None,
            label: paragraph::Plain::default(),
            option_paragraphs: Vec::new(),
            scroll_offset: 0.0,
            is_dragging_scrollbar: false,
            scrollbar_drag_offset: 0.0,
            needs_scroll_to_hovered: false,
        }
    }
}

/// Standard button padding used for the trigger (matches iced's button::DEFAULT_PADDING
/// plus extra right space for the dropdown arrow).
fn trigger_padding() -> Padding {
    // Matches `iced_widget::button::DEFAULT_PADDING` (top:5, bottom:5, right:10, left:10),
    // with extra right padding for the "▾" arrow.
    Padding {
        top: 5.0,
        bottom: 5.0,
        right: 28.0,
        left: 10.0,
    }
}

impl<'a, T, L, V, Message, Theme, Renderer> DropDown<'a, T, L, V, Message, Theme, Renderer>
where
    T: ToString + PartialEq + Clone,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: text::Renderer + 'a,
{
    /// Creates a new [`DropDown`] with the given options, selected value, and
    /// a callback that produces a message when an option is chosen.
    pub fn new(options: L, selected: Option<V>, on_select: impl Fn(T) -> Message + 'a) -> Self {
        Self {
            options,
            selected,
            on_select: Box::new(on_select),
            placeholder: None,
            width: Length::Shrink,
            menu_width: 250.0,
            padding: trigger_padding(),
            text_size: None,
            text_line_height: text::LineHeight::default(),
            font: None,
            style: Box::new(|_| Style::default()),
            menu_style: Box::new(|_| MenuStyle::default()),
            menu_height: Length::Shrink,
            on_open: None,
            on_close: None,
        }
    }

    /// Sets the placeholder text shown when no option is selected.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// Sets the width of the trigger button.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Sets the width of the dropdown popup menu in logical pixels.
    pub fn menu_width(mut self, width: f32) -> Self {
        self.menu_width = width;
        self
    }

    /// Sets the padding of the trigger button.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Sets the text size.
    pub fn text_size(mut self, size: impl Into<Pixels>) -> Self {
        self.text_size = Some(size.into());
        self
    }

    /// Sets the text line height.
    pub fn text_line_height(mut self, line_height: impl Into<text::LineHeight>) -> Self {
        self.text_line_height = line_height.into();
        self
    }

    /// Sets the font.
    pub fn font(mut self, font: impl Into<Renderer::Font>) -> Self {
        self.font = Some(font.into());
        self
    }

    /// Sets the style of the trigger button.
    pub fn style(mut self, style: impl Fn(&Theme) -> Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }

    /// Sets the style of the dropdown menu.
    pub fn menu_style(mut self, style: impl Fn(&Theme) -> MenuStyle + 'a) -> Self {
        self.menu_style = Box::new(style);
        self
    }

    /// Sets the max height of the dropdown menu.
    pub fn menu_height(mut self, height: impl Into<Length>) -> Self {
        self.menu_height = height.into();
        self
    }

    /// Sets the message that will be produced when the [`DropDown`] is opened.
    pub fn on_open(mut self, message: Message) -> Self {
        self.on_open = Some(message);
        self
    }

    /// Sets the message that will be produced when the [`DropDown`] is closed.
    pub fn on_close(mut self, message: Message) -> Self {
        self.on_close = Some(message);
        self
    }
}

// ── Widget impl ────────────────────────────────────────────────────

impl<'a, T, L, V, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for DropDown<'a, T, L, V, Message, Theme, Renderer>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]>,
    V: Borrow<T>,
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: text::Renderer + 'a,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Renderer::Paragraph>::new())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: Length::Shrink,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State<Renderer::Paragraph>>();

        let font = self.font.unwrap_or_else(|| renderer.default_font());
        let text_size = self.text_size.unwrap_or_else(|| renderer.default_size());

        // Build the trigger label text. Always show the selected option
        // (or placeholder). The trigger does NOT mirror the hovered item
        // in the open dropdown — the hover only highlights within the menu.
        let label_text = self
            .selected
            .as_ref()
            .map(|v| v.borrow().to_string())
            .unwrap_or_else(|| self.placeholder.clone().unwrap_or_default());
        let _ = state.label.update(Text {
            content: &label_text,
            bounds: Size::new(f32::INFINITY, f32::INFINITY),
            size: text_size,
            line_height: self.text_line_height,
            font,
            align_x: text::Alignment::Default,
            align_y: alignment::Vertical::Center,
            shaping: text::Shaping::default(),
            wrapping: text::Wrapping::default(),
        });

        // Build option paragraphs for the dropdown.
        let options = self.options.borrow();
        state
            .option_paragraphs
            .resize_with(options.len(), Default::default);
        for (option, para) in options.iter().zip(state.option_paragraphs.iter_mut()) {
            let _ = para.update(Text {
                content: &option.to_string(),
                bounds: Size::new(f32::INFINITY, f32::INFINITY),
                size: text_size,
                line_height: self.text_line_height,
                font,
                align_x: text::Alignment::Default,
                align_y: alignment::Vertical::Center,
                shaping: text::Shaping::default(),
                wrapping: text::Wrapping::None,
            });
        }

        // Calculate intrinsic width from the widest option.
        let labels_width = state
            .option_paragraphs
            .iter()
            .fold(state.label.min_width(), |w, p| f32::max(w, p.min_width()));

        let size = {
            // Match pick_list layout: intrinsic height omits vertical padding;
            // .shrink()/.expand() adds it back.
            let intrinsic = Size::new(
                labels_width + self.padding.left + self.padding.right,
                f32::from(self.text_line_height.to_absolute(text_size)),
            );

            limits
                .width(self.width)
                .shrink(self.padding)
                .resolve(self.width, Length::Shrink, intrinsic)
                .expand(self.padding)
        };

        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State<Renderer::Paragraph>>();

        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if cursor.is_over(layout.bounds()) {
                // Toggle open/close when clicking the trigger.
                if state.is_open {
                    state.is_open = false;
                    state.is_dragging_scrollbar = false;
                    if let Some(on_close) = &self.on_close {
                        shell.publish(on_close.clone());
                    }
                } else {
                    state.is_open = true;
                    state.hovered_option = self
                        .selected
                        .as_ref()
                        .and_then(|v| self.options.borrow().iter().position(|o| o == v.borrow()));
                    state.scroll_offset = 0.0;
                    state.needs_scroll_to_hovered = true;
                    if let Some(on_open) = &self.on_open {
                        shell.publish(on_open.clone());
                    }
                }
                shell.capture_event();
            } else if state.is_open {
                // Click outside both the trigger and the overlay -> close.
                state.is_open = false;
                state.is_dragging_scrollbar = false;
                if let Some(on_close) = &self.on_close {
                    shell.publish(on_close.clone());
                }
                shell.capture_event();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let style = (self.style)(theme);
        let bounds = layout.bounds();

        // Background
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: style.border,
                ..renderer::Quad::default()
            },
            style.background,
        );

        // The text is rendered inside a clip layer so it does not overflow
        // into the arrow indicator area. The anchor is offset by padding.left
        // so the text aligns with the padded content area.
        let text_color = if self.selected.is_some() {
            style.text_color
        } else {
            style.placeholder_color
        };

        let text_clip = Rectangle {
            x: bounds.x + self.padding.left,
            y: bounds.y,
            width: bounds.width - self.padding.left - self.padding.right,
            height: bounds.height,
        };
        let anchor = Point::new(
            bounds.x + self.padding.left,
            bounds.y + (bounds.height - state.label.min_bounds().height) / 2.0,
        );
        renderer.with_layer(text_clip, |renderer| {
            renderer.fill_paragraph(state.label.raw(), anchor, text_color, *viewport);
        });

        // Dropdown arrow indicator (▼) — use the same icon font and
        // rendering style as iced's built-in PickList widget.
        let arrow_size = self.text_size.unwrap_or_else(|| renderer.default_size());
        let arrow_line_height = self.text_line_height;
        renderer.fill_text(
            Text {
                content: Renderer::ARROW_DOWN_ICON.to_string(),
                bounds: Size::new(20.0, f32::from(arrow_line_height.to_absolute(arrow_size))),
                size: arrow_size,
                line_height: arrow_line_height,
                font: Renderer::ICON_FONT,
                align_x: text::Alignment::Right,
                align_y: alignment::Vertical::Center,
                shaping: text::Shaping::Basic,
                wrapping: text::Wrapping::default(),
            },
            Point::new(bounds.x + bounds.width - 10.0, bounds.center_y()),
            style.handle_color,
            *viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let state = tree.state.downcast_mut::<State<Renderer::Paragraph>>();

        if state.is_open {
            let bounds = layout.bounds();
            let position = layout.position() + translation;

            let font = self.font.unwrap_or_else(|| renderer.default_font());
            let text_size = self.text_size.unwrap_or_else(|| renderer.default_size());

            let on_select = &self.on_select;

            Some(overlay::Element::new(Box::new(Overlay {
                position,
                target_height: bounds.height,
                menu_width: self.menu_width.max(bounds.width),
                padding: self.padding,
                text_size,
                text_line_height: self.text_line_height,
                font,
                menu_height: self.menu_height,
                menu_style_fn: &self.menu_style,
                options: self.options.borrow(),
                hovered_option: &mut state.hovered_option,
                on_select: Box::new(on_select),
                on_close: self.on_close.as_ref(),
                is_open: &mut state.is_open,
                scroll_offset: &mut state.scroll_offset,
                is_dragging_scrollbar: &mut state.is_dragging_scrollbar,
                scrollbar_drag_offset: &mut state.scrollbar_drag_offset,
                needs_scroll_to_hovered: &mut state.needs_scroll_to_hovered,
            })))
        } else {
            None
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

// ── Overlay ────────────────────────────────────────────────────────

struct Overlay<'a, T, Message, Theme, Renderer>
where
    Renderer: text::Renderer + 'a,
{
    position: Point,
    target_height: f32,
    menu_width: f32,
    padding: Padding,
    text_size: Pixels,
    text_line_height: text::LineHeight,
    font: Renderer::Font,
    menu_height: Length,
    menu_style_fn: &'a dyn Fn(&Theme) -> MenuStyle,
    options: &'a [T],
    hovered_option: &'a mut Option<usize>,
    on_select: Box<dyn Fn(T) -> Message + 'a>,
    on_close: Option<&'a Message>,
    is_open: &'a mut bool,
    /// Persistent scroll offset, stored in widget `State`.
    scroll_offset: &'a mut f32,
    /// Whether the scrollbar thumb is currently being dragged.
    is_dragging_scrollbar: &'a mut bool,
    /// Cursor-to-thumb-top offset captured at drag start.
    scrollbar_drag_offset: &'a mut f32,
    /// Triggers scroll-to-hovered on the next layout pass.
    needs_scroll_to_hovered: &'a mut bool,
}

impl<'a, T, Message, Theme, Renderer> Overlay<'a, T, Message, Theme, Renderer>
where
    Renderer: text::Renderer + 'a,
{
    /// Height of a single option row (text line height + vertical padding).
    fn option_height(&self) -> f32 {
        f32::from(self.text_line_height.to_absolute(self.text_size)) + self.padding.y()
    }

    /// Total content height of all options.
    fn total_height(&self) -> f32 {
        self.option_height() * self.options.len() as f32
    }
}

impl<'a, T, Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for Overlay<'a, T, Message, Theme, Renderer>
where
    T: ToString + Clone + 'a,
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: text::Renderer + 'a,
{
    fn layout(&mut self, _renderer: &Renderer, bounds: Size) -> layout::Node {
        let option_height = self.option_height();
        let total_height = option_height * self.options.len() as f32;

        let space_below = bounds.height - (self.position.y + self.target_height);
        let space_above = self.position.y;
        let max_available = space_below.max(space_above);

        let menu_height = match self.menu_height {
            Length::Shrink => total_height.min(max_available),
            Length::Fixed(h) => h.min(total_height).min(max_available),
            Length::Fill | Length::FillPortion(_) => bounds.height.min(total_height),
        };

        let max_scroll = (total_height - menu_height).max(0.0);
        // Clamp stale scroll offset (e.g. options were removed since last frame).
        *self.scroll_offset = self.scroll_offset.min(max_scroll);

        // Scroll the hovered option into view on open (or when requested).
        if *self.needs_scroll_to_hovered {
            *self.needs_scroll_to_hovered = false;
            if let Some(idx) = *self.hovered_option {
                scroll_to_option(
                    self.scroll_offset,
                    idx,
                    option_height,
                    menu_height,
                    max_scroll,
                );
            }
        }

        let node = layout::Node::new(Size::new(self.menu_width, menu_height));

        if space_below > space_above {
            node.move_to(self.position + Vector::new(0.0, self.target_height))
        } else {
            node.move_to(self.position - Vector::new(0.0, menu_height))
        }
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();
        let option_height = self.option_height();
        let total_height = self.total_height();
        let max_scroll = (total_height - bounds.height).max(0.0);

        let sb = scrollbar_geometry(bounds, total_height, *self.scroll_offset);

        match event {
            // ── Mouse press ──────────────────────────────────────
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Priority 1: if pressing on the scrollbar thumb, start drag.
                if let (Some(sbg), Some(pos)) = (sb.as_ref(), cursor.position()) {
                    if sbg.thumb.contains(Point::new(pos.x, pos.y)) {
                        *self.is_dragging_scrollbar = true;
                        *self.scrollbar_drag_offset = pos.y - sbg.thumb.y;
                        shell.capture_event();
                        return;
                    }
                    // Priority 2: pressing on the track (not thumb) → page scroll.
                    if sbg.track.contains(Point::new(pos.x, pos.y)) {
                        let page = bounds.height;
                        if pos.y < sbg.thumb.y {
                            *self.scroll_offset = (*self.scroll_offset - page).max(0.0);
                        } else {
                            *self.scroll_offset = (*self.scroll_offset + page).min(max_scroll);
                        }
                        shell.request_redraw();
                        shell.capture_event();
                        return;
                    }
                }
                // Priority 3: clicking on an option selects it.
                if cursor.is_over(bounds)
                    && let Some(idx) = *self.hovered_option
                    && let Some(option) = self.options.get(idx)
                {
                    *self.is_open = false;
                    *self.is_dragging_scrollbar = false;
                    shell.publish((self.on_select)(option.clone()));
                    if let Some(on_close) = self.on_close {
                        shell.publish(on_close.clone());
                    }
                    shell.capture_event();
                }
            }

            // ── Mouse release — end scrollbar drag ───────────────
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if *self.is_dragging_scrollbar {
                    *self.is_dragging_scrollbar = false;
                    shell.capture_event();
                }
            }

            // ── Cursor moved — drag scrollbar or hover option ────
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if *self.is_dragging_scrollbar {
                    if let (Some(sbg), Some(pos)) = (&sb, cursor.position()) {
                        let new_thumb_y = pos.y - *self.scrollbar_drag_offset;
                        let range = sbg.track.height - sbg.thumb.height;
                        if range > 0.0 {
                            let ratio = ((new_thumb_y - sbg.track.y) / range).clamp(0.0, 1.0);
                            *self.scroll_offset = ratio * sbg.max_scroll;
                        }
                    }
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }

                if let Some(pos) = cursor.position_in(bounds) {
                    // `position_in` returns coordinates relative to `bounds`,
                    // so `pos.y` is already relative to the menu top.
                    let idx = ((pos.y + *self.scroll_offset) / option_height) as usize;
                    if idx < self.options.len() && *self.hovered_option != Some(idx) {
                        *self.hovered_option = Some(idx);
                        shell.request_redraw();
                    }
                }
            }

            // ── Mouse wheel scroll ────────────────────────────────
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let delta_y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * option_height,
                    mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                *self.scroll_offset = (*self.scroll_offset - delta_y).clamp(0.0, max_scroll);
                shell.request_redraw();
                shell.capture_event();
            }

            // ── Keyboard navigation ───────────────────────────────
            Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                use keyboard::key::Named;

                let option_count = self.options.len();
                if option_count == 0 {
                    return;
                }

                // Number of options that fit in one viewport (min 1).
                let page_steps = ((bounds.height / option_height) as usize).max(1);

                match key {
                    keyboard::Key::Named(Named::ArrowDown) => {
                        let new_idx = match *self.hovered_option {
                            None => 0,
                            Some(idx) => (idx + 1).min(option_count - 1),
                        };
                        *self.hovered_option = Some(new_idx);
                        scroll_to_option(
                            self.scroll_offset,
                            new_idx,
                            option_height,
                            bounds.height,
                            max_scroll,
                        );
                        shell.request_redraw();
                        shell.capture_event();
                    }
                    keyboard::Key::Named(Named::ArrowUp) => {
                        let new_idx = match *self.hovered_option {
                            None => option_count - 1,
                            Some(idx) => idx.saturating_sub(1),
                        };
                        *self.hovered_option = Some(new_idx);
                        scroll_to_option(
                            self.scroll_offset,
                            new_idx,
                            option_height,
                            bounds.height,
                            max_scroll,
                        );
                        shell.request_redraw();
                        shell.capture_event();
                    }
                    keyboard::Key::Named(Named::PageDown) => {
                        let new_idx = match *self.hovered_option {
                            None => 0,
                            Some(idx) => (idx + page_steps).min(option_count - 1),
                        };
                        *self.hovered_option = Some(new_idx);
                        scroll_to_option(
                            self.scroll_offset,
                            new_idx,
                            option_height,
                            bounds.height,
                            max_scroll,
                        );
                        shell.request_redraw();
                        shell.capture_event();
                    }
                    keyboard::Key::Named(Named::PageUp) => {
                        let new_idx = match *self.hovered_option {
                            None => option_count - 1,
                            Some(idx) => idx.saturating_sub(page_steps),
                        };
                        *self.hovered_option = Some(new_idx);
                        scroll_to_option(
                            self.scroll_offset,
                            new_idx,
                            option_height,
                            bounds.height,
                            max_scroll,
                        );
                        shell.request_redraw();
                        shell.capture_event();
                    }
                    keyboard::Key::Named(Named::Home) => {
                        *self.hovered_option = Some(0);
                        *self.scroll_offset = 0.0;
                        shell.request_redraw();
                        shell.capture_event();
                    }
                    keyboard::Key::Named(Named::End) => {
                        *self.hovered_option = Some(option_count - 1);
                        *self.scroll_offset = max_scroll;
                        shell.request_redraw();
                        shell.capture_event();
                    }
                    keyboard::Key::Named(Named::Enter) => {
                        if let Some(idx) = *self.hovered_option
                            && let Some(option) = self.options.get(idx)
                        {
                            *self.is_open = false;
                            *self.is_dragging_scrollbar = false;
                            shell.publish((self.on_select)(option.clone()));
                            if let Some(on_close) = self.on_close {
                                shell.publish(on_close.clone());
                            }
                            shell.request_redraw();
                            shell.capture_event();
                        }
                    }
                    keyboard::Key::Named(Named::Escape) => {
                        *self.is_open = false;
                        *self.is_dragging_scrollbar = false;
                        if let Some(on_close) = self.on_close {
                            shell.publish(on_close.clone());
                        }
                        shell.request_redraw();
                        shell.capture_event();
                    }
                    _ => {}
                }
            }

            _ => {}
        }
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        let style = (self.menu_style_fn)(theme);

        // Menu background
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: style.border,
                shadow: style.shadow,
                ..renderer::Quad::default()
            },
            style.background,
        );

        let option_height = self.option_height();
        let total_height = self.total_height();

        let scroll_offset = *self.scroll_offset;

        // Only draw visible items (skip items scrolled off-screen).
        let first_visible = (scroll_offset / option_height).floor() as usize;
        let last_visible = ((scroll_offset + bounds.height) / option_height).ceil() as usize;
        let last_visible = last_visible.min(self.options.len());

        // Clip area for option text, excluding the scrollbar strip.
        let text_clip = Rectangle {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width - SB_RESERVED,
            height: bounds.height,
        };

        let cursor_over_menu = cursor.is_over(bounds);
        let cursor_pos = cursor.position();

        for i in first_visible..last_visible {
            let option = &self.options[i];
            let opt_y = bounds.y + option_height * i as f32 - scroll_offset;
            let opt_bounds = Rectangle {
                x: bounds.x,
                y: opt_y,
                width: bounds.width,
                height: option_height,
            };

            let is_hovered = *self.hovered_option == Some(i);

            if is_hovered {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: opt_bounds.x + style.border.width,
                            width: opt_bounds.width - style.border.width * 2.0,
                            ..opt_bounds
                        },
                        border: border::rounded(style.border.radius),
                        ..renderer::Quad::default()
                    },
                    style.selected_background,
                );
            }

            let text_color = if is_hovered {
                style.selected_text_color
            } else {
                style.text_color
            };

            renderer.fill_text(
                Text {
                    content: option.to_string(),
                    // Use a very large width so the text never wraps (the
                    // fill_text pipeline ignores `wrapping`; clipping is
                    // handled by the clip_bounds below).
                    bounds: Size::new(f32::MAX / 2.0, opt_bounds.height),
                    size: self.text_size,
                    line_height: self.text_line_height,
                    font: self.font,
                    align_x: text::Alignment::Default,
                    align_y: alignment::Vertical::Center,
                    shaping: text::Shaping::default(),
                    wrapping: text::Wrapping::None,
                },
                Point::new(
                    opt_bounds.x + self.padding.left,
                    opt_bounds.y + opt_bounds.height * 0.5,
                ),
                text_color,
                text_clip,
            );
        }

        // ── scrollbar ───────────────────────────────────────────
        if let Some(sbg) = scrollbar_geometry(bounds, total_height, scroll_offset) {
            // Determine thumb visual state: highlighted when hovered or dragged.
            let is_thumb_hovered = cursor_over_menu
                && cursor_pos
                    .map(|p| sbg.thumb.contains(Point::new(p.x, p.y)))
                    .unwrap_or(false);
            let is_thumb_active = *self.is_dragging_scrollbar || is_thumb_hovered;

            let track_alpha = if is_thumb_active { 0.12 } else { 0.06 };
            let thumb_alpha = if is_thumb_active { 0.35 } else { 0.2 };

            // Track
            renderer.fill_quad(
                renderer::Quad {
                    bounds: sbg.track,
                    border: border::rounded(SB_WIDTH / 2.0),
                    ..renderer::Quad::default()
                },
                Background::Color(Color::from_rgba(0.0, 0.0, 0.0, track_alpha)),
            );
            // Thumb
            renderer.fill_quad(
                renderer::Quad {
                    bounds: sbg.thumb,
                    border: border::rounded(SB_WIDTH / 2.0),
                    ..renderer::Quad::default()
                },
                Background::Color(Color::from_rgba(0.0, 0.0, 0.0, thumb_alpha)),
            );
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();

        if *self.is_dragging_scrollbar {
            return mouse::Interaction::Grabbing;
        }

        // Check if hovering the scrollbar thumb.
        let total_height = self.total_height();

        if let (Some(sbg), Some(pos)) = (
            scrollbar_geometry(bounds, total_height, *self.scroll_offset),
            cursor.position(),
        ) {
            if sbg.thumb.contains(Point::new(pos.x, pos.y)) {
                return mouse::Interaction::Grab;
            }
            if sbg.track.contains(Point::new(pos.x, pos.y)) {
                return mouse::Interaction::Pointer;
            }
        }

        if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

// ── Element conversion ─────────────────────────────────────────────

impl<'a, T, L, V, Message, Theme, Renderer> From<DropDown<'a, T, L, V, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: text::Renderer + 'a,
{
    fn from(dropdown: DropDown<'a, T, L, V, Message, Theme, Renderer>) -> Self {
        Element::new(dropdown)
    }
}
