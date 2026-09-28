use std::borrow::Cow;
use std::ops::Range;

use iced::{
    Alignment, Border, Color, Element, Fill, Font, Theme, font, padding,
    widget::text::{IntoFragment, Span},
    widget::{Space, column, container, rich_text, row, span, text, text::Wrapping},
};
use iced_selection::text::Style as SelectionStyle;

use super::ASK_INPUT;
use super::styles::{primary_button, secondary_button};
use super::styles::{sel_default, sel_primary, sel_secondary};
use super::theme::{
    CRABOT_DANGER, CRABOT_SUCCESS, CRABOT_TOOL_ACCENT, color_diff_bg_add, color_diff_bg_del,
    color_muted, color_text, color_tool_content_bg, color_tool_content_border,
};
use super::tool_view::{self, ArgRow, EditRow, TodoRow, TodoState};
use crate::app::session_state::ASK_EXTEND_SECS;
use crate::{AskAction, AskRequest, ConversationEvent};
use crabot::i18n::Lang;
use iced::widget::{button, text_input};

/// Rounded box for tool argument rows and result bodies: optional fill plus border.
fn tool_box_style(background: Option<Color>, border: Color, radius: f32) -> container::Style {
    container::Style {
        background: background.map(Into::into),
        border: Border {
            color: border,
            width: 1.0,
            radius: radius.into(),
        },
        ..container::Style::default()
    }
}

/// Boxed tool content (ask views, argument rows): fill plus thin border.
fn tool_content_box(_theme: &Theme) -> container::Style {
    tool_box_style(
        Some(color_tool_content_bg()),
        color_tool_content_border(),
        4.0,
    )
}

/// Boxed tool result body: `background` fill plus a thin border.
fn tool_result_box(background: Color, border: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| tool_box_style(Some(background), border, 6.0)
}

/// Shared container style for ask tool views (active and completed).
fn ask_tool_container<'a>(
    content: impl Into<Element<'a, ConversationEvent>>,
) -> Element<'a, ConversationEvent> {
    container(content.into())
        .padding([10, 14])
        .style(tool_content_box)
        .width(Fill)
        .into()
}

/// Render a list of options with checkmarks.
/// In interactive mode each option is a clickable `button`; otherwise
/// options are rendered as read-only selectable text.
fn ask_option_list<'a>(
    options: impl IntoIterator<Item = &'a str>,
    selected: &str,
    font_scale: f32,
    interactive: bool,
) -> Vec<Element<'a, ConversationEvent>> {
    options
        .into_iter()
        .map(|option| {
            let is_selected = option == selected;
            let check = if is_selected { "✓" } else { " " };
            let label: Element<'a, ConversationEvent> = if interactive {
                button(text(option).size(13.0 * font_scale))
                    .style(secondary_button)
                    .on_press(ConversationEvent::AskAction(AskAction::OptionSelected(
                        option.to_owned(),
                    )))
                    .into()
            } else {
                selectable(option, "")
                    .size(13.0 * font_scale)
                    .style(sel_default)
                    .into()
            };
            row![
                text(check).width(16.0 * font_scale).size(13.0 * font_scale),
                label,
            ]
            .align_y(Alignment::Center)
            .into()
        })
        .collect()
}

/// Free-text answer input shared by both ask layouts.
fn ask_answer_input(input: &str, lang: Lang) -> Element<'static, ConversationEvent> {
    text_input(lang.tr("Type your answer…"), input)
        .id(ASK_INPUT.clone())
        .on_input(ConversationEvent::AskInputChanged)
        .on_submit_maybe((!input.is_empty()).then_some(ConversationEvent::AskAction(AskAction::Ok)))
        .into()
}

