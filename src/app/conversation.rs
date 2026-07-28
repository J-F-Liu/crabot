use iced::{Task, widget::text_editor};

use crabot::HashSetExt;
use crabot::chat::Turn;
use crabot::model::ModelConfig;
use crabot::session::Session;
use crabot::user::UserPrompt;
use futures::{SinkExt, future::FutureExt};
use std::sync::atomic::Ordering;

use crate::app::session_state::{self, AskAction, SessionEvent};
use crate::app::session_tab::SessionTab;
use crate::app::{App, ConversationEvent, ConversationState, FocusedTarget, Message};
use crate::llm::DialogPhase;
use crate::views::{self, SCROLL_STEP, scroll_to_end};

pub(crate) fn update(app: &mut App, event: ConversationEvent) -> Task<Message> {
    match event {
        ConversationEvent::NavigateSession(up) => {
            return navigate_session(app, up);
        }
        ConversationEvent::ResendSessionHistory => return resend_session(app),
        ConversationEvent::SessionEvent(number, event) => {
            return session_event(app, number, event);
        }
        ConversationEvent::SearchEvent(event) => {
            return search_event(app, event);
        }
        ConversationEvent::CopySessionTitle => {
            return iced::clipboard::write(app.conversation.viewing().center_pane_title.clone());
        }
        ConversationEvent::AppClosing => {
            app.conversation.stop();
            app.save_settings();
            return iced::exit();
        }
        ConversationEvent::NewSession => return new_session(app),
        ConversationEvent::LoadSession(entry) => return load_session(app, entry),
        ConversationEvent::SwitchTab(number) => return switch_tab(app, number),
        ConversationEvent::CloseTab(number) => return close_tab(app, number),
        ConversationEvent::AskAction(action) => ask_action(&mut app.conversation, action),
        ConversationEvent::SessionListLoaded(entries) => {
            app.conversation.session_list = entries;
        }
        ConversationEvent::ToggleTurnExpand(index, sub_index) => {
            let key = (index, sub_index);
            let tab = app.conversation.viewing_mut();
            let expanded = tab.expanded_turns.contains(&key);
            tab.expanded_turns.set(key, !expanded);
            tab.search.invalidate_offsets();
            app.layout.focused = None;
        }
        ConversationEvent::ToggleDialogExpand(index) => {
            let tab = app.conversation.viewing_mut();
            let expanded = tab.expanded_dialogs.contains(&index);
            tab.expanded_dialogs.set(index, !expanded);
            tab.search.invalidate_offsets();
            app.layout.focused = None;
        }
        ConversationEvent::ToggleAllDialogsExpand => {
            let tab = app.conversation.viewing_mut();
            if tab.expanded_dialogs.is_empty() {
                tab.expanded_dialogs.extend(0..tab.session.dialogs.len());
            } else {
                tab.expanded_dialogs.clear();
            }
            tab.search.invalidate_offsets();
            app.layout.focused = None;
        }
        ConversationEvent::SessionPickerFocused => {
            app.layout.focused = Some(FocusedTarget::SessionPicker);
        }
        ConversationEvent::DefocusSessionPicker => {
            app.layout.focused = None;
        }
        ConversationEvent::AskInputChanged(input) => {
            app.conversation.viewing_mut().session_state.ask_input = input;
        }
        ConversationEvent::ToggleSelectableMode(index) => {
            let tab = app.conversation.viewing_mut();
            match index {
                Some(index) => {
                    let selected = tab.selectable_msgs.contains(&index);
                    tab.selectable_msgs.set(index, !selected);
                }
                None => tab.selectable_msgs.clear(),
            }
        }
        ConversationEvent::TurnOffsetsMeasured(tab_number, generation, offsets, target_y) => {
            let Some(pos) = app.conversation.tab_pos(tab_number) else {
                // Tab was closed while measurement was in flight — discard.
                return Task::none();
            };
            let tab = &mut app.conversation.session_tabs[pos];
            tab.search.handle_offsets(generation, offsets);
            // Only scroll if the originating tab is still being viewed;
            // otherwise store the offsets silently so the next switch-to-tab
            // can use cached values without disrupting the current view.
            if pos == app.conversation.viewing {
                return views::scroll_to(target_y).discard();
            }
        }
    }
    Task::none()
}

