use iced::{
    Alignment, Element, Fill, Length, Padding,
    widget::{
        Space, checkbox, column, container, mouse_area, pick_list, row, scrollable, text,
        text_editor, text_input,
    },
};

use crate::FocusedTarget;
use crate::Message;
use crate::system::{DATE, FilepathEntry, PREAMBLE, RULES, TOOLS, WORKSPACE, WORKSPACE_TREE};
use crate::widgets::textarea::TextArea;

use std::path::PathBuf;

// ── internal helper ──────────────────────────────────────────────────

fn expandable_header<'a>(
    name: &'static str,
    checked: bool,
    expanded: bool,
) -> Element<'a, Message> {
    let arrow = if expanded { "▼" } else { "⯈" };
    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        Space::new().width(Length::Fill),
        mouse_area(text(arrow).size(12).width(16)).on_press(Message::ToggleExpanded(name)),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

// ── field views ──────────────────────────────────────────────────────

pub(crate) fn preamble_field_view<'a>(
    field: &'a (bool, String),
    options: &'a [FilepathEntry],
    selected_display: &'a str,
) -> Element<'a, Message> {
    let checked = field.0;
    let name = PREAMBLE;
    let selected = if selected_display.is_empty() {
        None
    } else {
        options
            .iter()
            .find(|e| e.display == selected_display)
            .cloned()
    };

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        pick_list(options, selected, Message::SelectPreamble).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub(crate) fn rules_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a TextArea,
) -> Element<'a, Message> {
    let header = expandable_header(RULES, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                content
                    .view(|msg| Message::EditTextArea(FocusedTarget::EditText(RULES), msg))
                    .height(Length::Fixed(120.0)),
            ),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub(crate) fn tools_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let name = TOOLS;
    let header = expandable_header(name, field.0, expanded);

    if expanded {
        column![
            header,
            scrollable(
                text_editor(content)
                    .on_action(move |a| Message::EditTextContent(name, a))
                    .height(Length::Fixed(120.0)),
            ),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub(crate) fn workspace_field_view<'a>(
    field: &'a (bool, PathBuf),
    options: &'a [FilepathEntry],
) -> Element<'a, Message> {
    let checked = field.0;
    let name = WORKSPACE;
    let selected = if field.1.as_os_str().is_empty() {
        None
    } else {
        options.iter().find(|e| e.path == field.1).cloned()
    };

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        pick_list(options, selected, Message::SelectWorkspace).width(Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

pub(crate) fn files_field_view<'a>(
    expanded: bool,
    field: &'a (bool, String),
    content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let name = WORKSPACE_TREE;
    let header = expandable_header(name, field.0, expanded);

    use iced::widget::scrollable::Direction;
    use iced::widget::text::Wrapping;

    if expanded {
        column![
            header,
            container(
                scrollable(
                    container(
                        text_editor(content)
                            .on_action(move |a| Message::EditTextContent(name, a))
                            .font(iced::Font::MONOSPACE)
                            .wrapping(Wrapping::None),
                    )
                    .padding(Padding::new(0.0).bottom(12.0)),
                )
                .direction(Direction::Both {
                    vertical: Default::default(),
                    horizontal: Default::default(),
                })
                .height(Length::Fixed(200.0)),
            )
            .style(container::bordered_box)
            .width(Fill),
        ]
        .spacing(4)
        .into()
    } else {
        header
    }
}

pub(crate) fn date_field_view<'a>(field: &'a (bool, String)) -> Element<'a, Message> {
    let checked = field.0;
    let name = DATE;

    row![
        checkbox(checked)
            .label(name)
            .style(crate::views::primary_checkbox)
            .on_toggle(move |v| Message::ToggleEnabled(name, v)),
        text_input("YYYY-MM-DD", &field.1)
            .on_input(move |s| Message::EditTextField(name, s))
            .width(Length::Fixed(110.0))
            .padding(4)
            .align_x(iced::alignment::Horizontal::Center),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}