/// Interactive response controls for the builtin ask tool.
pub(crate) fn ask_view<'a>(
    request: &'a AskRequest,
    input: &str,
    custom_input: bool,
    seconds_left: u64,
    font_scale: f32,
    lang: Lang,
) -> Element<'a, ConversationEvent> {
    let countdown: Element<'static, ConversationEvent> = row![
        text(lang.tr("⏳ {seconds_left}s left").replacen(
            "{seconds_left}",
            &seconds_left.to_string(),
            1
        ),)
        .size(12.0 * font_scale)
        .color(color_muted()),
        button(text(lang.tr("Extend +{} min").replacen(
            "{}",
            &(ASK_EXTEND_SECS / 60).to_string(),
            1
        ),))
        .style(secondary_button)
        // Dead at 0s — the timeout result is already in flight.
        .on_press_maybe(
            (seconds_left > 0).then_some(ConversationEvent::AskAction(AskAction::Extend))
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into();
    let header = row![
        text(lang.tr("🤖 LLM asks:"))
            .size(13.0)
            .color(CRABOT_TOOL_ACCENT),
        Space::new().width(Fill),
        countdown
    ];
    let question: Element<'a, ConversationEvent> =
        selectable(&request.question, "").style(sel_default).into();
    let enter_answer = button(text(lang.tr("Enter my answer")))
        .style(secondary_button)
        .on_press(ConversationEvent::AskAction(AskAction::EnterAnswer));
    let you_decide = button(text(lang.tr("You decide")))
        .style(secondary_button)
        .on_press(ConversationEvent::AskAction(AskAction::YouDecide));
    let ok = button(text(lang.tr("Ok")))
        .style(primary_button)
        .on_press_maybe((!input.is_empty()).then_some(ConversationEvent::AskAction(AskAction::Ok)));
    let controls: Element<'a, ConversationEvent> = if request.options.is_empty() {
        row![ask_answer_input(input, lang), ok, you_decide]
            .spacing(8)
            .into()
    } else {
        let mut options_col = column(ask_option_list(
            request.options.iter().map(String::as_str),
            input,
            font_scale,
            true,
        ))
        .spacing(8);
        if custom_input {
            options_col = options_col.push(ask_answer_input(input, lang));
        }
        let action_row = row![ok, enter_answer, you_decide]
            .spacing(8)
            .padding([4, 16]);
        column![options_col, action_row].spacing(8).into()
    };
    ask_tool_container(column![header, question, controls].spacing(8))
}

/// Completed ask tool result view — shows the question, all options
/// (with the selected one marked ✓), and the answer without interactive
/// controls (those only appear during active asking via [`ask_view`]).
pub(crate) fn ask_result_view<'a>(
    args: &'a serde_json::Value,
    result: &'a Result<String, String>,
    font_scale: f32,
    lang: Lang,
) -> Element<'a, ConversationEvent> {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let options: Vec<&str> = args
        .get("options")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let (answer, is_ok) = match result {
        Ok(s) => (s.as_str(), true),
        Err(e) => (e.as_str(), false),
    };

    let question_text: Element<'a, ConversationEvent> =
        selectable(question, "").style(sel_default).into();

    let answer_label = lang.tr(if is_ok { "Answer" } else { "Error" });
    let answer_color = if is_ok { CRABOT_SUCCESS } else { CRABOT_DANGER };

    let mut answer_col = column![];

    if !options.is_empty() {
        let matched = options.contains(&answer);
        let option_rows = ask_option_list(options, answer, font_scale, false);

        if matched {
            answer_col = answer_col
                .push(
                    text(format!("{answer_label}:"))
                        .size(12.0 * font_scale)
                        .color(answer_color)
                        .font(bold_font()),
                )
                .push(column(option_rows).spacing(2));
        } else {
            // When the answer doesn't match any option (e.g. the user skipped),
            // show the actual answer text so it isn't lost.
            answer_col = answer_col
                .push(
                    text(lang.tr("Options:"))
                        .size(12.0 * font_scale)
                        .color(answer_color)
                        .font(bold_font()),
                )
                .push(column(option_rows).spacing(2))
                .push(
                    row![
                        text(format!("{answer_label}: "))
                            .size(12.0 * font_scale)
                            .color(answer_color)
                            .font(bold_font()),
                        selectable(answer, "")
                            .size(13.0 * font_scale)
                            .style(sel_default),
                    ]
                    .spacing(4)
                    .padding(padding::top(4)),
                );
        }
    } else {
        let answer_element: Element<'a, ConversationEvent> = selectable(answer, "")
            .size(13.0 * font_scale)
            .style(sel_default)
            .into();
        answer_col = answer_col.push(
            row![
                text(format!("{answer_label}: "))
                    .size(12.0 * font_scale)
                    .color(answer_color)
                    .font(bold_font()),
                answer_element,
            ]
            .spacing(4),
        );
    }

    ask_tool_container(column![question_text, answer_col].spacing(8))
}

