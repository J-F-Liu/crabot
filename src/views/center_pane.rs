use std::borrow::Cow;
use std::collections::HashSet;

use crabot::chat::{Dialog, Turn, TurnBody};
use crabot::session::Session;
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

use crate::app::session_state::SessionEvent;
use crate::app::{ConversationState, SessionTab};
use crate::llm::DialogPhase;
use crate::views::search_bar::SearchState;
use crate::{AskRequest, CenterPaneEvent, ConversationEvent};

use super::icons;
use super::styles::{
    assistant_bubble_style, bordered_bar_style, icon_button_style, pane_center,
    reasoning_box_style, role_badge_style, sel_default, sel_secondary, session_header_style,
    tool_bubble_style, user_bubble_style,
};
use super::theme::{
    CRABOT_DANGER, CRABOT_DIALOG_RADIUS, CRABOT_PRIMARY, CRABOT_SUCCESS, CRABOT_TOOL_ACCENT,
    color_dialog_bg, color_muted, color_surface, color_text, color_text_strong, thin_vertical,
};
use super::tool_message::{
    args_rows, ask_result_view, highlighted_text, path_arg_row, result_text,
};

pub(crate) const MESSAGE_SCROLL: widget::Id = widget::Id::new("messages");
pub(crate) const SEARCH_INPUT: widget::Id = widget::Id::new("search-input");
pub(crate) const ASK_INPUT: widget::Id = widget::Id::new("ask-input");

/// Fraction of the viewport scrolled per page key (leaves 10% overlap for context).
const PAGE_SCROLL_FRACTION: f32 = 0.9;

/// Vertical pixels scrolled per arrow-key press on the message view.
pub(crate) const SCROLL_STEP: f32 = 40.0;

/// Snap the message scroll to the end unconditionally.
pub(crate) fn scroll_to_end() -> Task<()> {
    task_widget(scrollable_op::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::END.into(),
    ))
}

/// Snap the message scroll to the start unconditionally.
pub(crate) fn scroll_to_start() -> Task<()> {
    task_widget(scrollable_op::snap_to(
        MESSAGE_SCROLL.clone(),
        scrollable::RelativeOffset::START.into(),
    ))
}

/// Scroll the message viewport vertically by `delta_y` pixels (positive = down).
pub(crate) fn scroll_by(delta_y: f32) -> Task<()> {
    task_widget(scrollable_op::scroll_by(
        MESSAGE_SCROLL.clone(),
        scrollable::AbsoluteOffset { x: 0.0, y: delta_y },
    ))
}

/// Scroll the message viewport down by one page.
pub(crate) fn scroll_page_down(viewport_height: f32) -> Task<()> {
    scroll_by(viewport_height * PAGE_SCROLL_FRACTION)
}

/// Scroll the message viewport up by one page.
pub(crate) fn scroll_page_up(viewport_height: f32) -> Task<()> {
    scroll_by(-viewport_height * PAGE_SCROLL_FRACTION)
}

/// Measure each turn's content-relative y-offset (index `i` → turn `i`).
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
                // `bounds` is absolute layout position without scroll translation —
                // iced applies translation separately, so `bounds.y - sb.y` is the
                // content-relative offset that `scroll_to(AbsoluteOffset { y })` expects.
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

/// Scroll the message view to an absolute y-offset; `None` scrolls to top.
pub(crate) fn scroll_to(y: Option<f32>) -> Task<()> {
    match y {
        Some(y) => task_widget(scrollable_op::scroll_to(
            MESSAGE_SCROLL.clone(),
            scrollable::AbsoluteOffset {
                x: None,
                y: Some(y),
            },
        )),
        None => scroll_to_start(),
    }
}

// ── dialog styles ─────────────────────────────────────────────────

fn dialog_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(color_dialog_bg().into()),
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

/// Small work-mode badge pill shown in dialog headers.
fn work_mode_badge(name: String, font_scale: f32) -> Element<'static, CenterPaneEvent> {
    container(text(name).size(11.0 * font_scale).font(Font {
        weight: font::Weight::Semibold,
        ..Font::DEFAULT
    }))
    .padding([2, 8])
    .style(|_theme: &Theme| container::Style {
        background: Some(Color::from_rgba(0.1, 0.6, 0.55, 0.12).into()),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        text_color: Some(CRABOT_PRIMARY),
        ..container::Style::default()
    })
    .into()
}

