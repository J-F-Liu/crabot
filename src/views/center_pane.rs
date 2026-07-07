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
    CRABOT_BORDER, CRABOT_DIALOG_BG, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY, CRABOT_TEXT,
    CRABOT_TEXT_MUTED, CRABOT_TOOL_ACCENT, color_text, thin_vertical,
};
use super::tool_message::{args_rows, path_arg_row, result_text};
use crate::Message;
use crate::chat::{Dialog, TextContent, Turn, TurnBody};
use crate::llm::StreamState;
use std::collections::HashSet;

pub(crate) const MESSAGE_SCROLL: widget::Id = widget::Id::new("messages");

/// Snap the message scroll to the end unconditionally.
pub(crate) fn scroll_to_end() -> Task<Message> {
    iced_runtime::task::widget(iced::advanced::widget::operation::scrollable::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::END.into(),
    ))
}

// ── dialog styles ─────────────────────────────────────────────────

fn dialog_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(CRABOT_DIALOG_BG.into()),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: CRABOT_DIALOG_RADIUS.into(),
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
fn turn_count_badge(count: usize, font_scale: f32) -> Element<'static, Message> {
    container(
        text(format!(
            "{} turn{}",
            count,
            if count == 1 { "" } else { "s" }
        ))
        .size(10.0 * font_scale)
        .center(),
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
fn role_badge(
    badge_text: String,
    style_label: &'static str,
    font_scale: f32,
) -> Element<'static, Message> {
    container(text(badge_text).size(12.0 * font_scale).font(Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .padding([3, 0])
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

/// Collapsed args preview: just the path for edit/write, all args otherwise.
fn args_preview<'a>(
    name: &str,
    args: &'a serde_json::Value,
    font_scale: f32,
) -> Vec<Element<'a, Message>> {
    if name == "edit" || name == "write" {
        path_arg_row(args, font_scale).into_iter().collect()
    } else {
        args_rows(args, font_scale)
    }
}

/// Build a Tool turn block — handles both completed (`Tool`) and pending (`Temp`) calls.
/// Multiple tool calls from one LLM response are grouped into a single turn
/// and rendered as stacked sub-items within the same bubble.
fn tool_turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    expanded_turns: &HashSet<(usize, usize)>,
    font_scale: f32,
) -> Element<'a, Message> {
    // Build a unified list of (name, args, result_opt, timestamp) from either variant.
    type ToolItem<'a> = (
        &'a str,
        &'a serde_json::Value,
        Option<&'a Result<String, String>>,
        &'a str,
    );
    let items: Vec<ToolItem<'a>> = match &msg.body {
        TurnBody::Tool(trs) => {
            if trs.is_empty() {
                // No results yet — avoid rendering an empty bubble.
                return Space::new().height(0).into();
            }
            trs.iter()
                .map(|tr| {
                    (
                        tr.name.as_str(),
                        &tr.args,
                        Some(&tr.result),
                        tr.timestamp.as_str(),
                    )
                })
                .collect()
        }
        TurnBody::Temp(tcs) => tcs
            .iter()
            .map(|tc| (tc.name.as_str(), &tc.args, None, msg.timestamp.as_str()))
            .collect(),
        _ => unreachable!("tool_turn_block called on non-tool turn"),
    };

    let mut elements: Vec<Element<'a, Message>> = Vec::new();

    for (idx, (name, args, result, ts)) in items.into_iter().enumerate() {
        if idx > 0 {
            elements.push(Space::new().height(8).into());
        }

        let badge = role_badge(format!("Tool - {name}"), "Tool", font_scale);
        let completed = result.is_some();

        let (status_icon, status_color) = match result {
            Some(Ok(_)) => ("✓", super::theme::CRABOT_SUCCESS),
            Some(Err(_)) => ("✗", super::theme::CRABOT_DANGER),
            None => ("⏳", CRABOT_TEXT_MUTED),
        };
        let expanded = completed && expanded_turns.contains(&(i, idx));
        let indicator = if expanded { "▼" } else { "⏵" };

        let status_text = text(status_icon)
            .size(12.0 * font_scale)
            .color(status_color)
            .font(if completed {
                Font {
                    weight: font::Weight::Bold,
                    ..Font::DEFAULT
                }
            } else {
                Font::DEFAULT
            });

        let ts_text = text(ts).size(11.0 * font_scale).color(CRABOT_TEXT_MUTED);

        if completed {
            let header = row![
                badge,
                status_text,
                text(indicator)
                    .size(10.0 * font_scale)
                    .color(CRABOT_TOOL_ACCENT),
                Space::new().width(Length::Fill),
                ts_text,
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            elements.push(
                mouse_area(header)
                    .on_press(Message::ToggleTurnExpand(i, idx))
                    .interaction(iced::mouse::Interaction::Pointer)
                    .into(),
            );
        } else {
            let header = row![
                badge,
                status_text,
                Space::new().width(Length::Fill),
                ts_text,
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            elements.push(header.into());
        }

        if expanded {
            elements.extend(args_rows(args, font_scale));
            elements.push(result_text(result.unwrap(), font_scale));
        } else {
            elements.extend(args_preview(name, args, font_scale));
        }
    }

    wrap_bubble(column(elements).spacing(8).width(Fill), tool_bubble_style)
}

/// Build a complete Text turn block (header + body + bubble).
fn text_turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    expanded_turns: &HashSet<(usize, usize)>,
    selectable_msgs: &HashSet<usize>,
    theme: &'a Theme,
    font_scale: f32,
) -> Element<'a, Message> {
    let TurnBody::Text(TextContent { content, reasoning }) = &msg.body else {
        unreachable!("text_turn_block called on non-Text turn")
    };

    let (role_label, bubble_style): (&'static str, fn(&Theme) -> container::Style) = match msg.role
    {
        genai::chat::ChatRole::User => ("User", user_bubble_style),
        genai::chat::ChatRole::Assistant => ("Assistant", assistant_bubble_style),
        _ => ("System", assistant_bubble_style),
    };
    let badge = role_badge(role_label.to_string(), role_label, font_scale);
    let ts_text = text(&msg.timestamp)
        .size(11.0 * font_scale)
        .color(CRABOT_TEXT_MUTED);
    let mut content_col = column![].spacing(8).width(Fill);

    // ── header: badge + (indicator if reasoning) + timestamp ──
    if reasoning.is_some() {
        // Reasoning by default is expanded so inverse membership.
        let expanded = !expanded_turns.contains(&(i, 0));
        let indicator = if expanded { "▼" } else { "⏵" };
        let header = row![
            badge,
            text(indicator)
                .size(10.0 * font_scale)
                .color(CRABOT_PRIMARY),
            Space::new().width(Length::Fill),
            ts_text,
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        content_col = content_col.push(
            mouse_area(header)
                .on_press(Message::ToggleTurnExpand(i, 0))
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
        if !expanded_turns.contains(&(i, 0)) {
            content_col = content_col.push(
                SelectableText::new(reasoning)
                    .size(13.0 * font_scale)
                    .style(sel_secondary),
            );
        }
    }
    if selectable_msgs.contains(&i) {
        content_col = content_col.push(
            SelectableText::new(content)
                .size(14.0 * font_scale)
                .style(sel_default),
        );
    } else if let Some(md) = &msg.content_md {
        let mut md_style = markdown::Style::from(theme.clone());
        md_style.inline_code_highlight = Highlight {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
        };
        md_style.inline_code_padding = 0.into();
        md_style.inline_code_color = color_text(theme);
        md_style.code_block_font = Font::MONOSPACE;
        let md_settings = markdown::Settings {
            code_size: (13.0 * font_scale).into(),
            ..markdown::Settings::with_text_size(14.0 * font_scale, md_style)
        };
        content_col = content_col.push(
            mouse_area(markdown::view(md.items(), md_settings).map(|_| Message::Noop))
                .on_double_click(Message::ToggleSelectableMode(Some(i))),
        );
    } else {
        content_col = content_col.push(
            SelectableText::new(content)
                .size(14.0 * font_scale)
                .style(sel_default),
        );
    }

    wrap_bubble(content_col, bubble_style)
}

/// Build a single turn block (header + body) wrapped in its role-colored bubble.
fn turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    expanded_turns: &'a HashSet<(usize, usize)>,
    selectable_msgs: &HashSet<usize>,
    theme: &'a Theme,
    font_scale: f32,
) -> Element<'a, Message> {
    match &msg.body {
        TurnBody::Tool(_) | TurnBody::Temp(_) => {
            tool_turn_block(msg, i, expanded_turns, font_scale)
        }
        TurnBody::Text(_) => {
            text_turn_block(msg, i, expanded_turns, selectable_msgs, theme, font_scale)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn center_pane<'a>(
    title: &'a str,
    dialogs: &'a [Dialog],
    expanded_turns: &'a HashSet<(usize, usize)>,
    expanded_dialogs: &'a HashSet<usize>,
    status: &'a str,
    theme: &'a Theme,
    streaming: StreamState,
    selectable_msgs: &HashSet<usize>,
    font_scale: f32,
    pending_user_prompt: Option<&'a str>,
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
                text(indicator)
                    .size(10.0 * font_scale)
                    .color(CRABOT_PRIMARY),
                text(title).size(13.0 * font_scale).font(Font {
                    weight: font::Weight::Bold,
                    ..Font::DEFAULT
                }),
            ]
            .width(Length::Fill)
            .spacing(8)
            .align_y(iced::Alignment::Center);

            let header = mouse_area(
                container(
                    row![title_row, turn_count_badge(turn_count, font_scale)]
                        .spacing(10)
                        .align_y(iced::Alignment::Center)
                        .width(Fill),
                )
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
                        turn_block(msg, i, expanded_turns, selectable_msgs, theme, font_scale)
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
        session_header(title),
        pending_header(pending_user_prompt),
        scrollable(column(dialog_blocks).spacing(18).padding(14),)
            .height(Fill)
            .direction(thin_vertical())
            .id(MESSAGE_SCROLL.clone())
            .on_scroll(Message::SessionViewScrolled),
        status_line(status, streaming, font_scale),
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
        container(
            SelectableText::new(prompt)
                .size(14.0)
                .style(|theme: &Theme| {
                    let p = theme.extended_palette();
                    SelectionStyle {
                        color: Some(CRABOT_TEXT),
                        selection: p.primary.base.color,
                    }
                }),
        )
        .width(Length::Fill)
        .clip(true),
        button(text("▣").size(14.0))
            .on_press(Message::CopySessionTitle)
            .padding(6)
            .style(icon_button_style),
        button(text("↻").size(14.0))
            .on_press(Message::ResendLastPrompt)
            .padding(6)
            .style(icon_button_style),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    header_container(
        container(header)
            .width(Fill)
            .padding([10, 14])
            .style(bordered_bar_style),
        200.0,
    )
}

/// Wraps content in a bordered container that scrolls vertically
/// when its natural height exceeds `max_h`.
fn header_container<'a>(
    content: impl Into<Element<'a, Message>>,
    max_h: f32,
) -> Element<'a, Message> {
    container(scrollable(content).height(Length::Shrink))
        .max_height(max_h)
        .into()
}

/// Displays the pending prompt text with a muted style.
fn pending_header<'a>(prompt: Option<&'a str>) -> Element<'a, Message> {
    let Some(prompt) = prompt else {
        return row![].into();
    };
    header_container(
        container(text(prompt).size(13.0).color(CRABOT_TEXT_MUTED))
            .width(Fill)
            .padding([6, 14])
            .style(bordered_bar_style),
        200.0,
    )
}

// ── status line ───────────────────────────────────────────────────

fn status_line<'a>(
    status_text: &'a str,
    streaming: StreamState,
    font_scale: f32,
) -> Element<'a, Message> {
    let mut row = row![
        text(status_text)
            .size(12.0 * font_scale)
            .color(CRABOT_TEXT_MUTED),
    ]
    .align_y(iced::Alignment::Center)
    .spacing(8);
    if streaming != StreamState::Idle {
        row = row.push(
            button(text("⏹ Stop").size(11.0 * font_scale))
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