/// Color used for search keyword highlighting within text.
const SEARCH_HIGHLIGHT_BG: Color = Color::from_rgba(1.0, 0.92, 0.0, 0.35);

/// `Span`s of `content`, case-insensitive `query` matches highlighted.
///
/// A `&str`/`&String` is borrowed, an owned `String` moves in: the only copy
/// left is splitting an owned fragment whose match needs slicing.
fn highlighted_spans<'a>(
    content: impl IntoFragment<'a>,
    query: &str,
) -> Vec<Span<'a, (), iced::Font>> {
    let content = content.into_fragment();

    if query.trim().is_empty() {
        return vec![span(content)];
    }

    // Case-insensitive literal match: escaping keeps the query out of regex syntax.
    let Ok(re) = regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    else {
        return vec![span(content)];
    };

    let ranges: Vec<Range<usize>> = re.find_iter(&content).map(|m| m.start()..m.end()).collect();
    if ranges.is_empty() {
        return vec![span(content)];
    }

    match content {
        // Borrowed text: the pieces are slices of the caller's string.
        Cow::Borrowed(text) => split_spans(text, &ranges),
        // Owned text: splitting needs copies, so hand the pieces out as owned.
        Cow::Owned(text) => split_spans(&text, &ranges)
            .into_iter()
            .map(|piece| piece.to_static())
            .collect(),
    }
}

/// Build the span list for byte `ranges` inside `text`, highlighting each match;
/// every fragment is a slice of `text`.
fn split_spans<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<Span<'a, (), iced::Font>> {
    let mut spans = Vec::with_capacity(ranges.len() * 2 + 1);
    let mut last_end = 0;

    for range in ranges {
        if range.start > last_end {
            spans.push(span(&text[last_end..range.start]));
        }
        spans.push(span(&text[range.start..range.end]).background(SEARCH_HIGHLIGHT_BG));
        last_end = range.end;
    }

    if last_end < text.len() {
        spans.push(span(&text[last_end..]));
    }

    spans
}

/// Text with inline search keyword highlighting.
pub(super) fn highlighted_text<'a, M: Clone + 'static>(
    content: impl IntoFragment<'a>,
    query: &str,
    size: f32,
    font: Font,
) -> Element<'a, M> {
    rich_text(highlighted_spans(content, query))
        .size(size)
        .font(font)
        .into()
}

/// Selectable rich text, `query` matches highlighted (an empty query renders the
/// content plain); returns the widget so callers chain `.size()`, `.font()`,
/// `.style()`.
/// Use rich_text instead of `iced_selection::Text` so `\t` stays a tab stop, not a `.notdef` box.
pub(super) fn selectable<'a, M: Clone + 'static>(
    content: impl IntoFragment<'a>,
    query: &str,
) -> iced_selection::text::Rich<'a, (), M, iced::Theme, iced::Renderer> {
    iced_selection::rich_text(highlighted_spans(content, query))
}