/// Small turn-count pill.
fn turn_count_badge(count: usize, font_scale: f32) -> Element<'static, CenterPaneEvent> {
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
        background: Some(color_surface().into()),
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        text_color: Some(color_muted()),
        ..container::Style::default()
    })
    .into()
}

/// Shared context for building turn blocks.
struct TurnView<'a> {
    expanded_turns: &'a HashSet<(usize, usize)>,
    selectable_msgs: &'a HashSet<usize>,
    theme: &'a Theme,
    font_scale: f32,
    search_query: &'a str,
}

// ── turn block builders ────────────────────────────────────────────

/// Build the colored role badge shown in a turn header.
fn role_badge(
    badge_text: String,
    style_label: &'static str,
    font_scale: f32,
) -> Element<'static, CenterPaneEvent> {
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
    content: impl Into<Element<'a, CenterPaneEvent>>,
    style: fn(&Theme) -> container::Style,
) -> Element<'a, CenterPaneEvent> {
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
) -> Vec<Element<'a, CenterPaneEvent>> {
    if name == "edit" || name == "write" {
        path_arg_row(args, font_scale, search_query)
            .into_iter()
            .collect()
    } else {
        args_rows(name, args, font_scale, search_query)
    }
}

/// Tool item header without an expand indicator (pending/streaming/ask items).
fn tool_header_row<'a>(
    badge: impl Into<Element<'a, CenterPaneEvent>>,
    status_text: impl Into<Element<'a, CenterPaneEvent>>,
    ts_text: impl Into<Element<'a, CenterPaneEvent>>,
) -> Element<'a, CenterPaneEvent> {
    row![
        badge.into(),
        status_text.into(),
        Space::new().width(Length::Fill),
        ts_text.into(),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// Build a Tool turn block — completed (`Tool`) or pending (`Temp`) calls;
/// multiple calls from one response are grouped as stacked sub-items.
fn tool_turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    ctx: &TurnView<'_>,
    streaming_ids: &HashSet<&str>,
) -> Element<'a, CenterPaneEvent> {
    // Unified (name, args, result, timestamp, streaming) items from either variant.
    type ToolItem<'a> = (
        &'a str,
        &'a Value,
        Option<&'a Result<String, String>>,
        &'a str,
        bool, // streaming: still running, `result` holds partial live output
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
                        tr.streaming,
                    )
                })
                .collect()
        }
        TurnBody::Temp(tcs) => tcs
            .iter()
            // Hidden when a live streaming placeholder already renders the call.
            .filter(|tc| {
                tc.call_id
                    .as_deref()
                    .is_none_or(|id| !streaming_ids.contains(id))
            })
            .map(|tc| {
                (
                    tc.name.as_str(),
                    &tc.args,
                    None,
                    msg.timestamp.as_str(),
                    false,
                )
            })
            .collect(),
        _ => unreachable!("tool_turn_block called on non-tool turn"),
    };

    let mut elements: Vec<Element<'a, CenterPaneEvent>> = Vec::new();

    for (idx, (name, args, result, ts, streaming)) in items.into_iter().enumerate() {
        if idx > 0 {
            elements.push(Space::new().height(8).into());
        }

        let badge = role_badge(format!("Tool - {name}"), "Tool", ctx.font_scale);
        let completed = result.is_some() && !streaming;

        let (status_icon, status_color) = match (result, streaming) {
            (Some(Ok(_)), false) => ("✓", CRABOT_SUCCESS),
            (Some(Err(_)), false) => ("✗", CRABOT_DANGER),
            _ => ("⏳", color_muted()),
        };

        let status_text = text(status_icon)
            .size(12.0 * ctx.font_scale)
            .color(status_color)
            .font(if completed {
                Font {
                    weight: font::Weight::Bold,
                    ..Font::DEFAULT
                }
            } else {
                Font::DEFAULT
            });

        let ts_text = text(ts).size(11.0 * ctx.font_scale).color(color_muted());

        // Running tool: header + args + live output, no expand/collapse
        // control; the placeholder is replaced by the final result on finish.
        if streaming {
            elements.push(tool_header_row(badge, status_text, ts_text));
            elements.extend(args_rows(name, args, ctx.font_scale, ctx.search_query));
            if let Some(Ok(buffer)) = result {
                elements.push(super::tool_message::streaming_result_text(
                    buffer,
                    ctx.font_scale,
                ));
            }
            continue;
        }

        // Completed ask tool: render question + answer without expand/collapse.
        if name == "ask" && completed {
            elements.push(tool_header_row(badge, status_text, ts_text));
            elements.push(
                ask_result_view(args, result.unwrap(), ctx.font_scale)
                    .map(CenterPaneEvent::Conversation),
            );
            continue;
        }

        let expanded = completed && ctx.expanded_turns.contains(&(i, idx));
        let indicator = if expanded { "▼" } else { "⏵" };

        if completed {
            let header = row![
                badge,
                status_text,
                text(indicator)
                    .size(10.0 * ctx.font_scale)
                    .color(CRABOT_TOOL_ACCENT),
                Space::new().width(Length::Fill),
                ts_text,
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            elements.push(
                mouse_area(header)
                    .on_press(CenterPaneEvent::Conversation(
                        ConversationEvent::ToggleTurnExpand(i, idx),
                    ))
                    .interaction(mouse::Interaction::Pointer)
                    .into(),
            );
        } else {
            elements.push(tool_header_row(badge, status_text, ts_text));
        }

        if expanded {
            elements.extend(args_rows(name, args, ctx.font_scale, ctx.search_query));
            elements.push(result_text(
                result.unwrap(),
                ctx.font_scale,
                ctx.search_query,
            ));
        } else {
            elements.extend(args_preview(name, args, ctx.font_scale, ctx.search_query));
        }
    }

    wrap_bubble(column(elements).spacing(8).width(Fill), tool_bubble_style)
}

