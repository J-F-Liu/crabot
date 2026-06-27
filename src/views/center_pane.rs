use iced::widget;
use iced::{
    Background, Border, Color, Element, Fill, Font, Length, Padding, Task, Theme,
    advanced::text::Highlight,
    alignment, font,
    widget::{Space, button, column, container, markdown, mouse_area, row, scrollable, text},
};
use iced_selection::Text as SelectableText;

use super::styles::{
    assistant_bubble_style, icon_button_style, pane_center, role_badge_style, sel_default,
    sel_secondary, tool_bubble_style, user_bubble_style,
};
use super::theme::{
    CRABOT_BORDER, CRABOT_PRIMARY, CRABOT_TEXT, CRABOT_TEXT_MUTED, CRABOT_TOOL_ACCENT, color_text,
};
use super::tool_message::{args_rows, path_arg_row, result_text};
use crate::Message;
use crate::chat::{Dialog, TextContent, ToolResult, Turn, TurnBody};
use crate::llm::StreamState;

pub(crate) const MESSAGE_SCROLL: widget::Id = widget::Id::new("messages");

/// Snap the message scroll to the end unconditionally.
pub(crate) fn scroll_to_end() -> Task<Message> {
    iced_runtime::task::widget(iced::advanced::widget::operation::scrollable::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::END.into(),
    ))
}

// ── dialog styles ─────────────────────────────────────────────────

const DIALOG_BG: Color = Color::WHITE;
const DIALOG_RADIUS: f32 = 10.0;

fn dialog_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(DIALOG_BG.into()),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: DIALOG_RADIUS.into(),
        },
        ..container::Style::default()
    }
}

fn bordered_bar_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        border: Border {
            color: CRABOT_BORDER,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// Small turn-count pill.
fn turn_count_badge(count: usize) -> Element<'static, Message> {
    container(
        text(format!(
            "{} turn{}",
            count,
            if count == 1 { "" } else { "s" }
        ))
        .size(10),
    )
    .padding([2, 8])
    .style(|_theme: &Theme| container::Style {
        background: Some(Color::from_rgb8(0xE0, 0xE0, 0xE0).into()),
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        text_color: Some(CRABOT_TEXT_MUTED),
        ..container::Style::default()
    })
    .into()
}

// ── turn block builders ────────────────────────────────────────────

/// Build the colored role badge shown in a turn header.
fn role_badge(badge_text: String, style_label: &'static str) -> Element<'static, Message> {
    container(text(badge_text).size(11).font(Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .padding([3, 8])
    .style(role_badge_style(style_label))
    .into()
}

/// Wrap a turn's content in its role-colored bubble.
fn wrap_bubble<'a>(
    content: impl Into<Element<'a, Message>>,
    style: fn(&Theme) -> container::Style,
) -> Element<'a, Message> {
    container(content)
        .width(Fill)
        .padding([8, 12])
        .style(style)
        .into()
}

/// Build a complete Tool turn block (header + body + bubble).
fn tool_turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    expanded_turns: &std::collections::HashSet<usize>,
) -> Element<'a, Message> {
    let TurnBody::Tool(ToolResult {
        name, args, result, ..
    }) = &msg.body
    else {
        unreachable!("tool_turn_block called on non-Tool turn")
    };

    let expanded = expanded_turns.contains(&i);
    let indicator = if expanded { "▼" } else { "⏵" };
    let badge = role_badge(format!("Tool - {name}"), "Tool");
    let ts_text = text(&msg.timestamp).size(11).color(CRABOT_TEXT_MUTED);
    let mut content_col = column![].spacing(8).width(Fill);

    // ── header: badge + status icon + timestamp ──
    let (status_icon, status_color) = match result {
        Ok(_) => ("✓", super::theme::CRABOT_SUCCESS),
        Err(_) => ("✗", super::theme::CRABOT_DANGER),
    };
    let tool_header = row![
        badge,
        text(indicator).size(10).color(CRABOT_TOOL_ACCENT),
        Space::new().width(Length::Fill),
        text(status_icon).size(12).color(status_color).font(Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        }),
        ts_text,
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);
    content_col = content_col.push(
        mouse_area(tool_header)
            .on_press(Message::ToggleTurnExpand(i))
            .interaction(iced::mouse::Interaction::Pointer),
    );

    // ── body: args + result ──
    let is_edit_or_write = name == "edit" || name == "write";
    if expanded {
        for r in args_rows(args) {
            content_col = content_col.push(r);
        }
        content_col = content_col.push(result_text(result));
    } else if is_edit_or_write {
        if let Some(row) = path_arg_row(args) {
            content_col = content_col.push(row);
        }
    } else {
        for r in args_rows(args) {
            content_col = content_col.push(r);
        }
    }

    wrap_bubble(content_col, tool_bubble_style)
}

