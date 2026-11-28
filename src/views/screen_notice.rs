//! The frame window shown by the screen notice before `send_input` acts.

use iced::widget::{Space, container, stack, text};
use iced::{Border, Element, Fill, Widget as _};

use crabot::i18n::Lang;
use crabot::tools::notice::{BORDER, Frame, LABEL};

use crate::app::Message;
use crate::views::theme::{color_accent, color_card, color_text_strong};

/// Draw the target border and optional announcement.
pub(crate) fn frame(frame: Frame, lang: Lang) -> Element<'static, Message> {
    let accent = color_accent();
    // Windows/X11 fill a clipped ring; macOS draws a transparent border.
    let clipped = !cfg!(target_os = "macos");
    let border = container(Space::new())
        .width(Fill)
        .height(Fill)
        .style(move |_| container::Style {
            background: clipped.then(|| accent.into()),
            border: Border {
                width: BORDER,
                radius: 0.0.into(),
                color: accent,
            },
            ..container::Style::default()
        });
    if !frame.announce {
        return border.boxed();
    }
    // Match the clipped label rectangle exactly.
    let label = frame.label();
    let capsule = container(text(lang.tr(LABEL)).size(17).color(color_text_strong()))
        .width(label.width)
        .height(label.height)
        .padding([0.0, 16.0])
        .align_x(iced::Center)
        .align_y(iced::Center)
        .style(move |_| container::Style {
            background: Some(color_card().into()),
            border: Border {
                width: 2.0,
                radius: 10.0.into(),
                color: accent,
            },
            ..container::Style::default()
        });
    stack![
        border,
        container(capsule)
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .center_y(Fill),
    ]
    .boxed()
}