/// Small monospace selectable text for tool arguments, `query` matches highlighted.
fn mono_selectable<'a, M: Clone + 'static>(
    content: impl IntoFragment<'a>,
    query: &str,
    font_scale: f32,
    style: fn(&Theme) -> SelectionStyle,
) -> Element<'a, M> {
    selectable(content, query)
        .size(12.0 * font_scale)
        .font(mono_font())
        .style(style)
        .into()
}

/// Monospace font stack for paths and code snippets.
fn mono_font() -> Font {
    Font {
        family: font::Family::Monospace,
        ..Font::DEFAULT
    }
}

/// Bold weight version of the default font.
pub(super) fn bold_font() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }
}

/// A labelled, colour-coded row used inside the edits table.
///
/// `marker` is the leading glyph (e.g. "−", "+", "⚠"), coloured with
/// `marker_color`. `content` is rendered as selectable monospace text using
/// `sel_style`, all on a `bg` background with rounded corners.
fn diff_row<'a, M: Clone + 'static>(
    marker: &'static str,
    marker_color: Color,
    content: impl IntoFragment<'a>,
    sel_style: fn(&Theme) -> SelectionStyle,
    bg: Color,
    font_scale: f32,
    search_query: &str,
) -> Element<'a, M> {
    container(
        row![
            text(marker)
                .size(13.0 * font_scale)
                .color(marker_color)
                .font(bold_font()),
            Space::new().width(6),
            mono_selectable(content, search_query, font_scale, sel_style),
        ]
        .spacing(0),
    )
    .padding([4, 8])
    .width(Fill)
    .style(move |_theme: &Theme| container::Style {
        background: Some(bg.into()),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..container::Style::default()
    })
    .into()
}

/// A plain `key: value` argument row.
fn arg_row<'a, M: Clone + 'static>(
    key: &str,
    value: impl IntoFragment<'a>,
    font_scale: f32,
    search_query: &str,
) -> Element<'a, M> {
    row![
        text(format!("{}:", key))
            .size(12.0 * font_scale)
            .color(CRABOT_TOOL_ACCENT)
            .font(bold_font()),
        Space::new().width(8),
        mono_selectable(value, search_query, font_scale, sel_default),
    ]
    .spacing(0)
    .into()
}

/// Embedded table for the `edits` argument — each edit becomes a labelled block.
fn edits_table<M: Clone + 'static>(
    edits: Vec<EditRow>,
    font_scale: f32,
    search_query: &str,
    lang: Lang,
) -> Element<'static, M> {
    let header = row![
        text(format!("{}:", tool_view::EDITS_ARG))
            .size(12.0 * font_scale)
            .color(color_muted())
            .font(bold_font()),
        Space::new().width(8),
        text(
            lang.tr("{} edit(s)")
                .replacen("{}", &edits.len().to_string(), 1),
        )
        .size(12.0 * font_scale)
        .color(color_muted()),
    ]
    .spacing(0);

    let rows: Vec<Element<'static, M>> = edits
        .into_iter()
        .enumerate()
        .flat_map(|(i, edit)| {
            let idx: Element<'static, M> = container(
                text(lang.tr("Edit #{}").replacen("{}", &(i + 1).to_string(), 1))
                    .size(11.0 * font_scale)
                    .color(color_muted()),
            )
            .padding([2, 0])
            .into();

            match edit {
                EditRow::Edit { old_text, new_text } => vec![
                    idx,
                    diff_row(
                        "−",
                        CRABOT_DANGER,
                        old_text,
                        sel_secondary,
                        color_diff_bg_del(),
                        font_scale,
                        search_query,
                    ),
                    diff_row(
                        "+",
                        CRABOT_SUCCESS,
                        new_text,
                        sel_primary,
                        color_diff_bg_add(),
                        font_scale,
                        search_query,
                    ),
                ],
                EditRow::Invalid(raw) => vec![
                    idx,
                    diff_row(
                        "⚠",
                        CRABOT_DANGER,
                        raw,
                        sel_secondary,
                        color_diff_bg_del(),
                        font_scale,
                        search_query,
                    ),
                ],
            }
        })
        .collect();

    column![header.width(Fill), column(rows).spacing(4).width(Fill)]
        .spacing(6)
        .width(Fill)
        .into()
}

