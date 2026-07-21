use std::collections::HashSet;

use crabot::chat::{Dialog, Turn, TurnBody};
use genai::chat::ChatRole;
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Font, Length, Padding, Rectangle, Task,
    Theme, Vector,
    advanced::text::Highlight,
    advanced::widget::operation::{Operation, Outcome, Scrollable, scrollable as scrollable_op},
    alignment, font, mouse,
    widget::scrollable::{Direction, Scrollbar},
    widget::{self, Space, button, column, container, markdown, mouse_area, row, scrollable, text},
};
use iced_runtime::task::widget as task_widget;
use iced_selection::Text as SelectableText;
use iced_selection::text::Style as SelectionStyle;
use serde_json::Value;

use crate::Message;
use crate::llm::DialogPhase;
use crate::views::search_bar::SearchState;

use super::icons;
use super::styles::{
    assistant_bubble_style, bordered_bar_style, icon_button_style, pane_center,
    reasoning_box_style, role_badge_style, sel_default, sel_secondary, tool_bubble_style,
    user_bubble_style,
};
use super::theme::{
    CRABOT_DANGER, CRABOT_DIALOG_BG, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY, CRABOT_SUCCESS,
    CRABOT_TEXT, CRABOT_TEXT_MUTED, CRABOT_TOOL_ACCENT, color_text, thin_vertical,
};
use super::tool_message::{
    args_rows, ask_result_view, highlighted_text, path_arg_row, result_text,
};

pub(crate) const MESSAGE_SCROLL: widget::Id = widget::Id::new("messages");
pub(crate) const SEARCH_INPUT: widget::Id = widget::Id::new("search-input");
pub(crate) const ASK_INPUT: widget::Id = widget::Id::new("ask-input");

/// Snap the message scroll to the end unconditionally.
pub(crate) fn scroll_to_end() -> Task<Message> {
    task_widget(scrollable_op::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::END.into(),
    ))
}

/// Measure the y-offsets of all turns in the scrollable content.
/// Returns a `Vec<f32>` where index `i` is the content-relative y-offset of turn `i`.
pub(crate) fn measure_turn_offsets(turn_ids: Vec<widget::Id>) -> Task<Vec<f32>> {
    struct MeasureAll {
        scrollable_id: widget::Id,
        turn_ids: Vec<widget::Id>, // turn_ids[i] = id for turn i
        scrollable_bounds: Option<Rectangle>,
        offsets: Vec<f32>,
    }

    impl Operation<Vec<f32>> for MeasureAll {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Vec<f32>>)) {
            operate(self);
        }

        fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
            if let Some(id) = id
                && let Some(idx) = self.turn_ids.iter().position(|tid| tid == id)
                && let Some(sb) = self.scrollable_bounds
            {
                // `bounds` is the absolute layout position (screen-relative,
                // WITHOUT scroll translation — iced applies the translation
                // separately during rendering via `renderer.with_translation`).
                // So `bounds.y - sb.y` gives the content-relative y-offset,
                // which is exactly what `scroll_to(AbsoluteOffset { y })` expects.
                let y = bounds.y - sb.y;
                if idx >= self.offsets.len() {
                    self.offsets.resize(idx + 1, 0.0);
                }
                self.offsets[idx] = y;
            }
        }

        fn scrollable(
            &mut self,
            id: Option<&widget::Id>,
            bounds: Rectangle,
            _content_bounds: Rectangle,
            _translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&self.scrollable_id) {
                self.scrollable_bounds = Some(bounds);
            }
        }

        fn finish(&self) -> Outcome<Vec<f32>> {
            Outcome::Some(self.offsets.clone())
        }
    }

    task_widget(MeasureAll {
        scrollable_id: MESSAGE_SCROLL.clone(),
        turn_ids,
        scrollable_bounds: None,
        offsets: Vec::new(),
    })
}

