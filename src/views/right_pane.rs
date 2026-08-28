use std::borrow::Cow;

use iced::{
    Alignment, Color, Element, Fill, Font, Length, alignment, font, padding,
    widget::{Space, button, column, container, row, rule, scrollable, text, toggler},
};
use iced_selection::Text as SelectableText;

use super::icons;
use super::styles::{pane_side, primary_button, primary_toggler, secondary_button, sel_primary};
use super::theme::{CRABOT_DANGER, thin_horizontal, thin_vertical};
use crate::RightPaneEvent;
use crate::acp::AcpState;
use crate::app::SessionTab;
use crabot::model::ModelConfig;
use crabot::tools::{
    process,
    todo::{TodoItem, TodoStatus},
};

const MONO: Font = Font {
    family: font::Family::Monospace,
    ..Font::DEFAULT
};

/// Identifies a collapsible right-pane section.
#[derive(Clone, Copy)]
pub(crate) enum PaneSection {
    ContextWindow,
    TokenUsage,
    Processes,
    AccessedFiles,
    ModifiedFiles,
}

/// Expand/collapse state of the right-pane sections, shared across all
/// session tabs so switching sessions keeps the current layout.
#[derive(Debug)]
pub(crate) struct PaneSections {
    pub(crate) context_window: bool,
    pub(crate) token_usage: bool,
    pub(crate) processes: bool,
    pub(crate) accessed_files: bool,
    pub(crate) modified_files: bool,
}

impl Default for PaneSections {
    fn default() -> Self {
        Self {
            context_window: true,
            token_usage: true,
            processes: true,
            accessed_files: true,
            modified_files: true,
        }
    }
}

const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};

/// `label` with a right-aligned trailing widget (used for token rows and Revert buttons).
fn with_trailing<'a>(
    label: impl Into<Element<'a, RightPaneEvent>>,
    trailing: impl Into<Element<'a, RightPaneEvent>>,
) -> Element<'a, RightPaneEvent> {
    row![
        label.into(),
        Space::new().width(Length::Fill),
        trailing.into()
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Label-value row with the value right-aligned via a fill spacer.
fn token_row<'a>(label: &'a str, value: String) -> Element<'a, RightPaneEvent> {
    with_trailing(text(label).size(16), text(value).size(16).font(MONO))
}

fn section_header(title: Cow<'static, str>) -> Element<'static, RightPaneEvent> {
    text(title).size(14).font(BOLD).into()
}

/// Small text button used for the Revert / Revert All actions.
fn revert_button(label: &'static str, event: RightPaneEvent) -> Element<'static, RightPaneEvent> {
    button(text(label).size(12))
        .on_press(event)
        .style(secondary_button)
        .padding([2, 8])
        .into()
}

/// Section header with a trailing expand/collapse chevron.
fn collapsible_header(
    title: Cow<'static, str>,
    expanded: bool,
    section: PaneSection,
) -> Element<'static, RightPaneEvent> {
    with_trailing(section_header(title), collapse_toggle(expanded, section))
}

/// Expand/collapse chevron button used by collapsible sections.
fn collapse_toggle(expanded: bool, section: PaneSection) -> Element<'static, RightPaneEvent> {
    let (icon, tip) = if expanded {
        (&icons::CHEVRONS_DOWN, "Collapse")
    } else {
        (&icons::CHEVRONS_RIGHT, "Expand")
    };
    icons::icon_action(icon, tip, RightPaneEvent::ToggleSection(section))
}

/// Selectable row for a single file path.
fn file_row<'a>(path: &'a str) -> Element<'a, RightPaneEvent> {
    container(SelectableText::new(path).size(13).style(sel_primary))
        .padding([1, 0])
        .into()
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

/// Running-processes rows: one per process tagged with its owning session,
/// horizontally scrollable when wider than the pane.
fn process_section(processes: &[process::RunningProcess]) -> Element<'static, RightPaneEvent> {
    let rows: Vec<Element<'static, RightPaneEvent>> = processes
        .iter()
        .map(|p| {
            // Processes started outside the LLM loop (tool playground) carry
            // no owner tag; tab numbers match the tab-bar "Session N" labels.
            let owner = p
                .tab
                .map_or_else(String::new, |n| format!("Session {n} · "));
            container(
                text(format!("{owner}{}: {}", p.pid, p.command))
                    .size(14)
                    .font(MONO),
            )
            .padding([1, 0])
            .into()
        })
        .collect();
    // Shrink-width content: the viewport scrolls only when a row overflows.
    scrollable(column(rows).spacing(3))
        .direction(thin_horizontal())
        .width(Length::Fill)
        .spacing(2)
        .into()
}