/// True when parsed markdown is a single plain-text paragraph
/// (no headings, lists, code blocks, etc.).
fn is_plain_text(md: &markdown::Content) -> bool {
    md.items().len() == 1 && matches!(md.items().first(), Some(markdown::Item::Paragraph(_)))
}

/// Render markdown as double-click-to-select text with transparent inline-code styling.
fn markdown_element<'a>(
    md: &'a markdown::Content,
    i: usize,
    text_size: f32,
    ctx: &TurnView<'a>,
) -> Element<'a, CenterPaneEvent> {
    let code_size = (text_size - 1.0) * ctx.font_scale;
    let text_size = text_size * ctx.font_scale;
    let mut md_style = markdown::Style::from(ctx.theme.clone());
    md_style.inline_code_highlight = Highlight {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
    };
    md_style.inline_code_padding = 0.into();
    md_style.inline_code_color = color_text(ctx.theme);
    md_style.code_block_font = Font::MONOSPACE;
    let md_settings = markdown::Settings {
        code_size: code_size.into(),
        ..markdown::Settings::with_text_size(text_size, md_style)
    };
    mouse_area(markdown::view(md.items(), md_settings).map(CenterPaneEvent::LinkClicked))
        .on_double_click(CenterPaneEvent::Conversation(
            ConversationEvent::ToggleSelectableMode(Some(i)),
        ))
        .into()
}

