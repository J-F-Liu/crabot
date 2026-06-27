use iced::{
    Element, Fill, Font, Length, alignment, font,
    widget::{Space, button, column, container, rule, scrollable, text},
};
use iced_selection::Text as SelectableText;

use super::styles::{pane_side, primary_button, sel_primary};
use crate::Message;
use crate::model::TokenAmount;

/// Label-value row with the value right-aligned via a fill spacer.
fn token_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    iced::widget::row![
        text(label).size(16),
        Space::new().width(Length::Fill),
        text(value).size(16),
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

pub(crate) fn right_pane<'a>(
    pane_width: f32,
    context_window: Option<u64>,
    usage: &genai::chat::Usage,
    amount: &TokenAmount,
    cost: f64,
    modified_files: &'a [String],
    show_restart: bool,
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
        .push(text("Context window").size(14).font(Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        }))
        .push(token_row("Prompt tokens:", format!("{prompt_tokens}")))
        .push(token_row("Cached tokens:", format!("{cached_tokens}")));

    if let Some(cw) = context_window.filter(|&cw| cw > 0) {
        let pct = ((prompt_tokens as u64) * 100).checked_div(cw).unwrap_or(0);
        col = col
            .push(token_row("window size:", format!("{cw}")))
            .push(token_row("Window used:", format!("{pct}%")));
    }

    // ── cumulative token usage and cost ───────────────────────────────────────────
    col = col
        .push(rule::horizontal(1))
        .push(text("Token Usage").size(14).font(Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        }))
        .push(token_row("Input tokens:", format!("{}", amount.input)))
        .push(token_row("Cached tokens:", format!("{}", amount.cached)))
        .push(token_row("Output tokens:", format!("{}", amount.output)))
        .push(token_row("Session cost:", format_cost(cost)));

    // ── modified files ──
    if !modified_files.is_empty() {
        let files: Vec<Element<'_, Message>> = modified_files
            .iter()
            .map(|p| {
                container(SelectableText::new(p.as_str()).size(12).style(sel_primary))
                    .padding([1, 0])
                    .into()
            })
            .collect();
        let files_col = column(files).spacing(2);
        col = col
            .push(rule::horizontal(1))
            .push(text("Modified Files").size(14).font(Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }))
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

    container(scrollable(container(col.padding(20))))
        .width(Length::Fixed(pane_width))
        .height(Fill)
        .style(pane_side)
        .into()
}