/// Scroll to a turn using a pre-measured offset.
pub(crate) fn scroll_to_turn_at(y: f32) -> Task<Message> {
    task_widget(scrollable_op::scroll_to(
        MESSAGE_SCROLL.clone(),
        scrollable::AbsoluteOffset {
            x: None,
            y: Some(y),
        },
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

// ── search match styles ───────────────────────────────────────────

/// Style for a turn that matches the search query (not the current match).
fn search_match_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.1, 0.6, 0.55, 0.08).into()),
        border: Border {
            color: Color::from_rgba(0.1, 0.6, 0.55, 0.3),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

/// Style for the currently-focused search match.
fn search_current_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.1, 0.6, 0.55, 0.15).into()),
        border: Border {
            color: CRABOT_PRIMARY,
            width: 2.0,
            radius: 4.0.into(),
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
    args: &'a Value,
    font_scale: f32,
    search_query: &str,
) -> Vec<Element<'a, Message>> {
    if name == "edit" || name == "write" {
        path_arg_row(args, font_scale, search_query)
            .into_iter()
            .collect()
    } else {
        args_rows(name, args, font_scale, search_query)
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
    search_query: &str,
) -> Element<'a, Message> {
    // Build a unified list of (name, args, result_opt, timestamp) from either variant.
    type ToolItem<'a> = (
        &'a str,
        &'a Value,
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
            Some(Ok(_)) => ("✓", CRABOT_SUCCESS),
            Some(Err(_)) => ("✗", CRABOT_DANGER),
            None => ("⏳", CRABOT_TEXT_MUTED),
        };

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

        // Completed ask tool: render question + answer without expand/collapse.
        if name == "ask" && completed {
            let header = row![
                badge,
                status_text,
                Space::new().width(Length::Fill),
                ts_text,
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            elements.push(header.into());
            elements.push(ask_result_view(args, result.unwrap(), font_scale));
            continue;
        }

        let expanded = completed && expanded_turns.contains(&(i, idx));
        let indicator = if expanded { "▼" } else { "⏵" };

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
            .align_y(Alignment::Center);
            elements.push(
                mouse_area(header)
                    .on_press(Message::ToggleTurnExpand(i, idx))
                    .interaction(mouse::Interaction::Pointer)
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
            .align_y(Alignment::Center);
            elements.push(header.into());
        }

        if expanded {
            elements.extend(args_rows(name, args, font_scale, search_query));
            elements.push(result_text(result.unwrap(), font_scale, search_query));
        } else {
            elements.extend(args_preview(name, args, font_scale, search_query));
        }
    }

    wrap_bubble(column(elements).spacing(8).width(Fill), tool_bubble_style)
}

/// Render parsed markdown as a double-click-to-select element with
/// transparent inline-code styling (shared by content and reasoning bodies).
fn markdown_element<'a>(
    md: &'a markdown::Content,
    theme: &'a Theme,
    i: usize,
    text_size: f32,
    code_size: f32,
) -> Element<'a, Message> {
    let mut md_style = markdown::Style::from(theme.clone());
    md_style.inline_code_highlight = Highlight {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
    };
    md_style.inline_code_padding = 0.into();
    md_style.inline_code_color = color_text(theme);
    md_style.code_block_font = Font::MONOSPACE;
    let md_settings = markdown::Settings {
        code_size: code_size.into(),
        ..markdown::Settings::with_text_size(text_size, md_style)
    };
    mouse_area(markdown::view(md.items(), md_settings).map(Message::LinkClicked))
        .on_double_click(Message::ToggleSelectableMode(Some(i)))
        .into()
}