fn new_session(app: &mut App) -> Task<Message> {
    let number = app.conversation.next_tab_number();
    let tab = SessionTab::new(number);
    app.conversation.session_tabs.push(tab);
    app.conversation.viewing = app.conversation.session_tabs.len() - 1;

    app.layout.focused = None;

    // Refresh workspace-dependent fields
    let workspace = app.prompt.workspace.1.clone();
    let tree = crabot::workspace::build_files_tree(&workspace);
    app.prompt.files.content = text_editor::Content::with_text(&tree);
    app.prompt.files.enabled = true;
    let (exists, content) = crate::app::prompt::load_agents_md(&workspace);
    app.prompt.agents_md_exists = exists;
    app.prompt.agents_md.1 = content;

    // Fresh tab has no saved scroll offset — scroll to top.
    views::scroll_to_start().discard()
}

/// Switch the viewing tab to the one with the given number.
fn switch_tab(app: &mut App, number: usize) -> Task<Message> {
    let Some(pos) = app.conversation.tab_pos(number) else {
        return Task::none();
    };
    if pos == app.conversation.viewing {
        return Task::none();
    }

    // Save the outgoing tab's scroll position.
    // It was already captured by on_scroll, so scroll_offset is current.

    app.conversation.viewing = pos;
    app.layout.focused = None;

    // Restore or reset the incoming tab's scroll position.
    let tab = app.conversation.viewing_mut();
    views::scroll_to(tab.scroll_offset).discard()
}

/// Close the tab with the given number.
///
/// Running tabs cannot be closed (the close button is disabled).
/// The last remaining tab is replaced with a fresh one.
fn close_tab(app: &mut App, number: usize) -> Task<Message> {
    let Some(pos) = app.conversation.tab_pos(number) else {
        return Task::none();
    };
    if app.conversation.session_tabs[pos].running() {
        return Task::none();
    }
    let was_viewing = pos == app.conversation.viewing;
    app.conversation.session_tabs.remove(pos);

    if app.conversation.session_tabs.is_empty() {
        let number = app.conversation.next_tab_number();
        app.conversation.session_tabs.push(SessionTab::new(number));
        app.conversation.viewing = 0;
    } else if pos < app.conversation.viewing {
        app.conversation.viewing -= 1;
    } else if app.conversation.viewing >= app.conversation.session_tabs.len() {
        app.conversation.viewing = app.conversation.session_tabs.len().saturating_sub(1);
    }

    app.layout.focused = None;

    // Restore or reset the newly-viewed tab's scroll position if the closed tab was viewing.
    if was_viewing {
        return views::scroll_to(app.conversation.viewing().scroll_offset).discard();
    }
    Task::none()
}

fn load_session(app: &mut App, entry: views::session_list::SessionEntry) -> Task<Message> {
    // If the session is already open in a tab, just switch to it.
    if let Some(existing) = app
        .conversation
        .session_tabs
        .iter()
        .position(|t| t.session.id == entry.id)
    {
        if existing != app.conversation.viewing {
            return switch_tab(app, app.conversation.session_tabs[existing].number);
        }
        return Task::none();
    }

    // Block loading into the viewing tab while it is streaming.
    if app.conversation.viewing_is_streaming() {
        return Task::none();
    }

    match Session::load(&entry.path) {
        Ok(session) => {
            let number = app.conversation.viewing_tab_number();
            let tab = app.conversation.viewing_mut();
            *tab = SessionTab::from_session(number, session);
            // Scroll to top for a freshly loaded session.
            return views::scroll_to_start().discard();
        }
        Err(error) => {
            eprintln!("Failed to load session: {error}");
        }
    }
    Task::none()
}

