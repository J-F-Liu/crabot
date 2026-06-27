use iced::widget;
use iced::{
    Background, Border, Color, Element, Fill, Font, Length, Task, Theme,
    advanced::text::Highlight,
    alignment, font,
    widget::{Space, button, column, container, markdown, mouse_area, row, scrollable, text},
};
use iced_selection::Text as SelectableText;

use super::styles::{icon_button_style, pane_center, sel_default, sel_secondary};
use super::theme::{CRABOT_SURFACE, CRABOT_TEXT, CRABOT_TEXT_MUTED, color_text};
use super::tool_message::{args_rows, path_arg_row, result_text};
use crate::Message;
use crate::chat::{Dialog, TextContent, ToolResult, TurnBody};
use crate::llm::StreamState;

pub(crate) const MESSAGE_SCROLL: widget::Id = widget::Id::new("messages");

/// Snap the message scroll to the end unconditionally.
pub(crate) fn scroll_to_end() -> Task<Message> {
    iced_runtime::task::widget(iced::advanced::widget::operation::scrollable::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::END.into(),
    ))
}

pub(crate) fn center_pane<'a>(
    current_prompt: &'a str,
    dialogs: &'a [Dialog],
    expanded_tools: &'a std::collections::HashSet<usize>,
    status: &'a str,
    theme: &'a Theme,
    streaming: StreamState,
    selectable_msgs: &std::collections::HashSet<usize>,
) -> Element<'a, Message> {
    // Flatten dialogs into turns with a running flat index.
    let mut flat_idx: usize = 0;
    let dialog_blocks: Vec<Element<'_, Message>> = dialogs
        .iter()
        .map(|dialog| {
            let title_row: Option<Element<'_, Message>> = if dialog.title.is_empty() {
                None
            } else {
                Some(
                    container(text(&dialog.title).size(13).font(Font {
                        weight: font::Weight::Bold,
                        ..Font::DEFAULT
                    }))
                    .padding([4, 8])
                    .style(|_theme: &Theme| container::Style {
                        background: Some(CRABOT_SURFACE.into()),
                        ..container::Style::default()
                    })
                    .into(),
                )
            };
            let turn_blocks: Vec<Element<'_, Message>> = dialog
                .turns
                .iter()
                .map(|msg| {
                    let i = flat_idx;
                    flat_idx += 1;
                    container({
                        let is_tool = matches!(&msg.body, TurnBody::Tool(_));
                        let expanded = is_tool && expanded_tools.contains(&i);
                        let indicator = if is_tool {
                            if expanded { "▼" } else { "▶" }
                        } else {
                            ""
                        };
                        let (header, is_edit_or_write, _) = match &msg.body {
                            TurnBody::Tool(ToolResult { name, result, .. }) => {
                                let status_icon = match result {
                                    Ok(_) => " ✓",
                                    Err(_) => " ✗",
                                };
                                let hdr =
                                    format!("{} {} — {}{}", indicator, msg.role, name, status_icon);
                                let is_ew = name == "edit" || name == "write";
                                (hdr, is_ew, name.as_str())
                            }
                            _ => (msg.role.to_string(), false, ""),
                        };
                        let header_text = text(header).size(13).color(CRABOT_TEXT);
                        let ts_text = SelectableText::new(&msg.timestamp)
                            .size(11)
                            .style(sel_secondary);
                        let mut col = if is_tool {
                            let header_row =
                                row![header_text, Space::new().width(Length::Fill), ts_text,];
                            column![
                                mouse_area(header_row)
                                    .on_press(Message::ToggleToolExpand(i))
                                    .interaction(iced::mouse::Interaction::Pointer),
                            ]
                        } else {
                            column![row![header_text, Space::new().width(Length::Fill), ts_text,],]
                        };
                        match &msg.body {
                            TurnBody::Text(TextContent { content, reasoning }) => {
                                if let Some(reasoning) = reasoning {
                                    col = col.push(
                                        SelectableText::new(reasoning)
                                            .size(13)
                                            .style(sel_secondary),
                                    );
                                }
                                if selectable_msgs.contains(&i) {
                                    col = col.push(
                                        SelectableText::new(content).size(14).style(sel_default),
                                    );
                                } else if let Some(md) = &msg.content_md {
                                    let mut md_style = markdown::Style::from(theme.clone());
                                    md_style.inline_code_highlight = Highlight {
                                        background: Background::Color(Color::TRANSPARENT),
                                        border: Border::default(),
                                    };
                                    md_style.inline_code_padding = 0.into();
                                    md_style.inline_code_color = color_text(theme);
                                    col = col.push(
                                        mouse_area(
                                            markdown::view(
                                                md.items(),
                                                markdown::Settings::with_text_size(14, md_style),
                                            )
                                            .map(|_| Message::Noop),
                                        )
                                        .on_double_click(Message::ToggleSelectableMode(Some(i))),
                                    );
                                } else {
                                    col = col.push(
                                        SelectableText::new(content).size(14).style(sel_default),
                                    );
                                }
                            }
                            TurnBody::Tool(ToolResult { args, result, .. }) => {
                                if is_edit_or_write {
                                    if expanded {
                                        col = col.extend(args_rows(args));
                                        col = col.push(result_text(result));
                                    } else if let Some(row) = path_arg_row(args) {
                                        col = col.push(row);
                                    }
                                } else {
                                    col = col.extend(args_rows(args));
                                    if expanded {
                                        col = col.push(result_text(result));
                                    }
                                }
                            }
                        }
                        col.spacing(4).width(Fill)
                    })
                    .width(Fill)
                    .padding(8)
                    .style(|_theme: &Theme| container::Style::default())
                    .into()
                })
                .collect();
            let mut group = column(turn_blocks).spacing(8);
            if let Some(title) = title_row {
                group = column![title, group].spacing(4);
            }
            container(group)
                .style(|_theme: &Theme| container::Style::default())
                .into()
        })
        .collect();

    container(column![
        session_header(current_prompt),
        scrollable(column(dialog_blocks).spacing(16).padding(10),)
            .height(Fill)
            .id(MESSAGE_SCROLL.clone())
            .on_scroll(Message::MessageViewScrolled),
        status_line(status, streaming),
    ])
    .width(Fill)
    .height(Fill)
    .style(pane_center)
    .into()
}

