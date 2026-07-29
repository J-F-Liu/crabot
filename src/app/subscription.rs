use iced::event;
use iced::keyboard;
use iced::mouse;
use iced::window;
use iced::{Event, Subscription};

use crate::widgets::textarea;

use super::{App, ConversationEvent, LayoutEvent, Message};

/// Global event subscription mapping raw OS/input events to domain [`Message`]s.
pub(crate) fn subscription(_state: &App) -> Subscription<Message> {
    Subscription::batch([
        event::listen_with(|event, status, _window| match event {
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                Some(Message::Layout(LayoutEvent::CursorMoved(position)))
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                Some(Message::Layout(LayoutEvent::LeftPressed))
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                Some(Message::Layout(LayoutEvent::LeftReleased))
            }
            Event::Window(window::Event::Resized(size)) => {
                Some(Message::Layout(LayoutEvent::WindowResized(size)))
            }
            Event::Window(window::Event::Moved(pos)) => {
                Some(Message::Layout(LayoutEvent::WindowMoved(pos)))
            }
            // Always handle Escape regardless of capture status so that
            // selectable-text mode can be exited in a single keypress.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => Some(Message::Layout(LayoutEvent::EscapePressed)),
            // Skip keyboard shortcuts when a widget already captured the
            // event (e.g. dropdown overlay handling arrow-key navigation).
            Event::Keyboard(_) if status == event::Status::Captured => None,
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
                ..
            }) => Some(Message::Conversation(ConversationEvent::NavigateSession(
                true,
            ))),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
                ..
            }) => Some(Message::Conversation(ConversationEvent::NavigateSession(
                false,
            ))),
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if modifiers.command() =>
            {
                match key.as_ref() {
                    keyboard::Key::Character("z") => Some(Message::Layout(LayoutEvent::UndoRedo(
                        textarea::Message::Undo,
                    ))),
                    keyboard::Key::Character("y") => Some(Message::Layout(LayoutEvent::UndoRedo(
                        textarea::Message::Redo,
                    ))),
                    keyboard::Key::Character("f") => Some(Message::Conversation(
                        ConversationEvent::SearchEvent(crate::views::SearchEvent::ToggleSearch),
                    )),
                    keyboard::Key::Character("e") => Some(Message::Conversation(
                        ConversationEvent::ToggleAllDialogsExpand,
                    )),
                    keyboard::Key::Character("=") => Some(Message::Layout(LayoutEvent::Zoom(0.05))),
                    keyboard::Key::Character("-") => {
                        Some(Message::Layout(LayoutEvent::Zoom(-0.05)))
                    }
                    keyboard::Key::Character(
                        digit @ ("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"),
                    ) => {
                        // Ctrl+1–9 → Nth tab; Ctrl+0 → last tab (common convention).
                        let digit = (digit.as_bytes()[0] - b'0') as usize;
                        Some(Message::Conversation(ConversationEvent::SwitchTabByDigit(
                            digit,
                        )))
                    }
                    _ => None,
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Shift),
                ..
            }) => Some(Message::Layout(LayoutEvent::ShiftHeld(true))),
            Event::Keyboard(keyboard::Event::KeyReleased {
                key: keyboard::Key::Named(keyboard::key::Named::Shift),
                ..
            }) => Some(Message::Layout(LayoutEvent::ShiftHeld(false))),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Control),
                ..
            }) => Some(Message::Layout(LayoutEvent::CtrlHeld(true))),
            Event::Keyboard(keyboard::Event::KeyReleased {
                key: keyboard::Key::Named(keyboard::key::Named::Control),
                ..
            }) => Some(Message::Layout(LayoutEvent::CtrlHeld(false))),
            // Message-view scroll shortcuts (Home/End/PageUp/PageDown/Space).
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Home),
                ..
            }) => Some(Message::Layout(LayoutEvent::ScrollToHome)),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::End),
                ..
            }) => Some(Message::Layout(LayoutEvent::ScrollToEnd)),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::PageUp),
                ..
            }) => Some(Message::Layout(LayoutEvent::ScrollPageUp)),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::PageDown),
                ..
            }) => Some(Message::Layout(LayoutEvent::ScrollPageDown)),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Space),
                ..
            }) => Some(Message::Layout(LayoutEvent::ScrollPageDown)),
            _ => None,
        }),
        window::close_requests().map(|_id| Message::Conversation(ConversationEvent::AppClosing)),
    ])
}