/// Build a complete Text turn block (header + body + bubble).
fn text_turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    ctx: &TurnView<'a>,
) -> Element<'a, CenterPaneEvent> {
    let TurnBody::Text(tc) = &msg.body else {
        unreachable!("text_turn_block called on non-Text turn")
    };

    let (role_label, bubble_style): (&'static str, fn(&Theme) -> container::Style) = match msg.role
    {
        ChatRole::User => ("User", user_bubble_style),
        ChatRole::Assistant => ("Assistant", assistant_bubble_style),
        _ => ("System", assistant_bubble_style),
    };
    let badge = role_badge(role_label.to_string(), role_label, ctx.font_scale);
    let ts_text = text(&msg.timestamp)
        .size(11.0 * ctx.font_scale)
        .color(color_muted());
    let mut content_col = column![].spacing(8).width(Fill);

    // ── header: badge + (indicator if reasoning) + timestamp ──
    if tc.reasoning.is_some() {
        // Reasoning by default is expanded so inverse membership.
        let expanded = !ctx.expanded_turns.contains(&(i, 0));
        let indicator = if expanded { "▼" } else { "⏵" };
        let header = row![
            badge,
            text(indicator)
                .size(10.0 * ctx.font_scale)
                .color(CRABOT_PRIMARY),
            Space::new().width(Length::Fill),
            ts_text,
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        content_col = content_col.push(
            mouse_area(header)
                .on_press(CenterPaneEvent::Conversation(
                    ConversationEvent::ToggleTurnExpand(i, 0),
                ))
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
        if !ctx.expanded_turns.contains(&(i, 0)) {
            let reasoning_body: Element<'_, CenterPaneEvent> =
                if !ctx.search_query.trim().is_empty() {
                    // When searching, use highlighted plain text instead of markdown.
                    highlighted_text(reasoning, ctx.search_query, 13.0 * ctx.font_scale)
                } else if !ctx.selectable_msgs.contains(&i)
                    && let Some(md) = &tc.reasoning_md
                    && (!is_plain_text(md) || tc.reasoning_has_url)
                {
                    markdown_element(md, i, 13.0, ctx)
                } else {
                    SelectableText::new(reasoning)
                        .size(13.0 * ctx.font_scale)
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
    if !ctx.search_query.trim().is_empty() {
        content_col = content_col.push(highlighted_text(
            &tc.content,
            ctx.search_query,
            14.0 * ctx.font_scale,
        ));
    } else if !ctx.selectable_msgs.contains(&i)
        && let Some(md) = &tc.content_md
        && (!is_plain_text(md) || tc.has_url)
    {
        content_col = content_col.push(markdown_element(md, i, 14.0, ctx));
    } else {
        content_col = content_col.push(
            SelectableText::new(&tc.content)
                .size(14.0 * ctx.font_scale)
                .style(sel_default),
        );
    }

    wrap_bubble(content_col, bubble_style)
}

/// Build a single turn block (header + body) wrapped in its role-colored bubble.
fn turn_block<'a>(
    msg: &'a Turn,
    i: usize,
    ctx: &TurnView<'a>,
    streaming_ids: &HashSet<&str>,
) -> Element<'a, CenterPaneEvent> {
    match &msg.body {
        TurnBody::Tool(_) | TurnBody::Temp(_) => tool_turn_block(msg, i, ctx, streaming_ids),
        TurnBody::Text(_) => text_turn_block(msg, i, ctx),
    }
}

pub(crate) fn center_pane<'a>(
    conversation: &'a ConversationState,
    theme: &'a Theme,
    font_scale: f32,
) -> Element<'a, CenterPaneEvent> {
    let tab: &SessionTab = conversation.viewing();
    let title: &str = &tab.center_pane_title;
    let dialogs: &[Dialog] = tab.session.dialogs.as_slice();
    let expanded_turns: &HashSet<(usize, usize)> = &tab.expanded_turns;
    let expanded_dialogs: &HashSet<usize> = &tab.expanded_dialogs;
    let status = conversation.status();
    let streaming: DialogPhase = tab.session_state.phase;
    let selectable_msgs: &HashSet<usize> = &tab.selectable_msgs;
    let pending_user_prompt: Option<&str> = tab
        .session_state
        .pending_prompt
        .as_ref()
        .map(|p| p.content.as_str());
    let ask_request: Option<&AskRequest> = tab.session_state.ask_request.as_ref();
    let ask_input: &str = &tab.session_state.ask_input;
    let search_state: &SearchState = &tab.search;
    // Ensure turn widget IDs match the dialog layout for scroll-to-match.
    let total: usize = dialogs.iter().map(|d| d.turns.len()).sum();
    search_state.ensure_turn_ids(total);
    let turn_ids = search_state.turn_ids();
    // Render against the query only while the bar is visible; a hidden bar
    // (query preserved for Ctrl+F) fall back to normal markdown.
    let search_query: &str = if search_state.visible {
        &search_state.query
    } else {
        ""
    };
    let search_results: &[usize] = &search_state.results;
    // Running-in-background tab numbers for the status line.
    let running_tabs: Vec<usize> = conversation
        .running_positions()
        .filter(|&rp| rp != conversation.viewing)
        .map(|rp| conversation.session_tabs[rp].number)
        .collect();
    let viewing_number = tab.number;
    // Set up shared context for turn block builders.
    let turn_ctx = TurnView {
        expanded_turns,
        selectable_msgs,
        theme,
        font_scale,
        search_query,
    };
    // Flatten dialogs into turns with a running flat index per dialog.
    let mut flat_idx: usize = 0;
    let dialog_blocks: Vec<Element<'_, CenterPaneEvent>> = dialogs
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
            let mut row_elements: Vec<Element<'_, CenterPaneEvent>> = vec![
                text(indicator)
                    .size(10.0 * font_scale)
                    .color(CRABOT_PRIMARY)
                    .into(),
            ];
            if let Some(mode) = dialog.mode {
                row_elements.push(work_mode_badge(mode.name.to_string(), font_scale));
            }
            row_elements.push(
                text(title)
                    .size(13.0 * font_scale)
                    .font(Font {
                        weight: font::Weight::Bold,
                        ..Font::DEFAULT
                    })
                    .into(),
            );
            let title_row = iced::widget::Row::with_children(row_elements)
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
            .on_press(CenterPaneEvent::Conversation(
                ConversationEvent::ToggleDialogExpand(di),
            ))
            .interaction(mouse::Interaction::Pointer);

            // ── turn blocks (only built when expanded) ────────────
            let turn_blocks: Vec<Element<'_, CenterPaneEvent>> = if collapsed {
                flat_idx += dialog.turns.len();
                Vec::new()
            } else {
                // Pending calls already rendered by a live streaming placeholder.
                let mut streaming_ids: HashSet<&str> = HashSet::new();
                for turn in &dialog.turns {
                    if let TurnBody::Tool(trs) = &turn.body {
                        streaming_ids.extend(
                            trs.iter()
                                .filter(|tr| tr.streaming)
                                .filter_map(|tr| tr.call_id.as_deref()),
                        );
                    }
                }
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
                        let block = turn_block(msg, i, &turn_ctx, &streaming_ids);
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

    mouse_area(
        container(column![
            super::session_tabs::session_tabs(conversation),
            session_header(title),
            pending_header(pending_user_prompt),
            if search_state.visible {
                super::search_bar::view(search_query, search_results, search_state.current).map(
                    |event| CenterPaneEvent::Conversation(ConversationEvent::SearchEvent(event)),
                )
            } else {
                row![].into()
            },
            scrollable(
                column![
                    session_info(&tab.session, font_scale, conversation),
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
            .on_scroll(CenterPaneEvent::SessionViewScrolled),
            ask_request
                .map(|request| {
                    super::tool_message::ask_view(
                        request,
                        ask_input,
                        tab.session_state.ask_seconds_left,
                        font_scale,
                    )
                    .map(CenterPaneEvent::Conversation)
                })
                .unwrap_or_else(|| Space::new().into()),
            status_line(status, streaming, &running_tabs, viewing_number, font_scale),
        ])
        .width(Fill)
        .height(Fill)
        .style(pane_center),
    )
    .on_press(CenterPaneEvent::Conversation(
        ConversationEvent::DefocusSessionPicker,
    ))
    .into()
}

// ── session header ──────────────────────────────────────────────────

/// Header bar: prompt text or "New session", plus copy/resend icons.
fn session_header<'a>(prompt: &'a str) -> Element<'a, CenterPaneEvent> {
    let header = row![
        header_container(
            SelectableText::new(prompt)
                .size(14.0)
                .style(|theme: &Theme| {
                    let p = theme.extended_palette();
                    SelectionStyle {
                        color: Some(color_text_strong()),
                        selection: p.primary.base.color,
                    }
                }),
            200.0,
        ),
        icons::icon_action(
            icons::COPY,
            "Copy session title",
            CenterPaneEvent::Conversation(ConversationEvent::CopySessionTitle)
        ),
        icons::icon_action(
            icons::RESEND,
            "Resend session history",
            CenterPaneEvent::Conversation(ConversationEvent::ResendSessionHistory)
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(header)
        .width(Fill)
        .padding([6, 14])
        .style(session_header_style)
        .into()
}

/// Displays model ID, the spawning parent session, and creation time.
fn session_info<'a>(
    session: &'a Session,
    font_scale: f32,
    conversation: &'a ConversationState,
) -> Element<'a, CenterPaneEvent> {
    let model_id = session.model.as_ref().map(|m| m.model_id.as_str());
    let parent = session.parent.as_str();
    if model_id.is_none() && parent.is_empty() {
        return row![].into();
    }
    let mut info = row![].spacing(8).align_y(Alignment::Center);
    if let Some(model_id) = model_id {
        info = info.push(
            text(format!("Model: {model_id}"))
                .size(12.0 * font_scale)
                .color(color_muted()),
        );
    }
    if !parent.is_empty() {
        info = info.push(Space::new().width(Length::Fill)).push(
            text(format!(
                "Spawned from {}",
                parent_label(conversation, parent)
            ))
            .size(12.0 * font_scale)
            .color(color_muted()),
        );
    }
    let time_text = text(format!("Created: {}", session.created_at))
        .size(12.0 * font_scale)
        .color(color_muted());
    container(
        info.push(Space::new().width(Length::Fill))
            .push(time_text)
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

/// Parent reference: the open tab's label ("Session 2"), else the raw session id.
fn parent_label(conversation: &ConversationState, parent: &str) -> String {
    conversation
        .session_tabs
        .iter()
        .find(|t| t.session.id == parent)
        .map(|t| t.tab_label())
        .unwrap_or_else(|| format!("session {parent}"))
}

/// Scrollable container that fills width and scrolls vertically past `max_h`.
fn header_container<'a>(
    content: impl Into<Element<'a, CenterPaneEvent>>,
    max_h: f32,
) -> Element<'a, CenterPaneEvent> {
    container(
        scrollable(content)
            .direction(thin_vertical())
            .width(Fill)
            .height(Length::Shrink),
    )
    .width(Length::Fill)
    .max_height(max_h)
    .into()
}

