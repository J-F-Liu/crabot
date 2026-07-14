use iced::{
    Color, Element, Fill, Font, Length, alignment, font, padding,
    widget::{Space, button, column, container, rule, scrollable, text},
};
use iced_selection::Text as SelectableText;

use super::styles::{pane_side, primary_button, sel_primary};
use super::theme::thin_vertical;
use crate::Message;
use crabot::model::TokenAmount;
use crabot::tools::todo::{TodoItem, TodoStatus};

/// Label-value row with the value right-aligned via a fill spacer.
fn token_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    let mono = Font {
        family: font::Family::Monospace,
        ..Font::DEFAULT
    };
    iced::widget::row![
        text(label).size(16),
        Space::new().width(Length::Fill),
        text(value).size(16).font(mono),
    ]
    .into()
}

/// Format cost value.
/// Small amounts get 4 decimal places, larger amounts get 2 decimal places.
fn format_cost(amount: f64) -> String {
    if amount < 0.01 {
        format!("{:.4}", amount)
    } else {
        format!("{:.2}", amount)
    }
}

/// Bold section title used throughout the right pane.
fn section_header<'a>(title: &'a str) -> Element<'a, Message> {
    text(title)
        .size(14)
        .font(Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        })
        .into()
}

/// Build the todo-list section, returning `None` when the list is empty.
fn todo_section<'a>(todo_items: &'a [TodoItem]) -> Option<Element<'a, Message>> {
    if todo_items.is_empty() {
        return None;
    }
    let rows: Vec<Element<'_, Message>> = todo_items
        .iter()
        .map(|item| {
            let indent = item.depth as u16 * 16;
            let (icon, color) = match item.status {
                TodoStatus::Pending => ("⏳", Color::from_rgb(0.7, 0.7, 0.7)),
                TodoStatus::InProgress => ("🔄", Color::from_rgb(0.3, 0.6, 1.0)),
                TodoStatus::Completed => ("✅", Color::from_rgb(0.4, 0.7, 0.4)),
            };
            container(text(format!("{icon} {}", item.text)).size(14).color(color))
                .padding(padding::left(indent as f32))
                .into()
        })
        .collect();
    Some(
        column![
            rule::horizontal(1),
            section_header("Todo List"),
            column(rows).spacing(3),
        ]
        .spacing(8)
        .into(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn right_pane<'a>(
    pane_width: f32,
    context_window: Option<u32>,
    usage: &genai::chat::Usage,
    amount: &TokenAmount,
    cost: f64,
    modified_files: &'a [String],
    show_restart: bool,
    todo_items: &'a [TodoItem],
) -> Element<'a, Message> {
    let mut col = column![].spacing(8);

    let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
    let cached_tokens = usage
        .prompt_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens)
        .unwrap_or(0);

    col = col
        .push(rule::horizontal(1))
        .push(section_header("Context window"))
        .push(token_row("Prompt tokens:", format!("{prompt_tokens}")))
        .push(token_row("Cached tokens:", format!("{cached_tokens}")));

    if let Some(cw) = context_window.filter(|&cw| cw > 0) {
        let pct = (prompt_tokens as f64) * 100.0 / cw as f64;
        col = col
            .push(token_row("window size:", format!("{cw}")))
            .push(token_row("Window used:", format!("{:.1}%", pct)));
    }

    // ── cumulative token usage and cost ───────────────────────────────────────────
    col = col
        .push(rule::horizontal(1))
        .push(section_header("Token Usage"))
        .push(token_row("Input tokens:", format!("{}", amount.input)))
        .push(token_row("Cached tokens:", format!("{}", amount.cached)))
        .push(token_row("Output tokens:", format!("{}", amount.output)))
        .push(token_row("Session cost:", format_cost(cost)));

    // ── todo items ──
    if let Some(section) = todo_section(todo_items) {
        col = col.push(section);
    }

    // ── modified files ──
    if !modified_files.is_empty() {
        let files: Vec<Element<'_, Message>> = modified_files
            .iter()
            .map(|p| {
                container(SelectableText::new(p.as_str()).size(13).style(sel_primary))
                    .padding([1, 0])
                    .into()
            })
            .collect();
        let files_col = column(files).spacing(2);
        col = col
            .push(rule::horizontal(1))
            .push(section_header("Modified Files"))
            .push(files_col);
    }

    if show_restart {
        col = col.push(Space::new().height(Fill)).push(
            container(
                button(text("Restart").size(14))
                    .on_press(Message::Restart)
                    .style(primary_button)
                    .width(Length::Shrink),
            )
            .width(Fill)
            .align_x(alignment::Horizontal::Center),
        );
    }

    container(
        scrollable(container(col.padding(padding::all(20).left(16)))).direction(thin_vertical()),
    )
    .width(Length::Fixed(pane_width))
    .height(Fill)
    .style(pane_side)
    .into()
}
