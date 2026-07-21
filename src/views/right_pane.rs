use iced::{
    Color, Element, Fill, Font, Length, alignment, font, padding,
    widget::{Space, button, column, container, rule, scrollable, text},
};
use iced_selection::Text as SelectableText;

use super::settings::currency_symbol;
use super::styles::{pane_side, primary_button, sel_primary};
use super::theme::thin_vertical;
use crate::Message;
use crabot::model::{Model, TokenAmount};
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

/// Format cost value with currency symbol prefix.
/// Small amounts get 4 decimal places, larger amounts get 2 decimal places.
fn format_cost(amount: f64, sym: &str) -> String {
    if amount < 0.01 {
        format!("{sym}{:.4}", amount)
    } else {
        format!("{sym}{:.2}", amount)
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
    model: Option<&Model>,
    usage: &genai::chat::Usage,
    amount: &TokenAmount,
    cost: f64,
    modified_files: &'a [String],
    show_restart: bool,
    todo_items: &'a [TodoItem],
) -> Element<'a, Message> {
    let context_window = model.map(|m| m.context_window);
    let currency = model.map(|m| m.cost.currency.as_str()).unwrap_or("USD");
    let mut col = column![].spacing(8);
    let usage_amount = TokenAmount::from_genai(usage);

    col = col
        .push(rule::horizontal(1))
        .push(section_header("Context window"))
        .push(token_row(
            "Prompt tokens:",
            format!("{}", usage_amount.input),
        ))
        .push(token_row(
            "Cached tokens:",
            format!("{}", usage_amount.cached),
        ));
    if usage_amount.cache_write > 0 {
        col = col.push(token_row(
            "Cache write:",
            format!("{}", usage_amount.cache_write),
        ));
    }

    if let Some(cw) = context_window.filter(|&cw| cw > 0) {
        let pct = usage_amount.window_used(cw);
        col = col
            .push(token_row("window size:", format!("{cw}")))
            .push(token_row("Window used:", format!("{:.1}%", pct)));
    }

    // ── cumulative token usage and cost ───────────────────────────────────────────
    col = col
        .push(rule::horizontal(1))
        .push(section_header("Token Usage"))
        .push(token_row("Input tokens:", format!("{}", amount.input)))
        .push(token_row("Cached tokens:", format!("{}", amount.cached)));
    if amount.cache_write > 0 {
        col = col.push(token_row("Cache write:", format!("{}", amount.cache_write)));
    }
    col = col
        .push(token_row("Output tokens:", format!("{}", amount.output)))
        .push(token_row(
            "Session cost:",
            format_cost(cost, currency_symbol(currency)),
        ));

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