/// Displays the pending prompt text with a muted, selectable style.
fn pending_header<'a>(prompt: Option<&'a str>) -> Element<'a, CenterPaneEvent> {
    let Some(prompt) = prompt else {
        return row![].into();
    };
    header_container(
        container(
            SelectableText::new(prompt)
                .size(13.0)
                .style(|theme: &Theme| {
                    let p = theme.extended_palette();
                    SelectionStyle {
                        color: Some(color_muted()),
                        selection: p.primary.base.color,
                    }
                }),
        )
        .width(Fill)
        .padding([6, 14])
        .style(bordered_bar_style),
        200.0,
    )
}

// ── status line ───────────────────────────────────────────────────

fn status_line(
    status_text: Cow<'static, str>,
    phase: DialogPhase,
    running_tabs: &[usize],
    viewing_number: usize,
    font_scale: f32,
) -> Element<'static, CenterPaneEvent> {
    let mut row = row![].align_y(Alignment::Center).spacing(8);

    // Viewing tab status; stop button while streaming.
    row = row.push(
        text(status_text)
            .size(12.0 * font_scale)
            .color(color_muted()),
    );
    if phase != DialogPhase::Idle {
        row = row.push(
            button(text("⏹ Stop").size(11.0 * font_scale))
                .on_press(CenterPaneEvent::Conversation(
                    ConversationEvent::SessionEvent(viewing_number, SessionEvent::Stop),
                ))
                .padding([4, 10])
                .style(icon_button_style),
        );
    }

    // Background running sessions indicator with per-tab stop buttons.
    if !running_tabs.is_empty() {
        let label = if running_tabs.len() == 1 {
            format!("Session {} running…", running_tabs[0])
        } else {
            let nums: Vec<String> = running_tabs.iter().map(|n| n.to_string()).collect();
            format!("Sessions {} running…", nums.join(", "))
        };
        row = row.push(text(label).size(12.0 * font_scale).color(color_muted()));
    }

    container(row)
        .width(Fill)
        .align_x(alignment::Horizontal::Center)
        .padding([6, 12])
        .style(bordered_bar_style)
        .into()
}
