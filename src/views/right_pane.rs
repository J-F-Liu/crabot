use iced::{
    Alignment, Color, Element, Fill, Font, Length, alignment, font, padding,
    widget::{Space, button, column, container, row, rule, scrollable, text, toggler},
};
use iced_selection::Text as SelectableText;

use super::styles::{pane_side, primary_button, primary_toggler, sel_primary};
use super::theme::thin_vertical;
use crate::RightPaneEvent;
use crate::app::SessionTab;
use crabot::model::{ModelConfig, TokenAmount};
use crabot::tools::todo::{TodoItem, TodoStatus};

/// Label-value row with the value right-aligned via a fill spacer.
fn token_row<'a>(label: &'a str, value: String) -> Element<'a, RightPaneEvent> {
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

fn section_header<'a>(title: &'a str) -> Element<'a, RightPaneEvent> {
    text(title)
        .size(14)
        .font(Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        })
        .into()
}

/// Build the todo-list section, returning `None` when the list is empty.
fn todo_section<'a>(todo_items: &'a [TodoItem]) -> Option<Element<'a, RightPaneEvent>> {
    if todo_items.is_empty() {
        return None;
    }
    let rows: Vec<Element<'_, RightPaneEvent>> = todo_items
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

pub(crate) fn right_pane<'a>(
    pane_width: f32,
    model: Option<&ModelConfig>,
    tab: &'a SessionTab,
    show_restart: bool,
    dark_mode: bool,
) -> Element<'a, RightPaneEvent> {
    let usage = &tab.last_usage;
    let session = &tab.session;
    let todo_items: &[TodoItem] = &tab.todo_items;
    let context_window = model.map(|m| m.context_window);
    let mut col = column![].spacing(8);
    let token_amount = TokenAmount::from_genai(usage);

    col = col
        .push(rule::horizontal(1))
        .push(section_header("Context window"))
        .push(token_row(
            "Prompt tokens:",
            format!("{}", token_amount.prompt),
        ))
        .push(token_row(
            "Cached tokens:",
            format!("{}", token_amount.cache_read + token_amount.cache_write),
        ));

    if let Some(cw) = context_window.filter(|&cw| cw > 0) {
        let pct = token_amount.window_used(cw);
        col = col
            .push(token_row("Window size:", format!("{cw}")))
            .push(token_row("Fill ratio:", format!("{:.1}%", pct)));
    }

    // ── cumulative token usage and cost ───────────────────────────────────────────
    col = col
        .push(rule::horizontal(1))
        .push(section_header("Token Usage"))
        .push(token_row(
            "Input tokens:",
            format!("{}", session.tokens.input),
        ))
        .push(token_row(
            "Output tokens:",
            format!("{}", session.tokens.output),
        ))
        .push(token_row(
            "Cache read:",
            format!("{}", session.tokens.cache_read),
        ));
    if session.tokens.cache_write > 0 {
        col = col.push(token_row(
            "Cache write:",
            format!("{}", session.tokens.cache_write),
        ));
    }
    col = col
        .push(token_row("Session cost:", session.formatted_cost()))
        .push(rule::horizontal(1))
        .push(token_row("Num Requests:", session.requests.to_string()));

    // ── todo items ──
    if let Some(section) = todo_section(todo_items) {
        col = col.push(section);
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
        let files_col = column(files).spacing(2);
        col = col
            .push(rule::horizontal(1))
            .push(section_header("Modified Files"))
            .push(files_col);
    }

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

    let mut footer = column![].spacing(0);
    if show_restart {
        footer = footer.push(
            container(
                button(text("Restart").size(14))
                    .on_press(RightPaneEvent::Restart)
                    .style(primary_button)
                    .width(Length::Shrink),
            )
            .width(Fill)
            .align_x(alignment::Horizontal::Center)
            .padding(padding::bottom(12)),
        );
    }

    let body: Element<'_, RightPaneEvent> =
        scrollable(container(col.padding(padding::all(20).left(16).top(8))))
            .direction(thin_vertical())
            .height(Fill)
            .into();

    container(column![theme_toggle, body, footer,])
        .width(Length::Fixed(pane_width))
        .height(Fill)
        .style(pane_side)
        .into()
}