/// Build a complete Text turn block (header + body + bubble).
fn text_turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    expanded_turns: &std::collections::HashSet<usize>,
    selectable_msgs: &std::collections::HashSet<usize>,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let TurnBody::Text(TextContent { content, reasoning }) = &msg.body else {
        unreachable!("text_turn_block called on non-Text turn")
    };

    let (role_label, bubble_style): (&'static str, fn(&Theme) -> container::Style) = match msg.role {
        genai::chat::ChatRole::User => ("User", user_bubble_style),
        genai::chat::ChatRole::Assistant => ("Assistant", assistant_bubble_style),
        _ => ("System", assistant_bubble_style),
    };
    let badge = role_badge(role_label.to_string(), role_label);
    let ts_text = text(&msg.timestamp).size(11).color(CRABOT_TEXT_MUTED);
    let mut content_col = column![].spacing(8).width(Fill);

    // ── header: badge + (indicator if reasoning) + timestamp ──
    if reasoning.is_some() {
        // Reasoning by default is expanded so inverse membership.
        let expanded = !expanded_turns.contains(&i);
        let indicator = if expanded { "▼" } else { "⏵" };
        let header = row![
            badge,
            text(indicator).size(10).color(CRABOT_PRIMARY),
            Space::new().width(Length::Fill),
            ts_text,
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        content_col = content_col.push(
            mouse_area(header)
                .on_press(Message::ToggleTurnExpand(i))
                .interaction(iced::mouse::Interaction::Pointer),
        );
    } else {
        let header =
            row![badge, Space::new().width(Length::Fill), ts_text].align_y(iced::Alignment::Center);
        content_col = content_col.push(header);
    }

    // ── body: reasoning + content ──
    if let Some(reasoning) = reasoning {
        // Default expanded; badge-row click toggles collapse.
        if !expanded_turns.contains(&i) {
            content_col =
                content_col.push(SelectableText::new(reasoning).size(13).style(sel_secondary));
        }
    }
    if selectable_msgs.contains(&i) {
        content_col = content_col.push(SelectableText::new(content).size(14).style(sel_default));
    } else if let Some(md) = &msg.content_md {
        let mut md_style = markdown::Style::from(theme.clone());
        md_style.inline_code_highlight = Highlight {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
        };
        md_style.inline_code_padding = 0.into();
        md_style.inline_code_color = color_text(theme);
        content_col = content_col.push(
            mouse_area(
                markdown::view(md.items(), markdown::Settings::with_text_size(14, md_style))
                    .map(|_| Message::Noop),
            )
            .on_double_click(Message::ToggleSelectableMode(Some(i))),
        );
    } else {
        content_col = content_col.push(SelectableText::new(content).size(14).style(sel_default));
    }

    wrap_bubble(content_col, bubble_style)
}

/// Build a single turn block (header + body) wrapped in its role-colored bubble.
fn turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    expanded_turns: &'a std::collections::HashSet<usize>,
    selectable_msgs: &std::collections::HashSet<usize>,
    theme: &'a Theme,
) -> Element<'a, Message> {
    match &msg.body {
        TurnBody::Tool(_) => tool_turn_block(msg, i, expanded_turns),
        TurnBody::Text(_) => text_turn_block(msg, i, expanded_turns, selectable_msgs, theme),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn center_pane<'a>(
    current_prompt: &'a str,
    dialogs: &'a [Dialog],
    expanded_turns: &'a std::collections::HashSet<usize>,
    expanded_dialogs: &'a std::collections::HashSet<usize>,
    status: &'a str,
    theme: &'a Theme,
    streaming: StreamState,
    selectable_msgs: &std::collections::HashSet<usize>,
) -> Element<'a, Message> {
    // Flatten dialogs into turns with a running flat index per dialog.
    let mut flat_idx: usize = 0;
    let dialog_blocks: Vec<Element<'_, Message>> = dialogs
        .iter()
        .enumerate()
        .map(|(di, dialog)| {
            let collapsed = !expanded_dialogs.contains(&di);
            let indicator = if collapsed { "⊞" } else { "⊟" };
            let title = if dialog.title.is_empty() {
                format!("Dialog {}", di + 1)
            } else {
                dialog.title.clone()
            };
            let turn_count = dialog.turns.len();

            // ── clickable header ──────────────────────────────────
            let title_row = row![
                text(indicator).size(10).color(CRABOT_PRIMARY),
                text(title).size(13).font(Font {
                    weight: font::Weight::Bold,
                    ..Font::DEFAULT
                }),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);

            let mut header_row = row![title_row]
                .spacing(10)
                .align_y(iced::Alignment::Center)
                .width(Fill);
            if collapsed && turn_count > 0 {
                header_row = header_row.push(Space::new().width(Length::Fill));
                header_row = header_row.push(turn_count_badge(turn_count));
            }

            let header = mouse_area(
                container(header_row)
                    .width(Fill)
                    .padding([8, 12]),
            )
            .on_press(Message::ToggleDialogExpand(di))
            .interaction(iced::mouse::Interaction::Pointer);

            // ── turn blocks (only built when expanded) ────────────
            let turn_blocks: Vec<Element<'_, Message>> = if collapsed {
                flat_idx += dialog.turns.len();
                Vec::new()
            } else {
                dialog
                    .turns
                    .iter()
                    .map(|msg| {
                        let i = flat_idx;
                        flat_idx += 1;
                        turn_block(msg, i, expanded_turns, selectable_msgs, theme)
                    })
                    .collect()
            };

            // ── assemble dialog container ──────────────────────────
            let mut content = column![header];
            if !turn_blocks.is_empty() {
                content = content.push(
                    container(column(turn_blocks).spacing(8))
                        .padding(Padding::new(10.0).top(8.0))
                        .width(Fill),
                );
            }
            container(content.spacing(0).width(Fill))
                .style(dialog_container_style)
                .clip(true)
                .into()
        })
        .collect();

    container(column![
        session_header(current_prompt),
        scrollable(column(dialog_blocks).spacing(18).padding(14),)
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
            .padding(6)
            .style(icon_button_style),
        button(text("↻").size(14))
            .on_press(Message::ResendLastPrompt)
            .padding(6)
            .style(icon_button_style),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    container(header)
        .width(Fill)
        .padding([10, 14])
        .style(bordered_bar_style)
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
                .padding([4, 10])
                .style(icon_button_style),
        );
    }
    container(row)
        .width(Fill)
        .align_x(alignment::Horizontal::Center)
        .padding([6, 12])
        .style(bordered_bar_style)
        .into()
}
