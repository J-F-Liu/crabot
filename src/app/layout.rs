use iced::{Point, Task};

use crate::app::session_state;
use crate::app::{App, FocusedTarget, LayoutEvent, Message, ModelSettingsEvent, TabBarScrollState};
use crate::views;
use crate::views::theme::{HANDLE, MIN_W};
use crate::widgets::textarea;

/// Update application layout and global input state.
pub(crate) fn update(app: &mut App, event: LayoutEvent) -> Task<Message> {
    match event {
        LayoutEvent::CursorMoved(pos) => cursor_moved(app, pos),
        LayoutEvent::LeftPressed => return left_pressed(app),
        LayoutEvent::LeftReleased => left_released(app),
        LayoutEvent::WindowResized(size) => {
            if size.width > 0.0 && size.height > 0.0 {
                app.layout.window_size = size;
                for tab in &mut app.conversation.session_tabs {
                    tab.search.invalidate_offsets();
                }
            }
        }
        LayoutEvent::WindowMoved(pos) => app.layout.window_pos = pos,
        LayoutEvent::WindowFocusChanged(focused) => app.layout.window_focused = focused,
        LayoutEvent::ShiftHeld(held) => app.layout.shift_held = held,
        LayoutEvent::CtrlHeld(held) => app.layout.ctrl_held = held,
        LayoutEvent::SessionViewScrolled(viewport) => {
            app.layout.scroll_viewport_height = viewport.bounds().height;
            let tab = app.conversation.viewing_mut();
            tab.scroll_offset = Some(viewport.absolute_offset().y);
            session_state::handle_scroll(&tab.session_state, viewport);
        }
        LayoutEvent::TabBarScrolled(viewport) => {
            app.conversation.tab_bar_scroll = TabBarScrollState::from_viewport(&viewport);
        }
        LayoutEvent::ScrollPageDown => {
            return views::scroll_page_down(app.layout.scroll_viewport_height).discard();
        }
        LayoutEvent::ScrollPageUp => {
            return views::scroll_page_up(app.layout.scroll_viewport_height).discard();
        }
        LayoutEvent::ScrollToHome => {
            return views::scroll_to_start().discard();
        }
        LayoutEvent::ScrollToEnd => {
            return views::scroll_to_end().discard();
        }
        LayoutEvent::UndoRedo(message) => undo_redo(app, message),
        LayoutEvent::EscapePressed => escape(app),
        LayoutEvent::Zoom(delta) => {
            app.settings.font_scale = (app.settings.font_scale + delta).clamp(0.5, 2.0);
            for tab in &mut app.conversation.session_tabs {
                tab.search.invalidate_offsets();
            }
        }
        LayoutEvent::ToggleTheme(dark) => {
            app.settings.dark_mode = dark;
            views::theme::set_dark_mode(dark);
            app.layout.theme = views::theme::theme_for(dark);
            app.save_settings();
        }
    }
    Task::none()
}

fn cursor_moved(app: &mut App, pos: Point) {
    app.layout.cursor = pos;
    app.layout.left_divider.hovered =
        pos.x >= app.settings.left_pane_width && pos.x <= app.settings.left_pane_width + HANDLE;
    let right_x = app.layout.window_size.width - app.settings.right_pane_width - HANDLE;
    app.layout.right_divider.hovered = pos.x >= right_x && pos.x <= right_x + HANDLE;

    if app.layout.left_divider.dragging {
        let delta = pos.x - app.layout.left_divider.origin;
        let gutter = 2.0 * HANDLE;
        let max = (app.layout.window_size.width - app.settings.right_pane_width - gutter - MIN_W)
            .max(MIN_W);
        app.settings.left_pane_width = (app.layout.left_divider.start + delta).clamp(MIN_W, max);
    } else if app.layout.right_divider.dragging {
        let delta = pos.x - app.layout.right_divider.origin;
        let gutter = 2.0 * HANDLE;
        let max = (app.layout.window_size.width - app.settings.left_pane_width - gutter - MIN_W)
            .max(MIN_W);
        let new_width = (app.layout.right_divider.start - delta).max(0.0);
        app.settings.right_pane_width = if app.layout.right_divider.start == 0.0 {
            if new_width > 10.0 {
                new_width.clamp(MIN_W, max)
            } else {
                0.0
            }
        } else if new_width < MIN_W - 10.0 {
            0.0
        } else {
            new_width.min(max)
        };
    }
}

fn left_pressed(app: &mut App) -> Task<Message> {
    let left_x = app.settings.left_pane_width;
    let right_x = app.layout.window_size.width - app.settings.right_pane_width - HANDLE;

    if app.layout.cursor.x >= left_x && app.layout.cursor.x <= left_x + HANDLE {
        app.layout.left_divider.dragging = true;
        app.layout.left_divider.origin = app.layout.cursor.x;
        app.layout.left_divider.start = app.settings.left_pane_width;
    } else if app.layout.cursor.x >= right_x && app.layout.cursor.x <= right_x + HANDLE {
        if app.settings.right_pane_width == 0.0 {
            app.settings.right_pane_width = MIN_W;
        } else {
            app.layout.right_divider.dragging = true;
            app.layout.right_divider.origin = app.layout.cursor.x;
            app.layout.right_divider.start = app.settings.right_pane_width;
        }
    }

    if app.settings_dialog.open && app.settings_dialog.is_adding_label() {
        return iced::widget::operation::is_focused(views::NEW_LABEL_INPUT_ID).map(|focused| {
            Message::ModelSettings(ModelSettingsEvent::Settings(
                views::SettingsEvent::LabelInputFocus(focused),
            ))
        });
    }

    Task::none()
}

fn left_released(app: &mut App) {
    app.layout.left_divider.dragging = false;
    app.layout.right_divider.dragging = false;
    app.conversation.tab_bar_held_direction = None;
    if app.settings_dialog.is_label_dragging() {
        app.settings_dialog
            .update(views::SettingsEvent::LabelDragEnd);
    }
}

fn undo_redo(app: &mut App, message: textarea::Message) {
    let layout = &app.layout;
    if let Some(FocusedTarget::UserPrompt) = layout.focused {
        app.prompt.user_prompt.update(message, layout.shift_held);
    }
}

fn escape(app: &mut App) {
    if app.settings_dialog.open {
        if app.settings_dialog.is_adding_label() {
            app.confirm_pending_label();
        } else {
            app.settings.auto_check_updates = app.settings_dialog.auto_check_updates;
            app.settings_dialog.open = false;
        }
    } else if app.conversation.viewing().search.visible {
        app.conversation.viewing_mut().search.visible = false;
    } else {
        app.conversation.viewing_mut().selectable_msgs.clear();
    }
}