fn navigate_session(app: &mut App, up: bool) -> Task<Message> {
    let viewing_is_streaming = app.conversation.viewing_is_streaming();
    let list_empty = app.conversation.session_list.is_empty();
    let is_picker_focused = app.layout.focused == Some(FocusedTarget::SessionPicker);

    if !is_picker_focused || viewing_is_streaming || list_empty {
        return views::scroll_by(if up { -SCROLL_STEP } else { SCROLL_STEP }).discard();
    }

    let current = app
        .conversation
        .session_list
        .iter()
        .position(|entry| entry.id == app.conversation.viewing().session.id);

    // Find the next non-header entry, wrapping around the list.
    let next_idx = |idx: usize, up: bool| -> Option<usize> {
        let len = app.conversation.session_list.len();
        let step = |i: usize| {
            if up {
                i.checked_sub(1).unwrap_or(len - 1)
            } else {
                (i + 1) % len
            }
        };
        let start = step(idx);
        let mut i = start;
        loop {
            if !app.conversation.session_list[i].is_header {
                return Some(i);
            }
            i = step(i);
            if i == start {
                return None; // all entries are headers
            }
        }
    };

    let entry = current
        .and_then(|idx| next_idx(idx, up))
        .map(|i| app.conversation.session_list[i].clone());
    entry.map_or_else(Task::none, |entry| {
        Task::done(Message::Conversation(ConversationEvent::LoadSession(entry)))
    })
}

// ── ask tool ───────────────────────────────────────────────────────

fn ask_action(conversation: &mut ConversationState, action: AskAction) {
    let result = match action {
        AskAction::OptionSelected(option) => {
            conversation.viewing_mut().session_state.ask_input = option;
            return;
        }
        AskAction::Ok => Ok(conversation.viewing().session_state.ask_input.clone()),
        AskAction::Skip => Ok("No preference. Use your best judgment.".into()),
    };
    let _ = conversation
        .viewing_mut()
        .session_state
        .ask_sender
        .send(result);
    conversation.viewing_mut().session_state.ask_request = None;
}

// ── streaming events ───────────────────────────────────────────────

/// Route a tagged stream event to the owning tab.
fn session_event(app: &mut App, number: usize, event: SessionEvent) -> Task<Message> {
    let Some(pos) = app.conversation.tab_pos(number) else {
        // Tab was closed while the stream was still running — drop the event.
        return Task::none();
    };

    // Auto-switch to a background tab that issues an ask, then keep processing
    // the event below so the ask request is actually registered on the tab.
    let switch_task =
        if pos != app.conversation.viewing && matches!(event, SessionEvent::AskRequest(_)) {
            switch_tab(app, number)
        } else {
            Task::none()
        };
    // `switch_tab` only changes `viewing`, never reorders tabs, so `pos` stays valid.
    let viewing = pos == app.conversation.viewing;

    // Compute cost from the tab's session model BEFORE mutably borrowing the tab.
    let model_config = app.conversation.session_tabs[pos]
        .session
        .model
        .clone()
        .or_else(|| app.models.get_config(&app.settings.selected_model).cloned());
    let cost = model_config
        .as_ref()
        .and_then(|cfg| app.models.get_model(cfg))
        .map(|m| m.cost.clone());

    // Update todo snapshot into the target tab on todo ToolResult.
    if let SessionEvent::ToolResult(ref result) = event
        && result.name == "todo"
    {
        app.conversation.session_tabs[pos].todo_items = app.tools.tool_registry.snapshot_todo();
    }

    let is_terminal = matches!(
        event,
        SessionEvent::Done(_) | SessionEvent::Error(_, _) | SessionEvent::Cancelled(_)
    );

    let tab = &mut app.conversation.session_tabs[pos];
    let task = session_state::update(event, tab, cost, viewing);

    // After a stream finishes, auto-dispatch a prompt that was parked on another idle tab.
    let dispatch_task = if is_terminal && app.conversation.running_pos().is_none() {
        app.conversation
            .session_tabs
            .iter()
            .position(|t| t.number != number && t.session_state.pending_prompt.is_some())
            .map(|target| dispatch_pending(app, target))
            .unwrap_or_else(Task::none)
    } else {
        Task::none()
    };

    switch_task.chain(task.discard()).chain(dispatch_task)
}

/// Handle search-bar events on the viewing tab.
fn search_event(app: &mut App, event: crate::views::SearchEvent) -> Task<Message> {
    let tab_number = app.conversation.viewing_tab_number();
    let tab = app.conversation.viewing_mut();
    views::search_bar::update_on(event, tab, tab_number).map(Message::Conversation)
}

// ── send / resend ──────────────────────────────────────────────────