/// Build a complete Text turn block (header + body + bubble).
fn text_turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    expanded_turns: &HashSet<(usize, usize)>,
    selectable_msgs: &HashSet<usize>,
    theme: &'a Theme,
    font_scale: f32,
    search_query: &str,
) -> Element<'a, Message> {
    let TurnBody::Text(tc) = &msg.body else {
        unreachable!("text_turn_block called on non-Text turn")
    };

    let (role_label, bubble_style): (&'static str, fn(&Theme) -> container::Style) = match msg.role
    {
        ChatRole::User => ("User", user_bubble_style),
        ChatRole::Assistant => ("Assistant", assistant_bubble_style),
        _ => ("System", assistant_bubble_style),
    };
    let badge = role_badge(role_label.to_string(), role_label, font_scale);
    let ts_text = text(&msg.timestamp)
        .size(11.0 * font_scale)
        .color(CRABOT_TEXT_MUTED);
    let mut content_col = column![].spacing(8).width(Fill);

    // ── header: badge + (indicator if reasoning) + timestamp ──
    if tc.reasoning.is_some() {
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
        .align_y(Alignment::Center);
        content_col = content_col.push(
            mouse_area(header)
                .on_press(Message::ToggleTurnExpand(i, 0))
                .interaction(mouse::Interaction::Pointer),
        );
    } else {
        let header =
            row![badge, Space::new().width(Length::Fill), ts_text].align_y(Alignment::Center);
        content_col = content_col.push(header);
    }

    // ── body: reasoning + content ──
    if let Some(reasoning) = &tc.reasoning {
        // Default expanded; badge-row click toggles collapse.
        if !expanded_turns.contains(&(i, 0)) {
            let reasoning_body: Element<'_, Message> = if !search_query.trim().is_empty() {
                // When searching, use highlighted plain text instead of markdown.
                highlighted_text(reasoning, search_query, 13.0 * font_scale)
            } else if !selectable_msgs.contains(&i)
                && let Some(md) = &tc.reasoning_md
            {
                markdown_element(md, theme, i, 13.0 * font_scale, 12.0 * font_scale)
            } else {
                SelectableText::new(reasoning)
                    .size(13.0 * font_scale)
                    .style(sel_secondary)
                    .into()
            };
            content_col = content_col.push(
                container(reasoning_body)
                    .style(reasoning_box_style)
                    .width(Length::Fill)
                    .padding(Padding {
                        top: 6.0,
                        right: 10.0,
                        bottom: 6.0,
                        left: 10.0,
                    }),
            );
        }
    }
    if !search_query.trim().is_empty() {
        content_col = content_col.push(highlighted_text(
            &tc.content,
            search_query,
            14.0 * font_scale,
        ));
    } else if !selectable_msgs.contains(&i)
        && let Some(md) = &tc.content_md
    {
        content_col = content_col.push(markdown_element(
            md,
            theme,
            i,
            14.0 * font_scale,
            13.0 * font_scale,
        ));
    } else {
        content_col = content_col.push(
            SelectableText::new(&tc.content)
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
    search_query: &str,
) -> Element<'a, Message> {
    match &msg.body {
        TurnBody::Tool(_) | TurnBody::Temp(_) => {
            tool_turn_block(msg, i, expanded_turns, font_scale, search_query)
        }
        TurnBody::Text(_) => text_turn_block(
            msg,
            i,
            expanded_turns,
            selectable_msgs,
            theme,
            font_scale,
            search_query,
        ),
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
    streaming: DialogPhase,
    selectable_msgs: &HashSet<usize>,
    font_scale: f32,
    pending_user_prompt: Option<&'a str>,
    ask_request: Option<&'a super::session_state::AskRequest>,
    ask_input: &'a str,
    search_state: &'a SearchState,
    model_id: Option<&'a str>,
    created_at: &'a str,
) -> Element<'a, Message> {
    // Ensure turn widget IDs match the current dialog layout so that
    // scroll-to-match measurement can find each turn by its ID.
    let total: usize = dialogs.iter().map(|d| d.turns.len()).sum();
    search_state.ensure_turn_ids(total);
    let turn_ids = search_state.turn_ids();
    let search_query: &str = &search_state.query;
    let search_results: &[usize] = &search_state.results;
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
            .align_y(Alignment::Center);

            let header = mouse_area(
                container(
                    row![title_row, turn_count_badge(turn_count, font_scale)]
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .width(Fill),
                )
                .width(Fill)
                .padding([8, 12]),
            )
            .on_press(Message::ToggleDialogExpand(di))
            .interaction(mouse::Interaction::Pointer);

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
                        let is_match = search_results.contains(&i);
                        let is_current = is_match
                            && !search_results.is_empty()
                            && search_results[search_state.current] == i;
                        let block = turn_block(
                            msg,
                            i,
                            expanded_turns,
                            selectable_msgs,
                            theme,
                            font_scale,
                            search_query,
                        );
                        let style: fn(&Theme) -> container::Style = if is_current {
                            search_current_style
                        } else if is_match {
                            search_match_style
                        } else {
                            |_| container::Style::default()
                        };
                        container(block)
                            .width(Fill)
                            .padding(2)
                            .style(style)
                            .id(turn_ids[i].clone())
                            .into()
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
        if search_state.visible {
            super::search_bar::view(search_query, search_results, search_state.current)
                .map(Message::SearchEvent)
        } else {
            row![].into()
        },
        scrollable(
            column![
                session_info(model_id, created_at, font_scale),
                column(dialog_blocks).spacing(8),
            ]
            .spacing(8)
            .padding(14),
        )
        .height(Fill)
        .direction(Direction::Vertical(
            Scrollbar::new().width(6).scroller_width(6)
        ))
        .id(MESSAGE_SCROLL.clone())
        .on_scroll(Message::SessionViewScrolled),
        ask_request
            .map(|request| super::tool_message::ask_view(request, ask_input, font_scale))
            .unwrap_or_else(|| Space::new().into()),
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
        icons::icon_action(icons::COPY, "Copy session title", Message::CopySessionTitle),
        icons::icon_action(
            icons::RESEND,
            "Resend session history",
            Message::ResendSessionHistory
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    header_container(
        container(header)
            .width(Fill)
            .padding([10, 14])
            .style(bordered_bar_style),
        200.0,
    )
}

/// Displays the model ID and creation time for the current session.
fn session_info<'a>(
    model_id: Option<&'a str>,
    created_at: &'a str,
    font_scale: f32,
) -> Element<'a, Message> {
    let Some(model_id) = model_id else {
        return row![].into();
    };
    let model_text = text(format!("Model: {model_id}"))
        .size(12.0 * font_scale)
        .color(CRABOT_TEXT_MUTED);
    let time_text = text(format!("Created: {created_at}"))
        .size(12.0 * font_scale)
        .color(CRABOT_TEXT_MUTED);
    container(
        row![model_text, Space::new().width(Length::Fill), time_text]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Fill),
    )
    .padding(Padding {
        top: 4.0,
        right: 14.0,
        bottom: 4.0,
        left: 12.0,
    })
    .into()
}

/// Wraps content in a bordered container that scrolls vertically
/// when its natural height exceeds `max_h`.
fn header_container<'a>(
    content: impl Into<Element<'a, Message>>,
    max_h: f32,
) -> Element<'a, Message> {
    container(
        scrollable(content)
            .direction(thin_vertical())
            .height(Length::Shrink),
    )
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
    streaming: DialogPhase,
    font_scale: f32,
) -> Element<'a, Message> {
    let mut row = row![
        text(status_text)
            .size(12.0 * font_scale)
            .color(CRABOT_TEXT_MUTED),
    ]
    .align_y(Alignment::Center)
    .spacing(8);
    if streaming != DialogPhase::Idle {
        row = row.push(
            button(text("⏹ Stop").size(11.0 * font_scale))
                .on_press(Message::SessionEvent(
                    crate::views::session_state::SessionEvent::Stop,
                ))
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