/// Status colours for todo items.
const TODO_STATUS_PENDING: Color = Color::from_rgb8(0x99, 0x99, 0x99);
const TODO_STATUS_IN_PROGRESS: Color = Color::from_rgb8(0x29, 0x76, 0xFF);
const TODO_STATUS_WIDTH: f32 = 96.0;

fn todo_row<'a, M: Clone + 'static>(
    content: impl IntoFragment<'a>,
    status: &'static str,
    status_color: Color,
    font_scale: f32,
    search_query: &str,
) -> Element<'a, M> {
    row![
        container(mono_selectable(
            content,
            search_query,
            font_scale,
            sel_default
        ))
        .width(Fill)
        .padding(2),
        container(
            text(status)
                .size(12.0 * font_scale)
                .color(status_color)
                .font(bold_font())
                .wrapping(Wrapping::None),
        )
        .width(TODO_STATUS_WIDTH)
        .padding(2),
    ]
    .spacing(8)
    .into()
}

fn todo_item_row<M: Clone + 'static>(
    row: TodoRow,
    font_scale: f32,
    search_query: &str,
    lang: Lang,
) -> Element<'static, M> {
    let color = match row.state {
        TodoState::Pending => TODO_STATUS_PENDING,
        TodoState::InProgress => TODO_STATUS_IN_PROGRESS,
        TodoState::Completed => CRABOT_SUCCESS,
        TodoState::Invalid => CRABOT_DANGER,
    };
    let status = lang.tr(row.state.label_key());
    todo_row(row.content, status, color, font_scale, search_query)
}

/// Embedded table for the `items` argument of the `todo` tool.
fn todo_table<M: Clone + 'static>(
    items: Vec<TodoRow>,
    font_scale: f32,
    search_query: &str,
    lang: Lang,
) -> Element<'static, M> {
    let col_header = row![
        container(
            text(lang.tr("Text"))
                .size(11.0 * font_scale)
                .color(color_muted())
                .font(bold_font()),
        )
        .width(Fill)
        .padding(2),
        container(
            text(lang.tr("Status"))
                .size(11.0 * font_scale)
                .color(color_muted())
                .font(bold_font())
                .wrapping(Wrapping::None),
        )
        .width(TODO_STATUS_WIDTH)
        .padding(2),
    ]
    .spacing(8);

    let mut elements: Vec<Element<'static, M>> = vec![col_header.into()];
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            elements.push(
                container(Space::new().width(Fill).height(1.0))
                    .style(|_theme: &Theme| container::Style {
                        background: Some(color_tool_content_border().into()),
                        ..container::Style::default()
                    })
                    .into(),
            );
        }
        elements.push(todo_item_row(item, font_scale, search_query, lang));
    }

    container(column(elements).spacing(0).width(Fill))
        .padding(4)
        .style(|_theme: &Theme| tool_box_style(None, color_tool_content_border(), 4.0))
        .width(Fill)
        .into()
}

/// Render [`tool_view`] argument rows as elements for the live view.
pub(super) fn arg_rows<M: Clone + 'static>(
    rows: Vec<ArgRow>,
    font_scale: f32,
    search_query: &str,
    lang: Lang,
) -> Vec<Element<'static, M>> {
    rows.into_iter()
        .map(|row| arg_row_element(row, font_scale, search_query, lang))
        .collect()
}

