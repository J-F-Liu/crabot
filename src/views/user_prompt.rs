use iced::{
    Alignment, Element, Fill, Length, Padding, Widget, padding,
    widget::{
        button, checkbox, column, container, rich_text, row, scrollable, span, text, text::Wrapping,
    },
};

use crate::views::theme::color_muted;

use super::styles::{
    chip_style, icon_button_style, menu_container_style, menu_item_style, sel_default,
};
use super::system_prompt::expandable_header;
use crate::WORKSPACE_TREE;
use crate::app::FileTreePane;
use crate::widgets::popup_menu::PopupMenu;
use crate::widgets::textarea::TextArea;
use crate::{FocusedTarget, PromptEvent};
use crabot::i18n::Lang;
use crabot::user::{AttachmentKind, UserAttachment, WorkMode};

// The prompt editor's state, work mode, recipes, attachments, files, and UI language.
#[allow(clippy::too_many_arguments)]
pub(crate) fn user_prompt_view<'a>(
    user_prompt: &'a TextArea,
    workmode: WorkMode,
    workmode_enabled: bool,
    prompt_recipes: &'a [String],
    recipe_dropdown_expanded: bool,
    attachments: &'a [UserAttachment],
    attachment_menu_expanded: bool,
    files: &'a FileTreePane,
    workspace_set: bool,
    lang: Lang,
) -> Element<'a, PromptEvent> {
    // Blank input sends nothing, so the button shows that instead of failing silently.
    let can_send = !user_prompt.is_blank() || !attachments.is_empty();

    let tab_bar = crate::views::tab_bar(WorkMode::all().iter().map(|mode| {
        (
            mode.label(lang).to_string(),
            *mode == workmode,
            PromptEvent::SelectWorkMode(*mode),
        )
    }));

    // ── Recipe dropdown ──────────────────────────────────────────
    let overlay: Element<'_, PromptEvent> = container(
        scrollable(
            column(prompt_recipes.iter().enumerate().map(|(i, recipe)| {
                button(text(recipe.clone()).size(13))
                    .on_press(PromptEvent::SelectRecipe(i))
                    .padding([4, 10])
                    .width(Length::Fill)
                    .style(menu_item_style)
                    .boxed()
            }))
            .padding([4, 0]),
        )
        .height(Length::Fill),
    )
    .style(menu_container_style)
    .boxed();

    let recipe_dropdown = dropdown(
        lang.tr("Recipes ▾"),
        recipe_dropdown_expanded,
        PromptEvent::ToggleRecipeDropdown,
        PromptEvent::DismissRecipeDropdown,
        overlay,
        360.0,
        Length::Fixed(180.0),
    );

    // ── Attachment dropdown ───────────────────────────────────────
    let overlay: Element<'_, PromptEvent> = container(
        column![
            button(text(lang.tr("Attach text file")).size(13))
                .on_press(PromptEvent::AttachFile(AttachmentKind::Text))
                .padding([4, 10])
                .width(Length::Fill)
                .style(menu_item_style),
            button(text(lang.tr("Attach image file")).size(13))
                .on_press(PromptEvent::AttachFile(AttachmentKind::Image))
                .padding([4, 10])
                .width(Length::Fill)
                .style(menu_item_style),
        ]
        .padding([4, 0]),
    )
    .style(menu_container_style)
    .boxed();

    let attachment_dropdown = dropdown(
        lang.tr("Attachment ▾"),
        attachment_menu_expanded,
        PromptEvent::ToggleAttachmentMenu,
        PromptEvent::DismissAttachmentMenu,
        overlay,
        190.0,
        Length::Shrink,
    );

    let mut prompt_column = column![
        row![
            checkbox(workmode_enabled)
                .label(lang.tr("Work mode"))
                .width(Length::Fill)
                .on_toggle(PromptEvent::ToggleWorkMode)
                .style(crate::views::primary_checkbox)
                .wrapping(Wrapping::None),
            tab_bar,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        files_field_view(files, workspace_set, lang),
        user_prompt
            .view_with_paste(
                |msg| PromptEvent::EditTextArea(FocusedTarget::UserPrompt, msg),
                PromptEvent::PasteClipboard,
            )
            .height(120),
    ];
    if !attachments.is_empty() {
        prompt_column = prompt_column.push(attachment_row(attachments));
    }
    prompt_column
        .push(
            row![
                recipe_dropdown,
                attachment_dropdown,
                iced::widget::Space::new().width(Length::Fill),
                button(text(lang.tr("Send")).size(13).align_x(Alignment::Center))
                    .width(80)
                    .on_press_maybe(can_send.then_some(PromptEvent::SendPrompt))
                    .style(crate::views::primary_button),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed(),
        )
        .spacing(4)
        .padding(padding::bottom(4))
        .boxed()
}

/// A labeled button that opens a fixed-width popup menu of items.
fn dropdown<'a>(
    label: &'a str,
    expanded: bool,
    on_toggle: PromptEvent,
    on_dismiss: PromptEvent,
    overlay: Element<'a, PromptEvent>,
    width: f32,
    height: Length,
) -> Element<'a, PromptEvent> {
    let underlay = button(text(label).size(14))
        .on_press(on_toggle)
        .padding([4, 8])
        .style(crate::views::secondary_button);
    PopupMenu::new(underlay, overlay, expanded)
        .width(Length::Fixed(width))
        .height(height)
        .gap(2.0)
        .on_dismiss(on_dismiss)
        .boxed()
}

