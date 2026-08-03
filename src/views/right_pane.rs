use std::borrow::Cow;

use iced::{
    Alignment, Color, Element, Fill, Font, Length, alignment, font, padding,
    widget::{Space, button, column, container, row, rule, scrollable, text, toggler},
};
use iced_selection::Text as SelectableText;

use super::styles::{pane_side, primary_button, primary_toggler, sel_primary};
use super::theme::thin_vertical;
use crate::RightPaneEvent;
use crate::app::SessionTab;
use crabot::model::ModelConfig;
use crabot::tools::todo::{TodoItem, TodoStatus};

const MONO: Font = Font {
    family: font::Family::Monospace,
    ..Font::DEFAULT
};
const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

/// Label-value row with the value right-aligned via a fill spacer.
fn token_row<'a>(label: &'a str, value: String) -> Element<'a, RightPaneEvent> {
    iced::widget::row![
        text(label).size(16),
        Space::new().width(Length::Fill),
        text(value).size(16).font(MONO),
    ]
    .into()
}

fn section_header(title: Cow<'static, str>) -> Element<'static, RightPaneEvent> {
    text(title).size(14).font(BOLD).into()
}

/// Build the todo-list section, returning `None` when the list is empty.
fn todo_section(todo_items: &[TodoItem]) -> Option<Element<'static, RightPaneEvent>> {
    if todo_items.is_empty() {
        return None;
    }
    let rows: Vec<Element<'static, RightPaneEvent>> = todo_items
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
            section_header(Cow::Borrowed("Todo List")),
            column(rows).spacing(3),
        ]
        .spacing(8)
        .into(),
    )
}

pub(crate) fn right_pane<'a>(
    pane_width: f32,
    model: Option<&ModelConfig>,
    tab: &'a SessionTab,
    dark_mode: bool,
) -> Element<'a, RightPaneEvent> {
    let token_amount = &tab.latest_tokens;
    let session = &tab.session;
    let todo_items: Vec<TodoItem> = tab
        .todo_items
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    let context_window = model.map(|m| m.context_window);
    let mut items: Vec<Element<'_, RightPaneEvent>> = Vec::new();

    // ── context window ──
    items.push(rule::horizontal(1).into());
    let header = match context_window.filter(|&cw| cw > 0) {
        Some(cw) => format!("Context window ({cw})"),
        None => "Context window".to_string(),
    };
    items.push(section_header(Cow::from(header)));
    items.push(token_row(
        "Prompt tokens:",
        format!("{}", token_amount.prompt),
    ));
    items.push(token_row(
        "Cached tokens:",
        format!("{}", token_amount.cache_read + token_amount.cache_write),
    ));
    if let Some(cw) = context_window.filter(|&cw| cw > 0) {
        let cfr = token_amount.context_fill_ratio(cw);
        items.push(token_row("Fill ratio:", format!("{:.1}%", cfr)));
    }

    // ── cumulative token usage and cost ──
    items.push(rule::horizontal(1).into());
    items.push(section_header(Cow::Borrowed("Token Usage")));
    items.push(token_row(
        "Input tokens:",
        format!("{}", session.tokens.input),
    ));
    items.push(token_row(
        "Output tokens:",
        format!("{}", session.tokens.output),
    ));
    items.push(token_row(
        "Cache read:",
        format!("{}", session.tokens.cache_read),
    ));
    if session.tokens.cache_write > 0 {
        items.push(token_row(
            "Cache write:",
            format!("{}", session.tokens.cache_write),
        ));
    }
    items.push(token_row("Session cost:", session.formatted_cost()));
    items.push(rule::horizontal(1).into());
    items.push(token_row("Num Requests:", session.requests.to_string()));
    if !session.updated_at.is_empty() {
        items.push(token_row("Last Response:", session.updated_at_time()));
    }

    // ── todo items ──
    if let Some(section) = todo_section(&todo_items) {
        items.push(section);
    }

    // ── modified files ──
    if !session.modified_files.is_empty() {
        let files: Vec<Element<'_, RightPaneEvent>> = session
            .modified_files
            .iter()
            .map(|p| {
                container(SelectableText::new(p.as_str()).size(13).style(sel_primary))
                    .padding([1, 0])
                    .into()
            })
            .collect();
        items.push(rule::horizontal(1).into());
        items.push(section_header(Cow::Borrowed("Modified Files")));
        items.push(column(files).spacing(2).into());
    }

    let col = column(items).spacing(8);

    let theme_toggle = row![
        text("Dark theme").size(14),
        Space::new().width(Fill),
        toggler(dark_mode)
            .on_toggle(RightPaneEvent::ToggleTheme)
            .style(primary_toggler)
            .size(18),
    ]
    .align_y(Alignment::Center)
    .padding(padding::top(12).right(16).left(16));

    let footer = container(
        button(text("Restart").size(14))
            .on_press(RightPaneEvent::Restart)
            .style(primary_button)
            .width(Length::Shrink),
    )
    .width(Fill)
    .align_x(alignment::Horizontal::Center)
    .padding(padding::bottom(12));

    let body: Element<'_, RightPaneEvent> =
        scrollable(container(col.padding(padding::all(20).left(16).top(8))))
            .direction(thin_vertical())
            .height(Fill)
            .into();

    container(column![theme_toggle, body, footer])
        .width(Length::Fixed(pane_width))
        .height(Fill)
        .style(pane_side)
        .into()
}