/// Render one [`ArgRow`] as an iced element.
fn arg_row_element<M: Clone + 'static>(
    row: ArgRow,
    font_scale: f32,
    search_query: &str,
    lang: Lang,
) -> Element<'static, M> {
    match row {
        ArgRow::Text { key, value } => arg_row(&key, value, font_scale, search_query),
        ArgRow::OffsetLimit { offset, limit } => {
            offset_limit_row(&offset, &limit, font_scale, search_query)
        }
        ArgRow::Edits(edits) => edits_table(edits, font_scale, search_query, lang),
        ArgRow::Todo(rows) => todo_table(rows, font_scale, search_query, lang),
    }
}

/// The combined `offset`/`limit` row (e.g. the `read` tool).
fn offset_limit_row<M: Clone + 'static>(
    offset: &str,
    limit: &str,
    font_scale: f32,
    search_query: &str,
) -> Element<'static, M> {
    let combined = format!("offset: {offset}  limit: {limit}");
    container(mono_selectable(
        combined,
        search_query,
        font_scale,
        sel_secondary,
    ))
    .padding([4, 8])
    .style(tool_content_box)
    .into()
}

/// Live-render window; the final result replaces this view on finish.
const STREAMING_RENDER_WINDOW: usize = 16 * 1024;

/// Last `window` bytes of `s` on a UTF-8 boundary, plus the bytes skipped.
fn tail_window(s: &str, window: usize) -> (&str, usize) {
    if s.len() <= window {
        return (s, 0);
    }
    let start = s.floor_char_boundary(s.len() - window);
    (&s[start..], start)
}

/// Live view of a running tool's output buffer (tail window only, mono text).
pub(super) fn streaming_result_text<'a, M: Clone + 'static>(
    buffer: &'a str,
    font_scale: f32,
    lang: Lang,
) -> Element<'a, M> {
    let (shown, skipped) = tail_window(buffer, STREAMING_RENDER_WINDOW);
    let mut body = column![
        text(lang.tr("Running…"))
            .size(11.0 * font_scale)
            .color(CRABOT_TOOL_ACCENT)
            .font(bold_font())
    ]
    .spacing(4)
    .width(Fill);
    if skipped > 0 {
        body = body.push(
            text(
                lang.tr("… {skipped} bytes of earlier output hidden …")
                    .replacen("{skipped}", &skipped.to_string(), 1),
            )
            .size(11.0 * font_scale)
            .color(color_muted())
            .font(mono_font()),
        );
    }
    body = body.push(text(shown).size(13.0 * font_scale).font(mono_font()));
    container(body)
        .padding([8, 10])
        .style(tool_result_box(
            color_tool_content_bg(),
            color_tool_content_border(),
        ))
        .into()
}

/// Tool result text (success or error).
pub(super) fn result_text<'a, M: Clone + 'static>(
    result: &'a Result<String, String>,
    font_scale: f32,
    search_query: &str,
    lang: Lang,
) -> Element<'a, M> {
    let display: &str = result
        .as_ref()
        .map(|s| s.as_str())
        .unwrap_or_else(|e| e.as_str());
    let is_ok = result.is_ok();
    let accent = if is_ok {
        CRABOT_TOOL_ACCENT
    } else {
        CRABOT_DANGER
    };
    let fill = if is_ok {
        color_tool_content_bg()
    } else {
        color_diff_bg_del()
    };
    let border = if is_ok {
        color_tool_content_border()
    } else {
        accent.scale_alpha(0.4)
    };

    let body: Element<'a, M> = selectable(display, search_query)
        .size(13.0 * font_scale)
        .font(mono_font())
        .style(move |theme: &Theme| SelectionStyle {
            color: Some(color_text(theme)),
            selection: accent,
        })
        .into();

    container(
        column![
            text(lang.tr(if is_ok { "Result" } else { "Error" }))
                .size(11.0 * font_scale)
                .color(accent)
                .font(bold_font()),
            body,
        ]
        .spacing(4)
        .width(Fill),
    )
    .padding([8, 10])
    .style(tool_result_box(fill, border))
    .into()
}
