//! A compact local tab bar, replacing `iced_aw`'s `TabBar`.

use iced::{
    Alignment, Background, Border, Element, Length, Theme, Widget,
    border::Radius,
    widget::scrollable::{Direction, Scrollbar},
    widget::{button, row, scrollable, text},
};

use super::theme::{
    color_accent, color_accent_hover, color_border, color_on_accent, color_surface,
    color_text_strong,
};

const TAB_RADIUS: Radius = Radius {
    top_left: 6.0,
    top_right: 6.0,
    bottom_right: 0.0,
    bottom_left: 0.0,
};

/// Top-rounded tab button: accent when active, accent-hover on hover, surface otherwise.
fn tab_style(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let background = if active {
            color_accent()
        } else if hovered {
            color_accent_hover()
        } else {
            color_surface()
        };
        let text_color = if active || hovered {
            color_on_accent()
        } else {
            color_text_strong()
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color,
            border: Border {
                color: color_border(),
                width: 1.0,
                radius: TAB_RADIUS,
            },
            ..button::Style::default()
        }
    }
}

/// Build a row of tabs from `(label, active, message)` triples.
pub(crate) fn tab_bar<'a, M: Clone + 'a>(
    tabs: impl IntoIterator<Item = (String, bool, M)>,
) -> Element<'a, M> {
    row(tabs.into_iter().map(|(label, active, on_press)| {
        button(text(label).size(13))
            .on_press(on_press)
            .padding([4, 10])
            .width(Length::Shrink)
            .style(tab_style(active))
    }))
    .spacing(2)
    .align_y(Alignment::Center)
    .boxed()
}

/// Like [`tab_bar`], but scrolls horizontally so many tabs remain reachable.
pub(crate) fn scrollable_tab_bar<'a, M: Clone + 'a>(
    tabs: impl IntoIterator<Item = (String, bool, M)>,
) -> Element<'a, M> {
    scrollable(tab_bar(tabs))
        .direction(Direction::Horizontal(Scrollbar::hidden()))
        .width(Length::Fill)
        .height(Length::Shrink)
        .boxed()
}