pub(crate) fn send_prompt(app: &mut App) -> Task<Message> {
    let raw = app.prompt.user_prompt.text();
    let content = crabot::tools::normalize_newlines(&raw).into_owned();
    if content.trim().is_empty() {
        return Task::none();
    }
    let Some(model) = app.selected_model_config() else {
        return Task::none();
    };
    if app.prompt.workspace.1.as_os_str().is_empty() {
        app.overlay.show_workspace_dialog = true;
        return Task::none();
    }

    let user_prompt = build_user_prompt(app, &content);
    app.prompt.files.enabled = false;
    app.prompt.user_prompt.clear();

    let another_running =
        app.conversation.running_pos().is_some() && !app.conversation.viewing_is_streaming();
    let tab_pos = app.conversation.viewing;
    let tab = app.conversation.viewing_mut();

    // If current tab is running, inject to streaming.
    if tab.session_state.phase != DialogPhase::Idle {
        tab.session_state.inject_prompt(user_prompt);
        return Task::none();
    }

    // If another tab is running, park as pending.
    if another_running {
        tab.session_state.set_pending(user_prompt);
        return Task::none();
    }

    launch_dialog(app, tab_pos, &content, &model, user_prompt)
}

/// Build a `UserPrompt` from `content` using the current prompt settings.
fn build_user_prompt(app: &App, content: &str) -> UserPrompt {
    let mode = app.prompt.workmode_enabled.then_some(app.prompt.workmode);
    let workspace_tree = if app.prompt.files.enabled {
        let tree = app.prompt.files.content.text();
        (!tree.is_empty()).then(|| tree.to_string())
    } else {
        None
    };
    UserPrompt::new(mode, content.to_owned(), workspace_tree)
}

/// Set up a new dialog from `content` on the given tab and start streaming.
fn launch_dialog(
    app: &mut App,
    tab_pos: usize,
    content: &str,
    model: &ModelConfig,
    user_prompt: UserPrompt,
) -> Task<Message> {
    let tab = &mut app.conversation.session_tabs[tab_pos];
    tab.center_pane_title = content.to_owned();
    let dialog_index = tab.session.dialogs.len();
    tab.expanded_dialogs.clear();
    tab.expanded_dialogs.insert(dialog_index);
    tab.session.add_dialog(Session::derive_title(content));
    tab.session
        .push_turn(Turn::user(user_prompt.content.clone()));
    // `tab` borrow ends here (NLL); start_dialog takes a fresh &mut App.
    start_dialog(app, tab_pos, model, Some(user_prompt))
}

/// Auto-dispatch a prompt parked on an idle tab while another tab streamed.
fn dispatch_pending(app: &mut App, tab_pos: usize) -> Task<Message> {
    // Validate guards BEFORE taking the parked prompt so it isn't lost on failure.
    let Some(model) = app.selected_model_config() else {
        return Task::none();
    };
    if app.prompt.workspace.1.as_os_str().is_empty() {
        app.overlay.show_workspace_dialog = true;
        return Task::none();
    }
    let Some(user_prompt) = app.conversation.session_tabs[tab_pos]
        .session_state
        .pending_prompt
        .take()
    else {
        return Task::none();
    };
    // `start_dialog` clears the stale `pending_user_prompt` shared-lock copy
    // so the new stream won't re-inject it as an interrupt.
    app.prompt.files.enabled = false;
    let content = user_prompt.content.clone();
    launch_dialog(app, tab_pos, &content, &model, user_prompt)
}

fn resend_session(app: &mut App) -> Task<Message> {
    let viewing_is_streaming = app.conversation.viewing_is_streaming();
    let is_new = app.conversation.viewing().center_pane_title == "New session";

    if viewing_is_streaming || is_new || app.conversation.running_pos().is_some() {
        return Task::none();
    }
    let Some(model) = app.selected_model_config() else {
        return Task::none();
    };
    let tab_pos = app.conversation.viewing;
    let tab = app.conversation.viewing_mut();
    tab.expanded_dialogs.clear();
    if let Some(index) = tab.session.dialogs.len().checked_sub(1) {
        tab.expanded_dialogs.insert(index);
    }
    // `tab` borrow ends here (NLL); start_dialog takes a fresh &mut App.
    start_dialog(app, tab_pos, &model, None)
}

// ── Stream orchestration ──────────────────────────────────────────