// ── session header ──────────────────────────────────────────────────

/// Header bar at the top of the center pane: prompt text or "New session",
/// plus copy-to-clipboard and resend action icons on the far right.
fn session_header<'a>(prompt: &'a str) -> Element<'a, Message> {
    use iced_selection::text::Style as SelectionStyle;

    let header = row![
        container(SelectableText::new(prompt).size(14).style(|theme: &Theme| {
            let p = theme.extended_palette();
            SelectionStyle {
                color: Some(CRABOT_TEXT),
                selection: p.primary.base.color,
            }
        }),)
        .width(Length::Fill)
        .clip(true),
        button(text("▣").size(14))
            .on_press(Message::CopySession)
            .padding(4)
            .style(icon_button_style),
        button(text("↻").size(14))
            .on_press(Message::ResendLastPrompt)
            .padding(4)
            .style(icon_button_style),
    ];

    container(header)
        .width(Fill)
        .padding([8, 12])
        .style(|_theme: &Theme| container::Style {
            background: Some(CRABOT_SURFACE.into()),
            ..container::Style::default()
        })
        .into()
}

// ── status line ───────────────────────────────────────────────────

fn status_line<'a>(status_text: &'a str, streaming: StreamState) -> Element<'a, Message> {
    let mut row = row![text(status_text).size(12).color(CRABOT_TEXT_MUTED),]
        .align_y(iced::Alignment::Center)
        .spacing(8);
    if streaming != StreamState::Idle {
        row = row.push(
            button(text("⏹ Stop").size(11))
                .on_press(Message::StopStream)
                .padding([2, 8])
                .style(icon_button_style),
        );
    }
    container(row)
        .width(Fill)
        .align_x(alignment::Horizontal::Center)
        .padding([4, 10])
        .style(|_theme: &Theme| container::Style {
            background: Some(CRABOT_SURFACE.into()),
            ..container::Style::default()
        })
        .into()
}