/// One horizontally scrollable row of selected-file chips below the textarea.
fn attachment_row<'a>(attachments: &'a [UserAttachment]) -> Element<'a, PromptEvent> {
    use iced::widget::scrollable::{Direction, Scrollbar};

    let mut chips = row![].spacing(6).align_y(Alignment::Center);
    for (index, attachment) in attachments.iter().enumerate() {
        let chip: Element<'a, PromptEvent> = container(
            row![
                text(attachment.kind.icon()).size(12),
                text(attachment.display_name()).size(11),
                button(text("✕").size(10))
                    .on_press(PromptEvent::RemoveAttachment(index))
                    .padding([0, 2])
                    .style(icon_button_style),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([2, 6])
        .style(chip_style)
        .boxed();
        chips = chips.push(chip);
    }

    scrollable(chips)
        .direction(Direction::Horizontal(
            Scrollbar::new().width(4).scroller_width(4),
        ))
        .width(Fill)
        .height(Length::Fixed(30.0))
        .boxed()
}

// ── Workspace tree view ──────────────────────────────────────────

fn files_field_view<'a>(
    files: &'a FileTreePane,
    workspace_set: bool,
    lang: Lang,
) -> Element<'a, PromptEvent> {
    let name = WORKSPACE_TREE;
    let header = expandable_header(name, files.enabled, files.expanded, lang);

    use iced::widget::scrollable::{Direction, Scrollbar};

    if files.expanded {
        let body: Element<'_, PromptEvent> = if files.tree.is_empty() {
            text(if workspace_set {
                lang.tr("Loading…")
            } else {
                lang.tr("No workspace selected.")
            })
            .size(12)
            .style(|_| text::Style {
                color: Some(color_muted()),
                selection: None,
            })
            .boxed()
        } else {
            scrollable(
                container(
                    rich_text([span::<()>(&files.tree)])
                        .font(iced::Font::MONOSPACE)
                        .wrapping(Wrapping::None)
                        .selectable(true)
                        .style(sel_default),
                )
                .padding(Padding::new(0.0).bottom(12.0)),
            )
            .direction(Direction::Both {
                vertical: Scrollbar::new().width(4).scroller_width(4),
                horizontal: Scrollbar::new().width(4).scroller_width(4),
            })
            .width(Fill)
            .height(Length::Fixed(200.0))
            .boxed()
        };
        column![header, body]
            .spacing(4)
            .padding(Padding::new(0.0).top(4.0))
            .width(Fill)
            .boxed()
    } else {
        header
    }
}