/// Prepare and launch an LLM dialog stream for the given tab.
pub(crate) fn start_dialog(
    app: &mut App,
    tab_pos: usize,
    model_config: &ModelConfig,
    user_prompt: Option<UserPrompt>,
) -> Task<Message> {
    let Some(model) = app.models.get_model_info(model_config) else {
        return Task::none();
    };
    let is_viewing = tab_pos == app.conversation.viewing;
    // When continuing with a different model, fork the session.
    let (tab_number, session_list_entry) = {
        let tab = &mut app.conversation.session_tabs[tab_pos];
        let tab_number = tab.number;
        let model_changed = tab
            .session
            .model
            .as_ref()
            .is_some_and(|m| m.model_id != model_config.model_id);
        let session_forked = if model_changed && tab.session.history.len() > 1 {
            tab.session = tab.session.fork();
            if model_config.model_id.starts_with("deepseek") {
                tab.session.fix_history();
            }
            true
        } else {
            false
        };
        tab.session.model = Some(model_config.clone());
        tab.session.workspace = app.prompt.workspace.1.clone();
        tab.session.save().ok();

        let entry = if (tab.session.is_fresh() || session_forked)
            && let Some(path) = tab.session.save_path()
        {
            let year_month = crabot::session::year_month_from_id(&tab.session.id);
            Some(crate::views::session_list::SessionEntry {
                id: tab.session.id.clone(),
                title: tab.session.title.clone(),
                path,
                year_month,
                is_header: false,
            })
        } else {
            None
        };
        (tab_number, entry)
    };

    // Add to session list now that the tab borrow is released.
    if let Some(entry) = session_list_entry {
        app.conversation.session_list.insert(0, entry);
    }

    // Re-borrow for the remaining setup.
    let tab = &mut app.conversation.session_tabs[tab_pos];
    tab.session_state.clear_pending();
    tab.session_state.start_index = tab.session.total_turns();
    tab.session_state.auto_scroll.store(true, Ordering::Relaxed);

    // Create a fresh mpsc channel for this stream's ask-tool responses.
    let (ask_tx, ask_rx) = tokio::sync::mpsc::unbounded_channel();
    tab.session_state.ask_sender = ask_tx;

    let config = crate::llm::SendConfig {
        model,
        workspace: app.prompt.workspace.1.clone(),
        system_prompt: app.prompt.get_prompt(),
        user_prompt,
        tools: app
            .tools
            .tool_registry
            .enabled_tools(&app.tools.enabled_tools, &app.tools.enabled_mcp_servers),
        injected_prompt: tab.session_state.injected_prompt.clone(),
        ask_receiver: ask_rx,
        user_agent: crabot::app_title().to_string(),
        cancel_token: tab.session_state.cancel_token.clone(),
    };

    let history = tab.session.history.clone();

    tab.session_state.phase = DialogPhase::LlmLoading;
    tab.session_state
        .cancel_token
        .store(false, Ordering::Relaxed);
    let cancel_token = tab.session_state.cancel_token.clone();

    Task::batch([
        if is_viewing {
            scroll_to_end().discard()
        } else {
            Task::none()
        },
        Task::stream(iced::stream::channel(128, async move |sender| {
            let cancel = cancel_token.clone();
            let mut callback = {
                move |msg: SessionEvent| {
                    let cancel = cancel.clone();
                    let mut sender = sender.clone();
                    async move {
                        let ok = sender
                            .send(Message::Conversation(ConversationEvent::SessionEvent(
                                tab_number, msg,
                            )))
                            .await
                            .is_ok();
                        if cancel.load(Ordering::Relaxed) {
                            false
                        } else {
                            ok
                        }
                    }
                    .boxed()
                }
            };
            crate::llm::send_stream(config, history, &mut callback).await;
        })),
    ])
}

/// Refresh the session list dropdown entries from disk.
pub(crate) fn refresh_session_list(workspace: std::path::PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                crate::views::session_list::list_entries(&workspace)
            })
            .await
            .unwrap_or(Ok(Vec::new()))
        },
        |result| match result {
            Ok(entries) => Message::Conversation(ConversationEvent::SessionListLoaded(entries)),
            Err(_) => Message::Conversation(ConversationEvent::SessionListLoaded(Vec::new())),
        },
    )
}