pub(crate) fn right_pane<'a>(
    pane_width: f32,
    model: Option<&ModelConfig>,
    tab: &'a SessionTab,
    sections: &PaneSections,
    processes: &[process::RunningProcess],
    dark_mode: bool,
    acp: &AcpState,
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
    let cw = context_window.filter(|&cw| cw > 0);
    let expanded = sections.context_window;
    // Collapsed shows only the fill ratio; expanded shows the raw size.
    let header = match cw {
        Some(cw) if !expanded => {
            format!(
                "Context window: {:.1}%",
                token_amount.context_fill_ratio(cw)
            )
        }
        Some(cw) => format!("Context window ({cw})"),
        None => "Context window".to_string(),
    };
    items.push(collapsible_header(
        Cow::from(header),
        expanded,
        PaneSection::ContextWindow,
    ));
    if expanded {
        items.push(token_row("Prompt tokens:", token_amount.prompt.to_string()));
        items.push(token_row(
            "Cached tokens:",
            (token_amount.cache_read + token_amount.cache_write).to_string(),
        ));
        if let Some(cw) = cw {
            items.push(token_row(
                "Fill ratio:",
                format!("{:.1}%", token_amount.context_fill_ratio(cw)),
            ));
        }
    }

    // ── cumulative token usage and cost ──
    items.push(rule::horizontal(1).into());
    let expanded = sections.token_usage;
    items.push(collapsible_header(
        if expanded {
            Cow::Borrowed("Token Usage")
        } else {
            // Collapsed shows only the session cost.
            Cow::from(format!("Token Usage: {}", session.formatted_cost()))
        },
        expanded,
        PaneSection::TokenUsage,
    ));
    if expanded {
        items.push(token_row("Input tokens:", session.tokens.input.to_string()));
        items.push(token_row(
            "Output tokens:",
            session.tokens.output.to_string(),
        ));
        items.push(token_row(
            "Cache read:",
            session.tokens.cache_read.to_string(),
        ));
        if session.tokens.cache_write > 0 {
            items.push(token_row(
                "Cache write:",
                session.tokens.cache_write.to_string(),
            ));
        }
        items.push(token_row("Session cost:", session.formatted_cost()));
        items.push(rule::horizontal(1).into());
        items.push(token_row("Num Requests:", session.requests.to_string()));
        if !session.updated_at.is_empty() {
            items.push(token_row("Last Response:", session.updated_at_time()));
        }
    }

    // ── todo items ──
    if let Some(section) = todo_section(&todo_items) {
        items.push(section);
    }

    // ── running processes ──
    if !processes.is_empty() {
        let expanded = sections.processes;
        items.push(rule::horizontal(1).into());
        items.push(collapsible_header(
            if expanded {
                Cow::Borrowed("Running Processes")
            } else {
                // Collapsed shows only the live count.
                Cow::from(format!("Running Processes: {}", processes.len()))
            },
            expanded,
            PaneSection::Processes,
        ));
        if expanded {
            items.push(process_section(processes));
        }
    }

    // ── accessed files ──
    if !session.accessed_files.is_empty() {
        let expanded = sections.accessed_files;
        items.push(rule::horizontal(1).into());
        items.push(collapsible_header(
            Cow::Borrowed("Accessed Files"),
            expanded,
            PaneSection::AccessedFiles,
        ));
        if expanded {
            let files: Vec<_> = session.accessed_files.iter().map(|p| file_row(p)).collect();
            items.push(column(files).spacing(2).into());
        }
    }

    // ── modified files ──
    if !session.modified_files.is_empty() {
        let expanded = sections.modified_files;
        items.push(rule::horizontal(1).into());
        // Revert All is available only when snapshots exist and the session is
        // idle; then the list is always shown expanded.
        let show_revert_all = !tab.snapshot_files.is_empty() && !tab.running();
        let trailing: Element<RightPaneEvent> = if show_revert_all {
            revert_button("Revert All", RightPaneEvent::RevertAll)
        } else {
            collapse_toggle(expanded, PaneSection::ModifiedFiles)
        };
        items.push(with_trailing(
            section_header(Cow::Borrowed("Modified Files")),
            trailing,
        ));
        if expanded || show_revert_all {
            let can_revert = |p: &String| tab.snapshot_files.contains(p) && !tab.running();
            let files: Vec<_> = session
                .modified_files
                .iter()
                .map(|p| {
                    if can_revert(p) {
                        with_trailing(
                            file_row(p),
                            revert_button("Revert", RightPaneEvent::RevertFile(p.clone())),
                        )
                    } else {
                        file_row(p)
                    }
                })
                .collect();
            items.push(column(files).spacing(2).into());
        }
        if let Some(err) = &tab.modified_files_error {
            items.push(
                container(with_trailing(
                    text(err.as_str()).size(12).color(CRABOT_DANGER),
                    revert_button("\u{00d7}", RightPaneEvent::DismissRevertError),
                ))
                .padding(padding::top(2))
                .into(),
            );
        }
    }

    let col = column(items).spacing(8);

    // ── top toggles: ACP Server left of Dark theme, plus a status line ──
    fn toggle_group(
        label: &'static str,
        active: bool,
        on_toggle: impl Fn(bool) -> RightPaneEvent + 'static,
    ) -> Element<'static, RightPaneEvent> {
        row![
            text(label).size(13),
            Space::new().width(6),
            toggler(active)
                .on_toggle(on_toggle)
                .style(primary_toggler)
                .size(14),
        ]
        .align_y(Alignment::Center)
        .into()
    }
    let toggles = row![
        toggle_group("ACP Server", acp.enabled, RightPaneEvent::ToggleAcpServer),
        Space::new().width(Fill),
        toggle_group("Dark theme", dark_mode, RightPaneEvent::ToggleTheme),
    ]
    .align_y(Alignment::Center)
    .padding(padding::top(12).right(16).left(16));
    let mut header = column![toggles].spacing(4);
    if acp.enabled {
        let (status, color) = if acp.stdio {
            // Host-spawned transport — no HTTP address to show.
            (
                "stdio (host-spawned)".to_string(),
                Color::from_rgb(0.5, 0.6, 0.8),
            )
        } else {
            match (&acp.error, acp.running) {
                (Some(error), _) => (error.clone(), CRABOT_DANGER),
                (None, true) => (
                    format!("http://{}", acp.addr),
                    Color::from_rgb(0.4, 0.7, 0.4),
                ),
                (None, false) => ("Starting…".to_string(), Color::from_rgb(0.6, 0.6, 0.6)),
            }
        };
        header = header.push(
            container(text(status).size(12).color(color)).padding(padding::left(16).right(16)),
        );
    }

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

    container(column![header, body, footer])
        .width(Length::Fixed(pane_width))
        .height(Fill)
        .style(pane_side)
        .into()
}
