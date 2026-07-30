use iced::{
    Task,
    widget::{self, text_editor},
};

use crabot::HashSetExt;
use crabot::chat::Turn;
use crabot::model::ModelConfig;
use crabot::session::Session;
use crabot::user::UserPrompt;
use futures::{SinkExt, future::FutureExt};
use genai::chat::ChatRole;
use std::sync::atomic::Ordering;

use crate::app::session_state::{self, AskAction, SessionEvent};
use crate::app::session_tab::SessionTab;
use crate::app::{App, ConversationEvent, FocusedTarget, Message, TabBarScrollState};
use crate::llm::DialogPhase;
use crate::views::session_tabs::TAB_SCROLL_STEP;
use crate::views::{self, ASK_INPUT, SCROLL_STEP, scroll_to_end};

/// Direction of tab-bar for scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabBarDirection {
    Left,
    Right,
}

/// Scroll the tab bar by `delta` pixels, clamping to valid range.
fn scroll_tab_bar(s: &mut TabBarScrollState, delta: f32) -> Task<Message> {
    let new_x = (s.offset + delta).clamp(0.0, s.max_offset());
    s.offset = new_x;
    crate::views::session_tabs::scroll_tab_bar_to(new_x).discard()
}

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
        ConversationEvent::NewSession => {
            return new_session(app, app.settings.selected_model.clone());
        }
        ConversationEvent::LoadSession(entry) => return load_session(app, entry),
        ConversationEvent::SwitchTab(number) => return switch_tab(app, number),
        ConversationEvent::SwitchTabByDigit(digit) => return switch_tab_by_digit(app, digit),
        ConversationEvent::CloseTab(number) => return close_tab(app, number),
        ConversationEvent::AskAction(action) => return ask_action(app, action),
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
        ConversationEvent::TabBarScrollLeftHold => {
            app.conversation.tab_bar_held_direction = Some(TabBarDirection::Left);
            return scroll_tab_bar(&mut app.conversation.tab_bar_scroll, -TAB_SCROLL_STEP);
        }
        ConversationEvent::TabBarScrollRightHold => {
            app.conversation.tab_bar_held_direction = Some(TabBarDirection::Right);
            return scroll_tab_bar(&mut app.conversation.tab_bar_scroll, TAB_SCROLL_STEP);
        }
        ConversationEvent::TabBarScrollTick => {
            let dir = match app.conversation.tab_bar_held_direction {
                Some(d) => d,
                None => return Task::none(),
            };
            let can_scroll = match dir {
                TabBarDirection::Left => app.conversation.tab_bar_scroll.can_scroll_left(),
                TabBarDirection::Right => app.conversation.tab_bar_scroll.can_scroll_right(),
            };
            if !can_scroll {
                return Task::none();
            }
            let delta = match dir {
                TabBarDirection::Left => -TAB_SCROLL_STEP,
                TabBarDirection::Right => TAB_SCROLL_STEP,
            };
            return scroll_tab_bar(&mut app.conversation.tab_bar_scroll, delta);
        }
        ConversationEvent::TabBarArrowEnter(dir) => {
            app.conversation.tab_bar_hovered_direction = Some(dir);
        }
        ConversationEvent::TabBarArrowExit => {
            app.conversation.tab_bar_hovered_direction = None;
        }
    }
    Task::none()
}

fn new_session(app: &mut App, selected_model: String) -> Task<Message> {
    let number = app.conversation.next_tab_number();
    let tab = SessionTab::new(number, selected_model);
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

    // Switch tab
    app.conversation.viewing = pos;
    app.layout.focused = None;
    // Remove from pending-ask queue — the user is now viewing this tab.
    app.conversation.pending_ask_queue.retain(|&n| n != number);

    // Restore the incoming tab's selected model.
    let tab = app.conversation.viewing_mut();
    app.settings.selected_model = tab.selected_model.clone();

    // Restore or reset the incoming tab's scroll position.
    let scroll_task = views::scroll_to(tab.scroll_offset).discard();

    // Focus the ask input so the user can answer the ask tool immediately.
    let focus_task = widget::operation::focus(ASK_INPUT.clone());
    Task::batch([scroll_task, focus_task])
}

/// Switch to the tab at the given 1-based position; digit 0 means the last tab.
fn switch_tab_by_digit(app: &mut App, digit: usize) -> Task<Message> {
    let tabs = &app.conversation.session_tabs;
    let number = if digit == 0 {
        tabs.last().map(|t| t.number)
    } else {
        tabs.get(digit - 1).map(|t| t.number)
    };
    match number {
        Some(n) => switch_tab(app, n),
        None => Task::none(),
    }
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
    // Clean up the pending-ask queue — the tab is gone.
    app.conversation.pending_ask_queue.retain(|&n| n != number);

    if app.conversation.session_tabs.is_empty() {
        let number = app.conversation.next_tab_number();
        let selected_model = app.settings.selected_model.clone();
        app.conversation
            .session_tabs
            .push(SessionTab::new(number, selected_model));
        app.conversation.viewing = 0;
    } else if pos < app.conversation.viewing {
        app.conversation.viewing -= 1;
    } else if app.conversation.viewing >= app.conversation.session_tabs.len() {
        app.conversation.viewing = app.conversation.session_tabs.len().saturating_sub(1);
    }

    app.layout.focused = None;

    // Restore or reset the newly-viewed tab's scroll position if the closed tab was viewing.
    if was_viewing {
        let tab = app.conversation.viewing_mut();
        app.settings.selected_model = tab.selected_model.clone();
        return views::scroll_to(tab.scroll_offset).discard();
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
            // If the loaded session has a stored model, find the matching model name
            let selected_model = if let Some(ref model_config) = session.model {
                app.find_model_label(model_config)
            } else {
                app.settings.selected_model.clone()
            };
            // Restore the selected model in settings
            app.settings.selected_model = selected_model.clone();
            let tab = app.conversation.viewing_mut();
            *tab = SessionTab::from_session(number, session, selected_model);
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

fn ask_action(app: &mut App, action: AskAction) -> Task<Message> {
    let result = match action {
        AskAction::OptionSelected(option) => {
            app.conversation.viewing_mut().session_state.ask_input = option;
            return Task::none();
        }
        AskAction::Ok => Ok(app.conversation.viewing().session_state.ask_input.clone()),
        AskAction::Skip => Ok("No preference. Use your best judgment.".into()),
    };
    let _ = app
        .conversation
        .viewing_mut()
        .session_state
        .ask_sender
        .send(result);
    app.conversation.viewing_mut().session_state.ask_request = None;

    // After answered, switch to the next pending tab that issued an ask.
    process_pending_ask_queue(app)
}

// ── streaming events ───────────────────────────────────────────────

/// Handle a renew-tool request: create a new session tab and launch the
/// continuation prompt on it, using the same model and work mode as the
/// originating session.
fn handle_renew(app: &mut App, number: usize, prompt: String) -> Task<Message> {
    // Look up the model config and work mode from the originating tab's session.
    let (model, work_mode, workspace_tree, selected_model) = {
        let Some(pos) = app.conversation.tab_pos(number) else {
            return Task::none();
        };
        let tab = &app.conversation.session_tabs[pos];
        let selected_model = tab.selected_model.clone();
        let model = match tab
            .session
            .model
            .clone()
            .or_else(|| app.models.get_config(&tab.selected_model).cloned())
        {
            Some(m) => m,
            None => return Task::none(),
        };
        let work_mode = tab.session.dialogs.last().and_then(|d| d.mode);
        // If the original session was started with a workspace tree, rebuild it
        // so the new session has an up-to-date view of the workspace.
        let workspace_tree = tab
            .session
            .history
            .first()
            .filter(|m| m.role == ChatRole::User)
            .and_then(|m| {
                m.content.parts().iter().find_map(|p| {
                    p.as_text()
                        .filter(|t| t.starts_with("Working directory layout"))
                        .map(|_| crabot::workspace::build_files_tree(&app.prompt.workspace.1))
                })
            });
        (model, work_mode, workspace_tree, selected_model)
    };
    let new_task = new_session(app, selected_model);
    let tab_pos = app.conversation.viewing;
    let user_prompt = UserPrompt::new(work_mode, prompt, workspace_tree);
    let launch_task = launch_dialog(app, tab_pos, &model, user_prompt);
    new_task.chain(launch_task)
}

/// Pop the next pending ask from the queue and switch to that tab.
fn process_pending_ask_queue(app: &mut App) -> Task<Message> {
    while let Some(number) = app.conversation.pending_ask_queue.pop_front() {
        // Skip tabs that have already been closed or whose ask was resolved by other means (e.g. timeout).
        if let Some(pos) = app.conversation.tab_pos(number)
            && app.conversation.session_tabs[pos]
                .session_state
                .ask_request
                .is_some()
        {
            if pos == app.conversation.viewing {
                // Already viewing — request is visible, nothing to do.
                break;
            }
            return switch_tab(app, number);
        }
    }
    Task::none()
}

/// Route a tagged stream event to the owning tab.
fn session_event(app: &mut App, number: usize, event: SessionEvent) -> Task<Message> {
    // Handle RenewRequest before the normal flow — it creates a new session.
    if let SessionEvent::RenewRequest(ref prompt) = event {
        return handle_renew(app, number, prompt.clone());
    }

    let Some(pos) = app.conversation.tab_pos(number) else {
        // Tab was closed while the stream was still running — drop the event.
        return Task::none();
    };

    let switch_task =
        if pos != app.conversation.viewing && matches!(event, SessionEvent::AskRequest(_)) {
            if app
                .conversation
                .viewing()
                .session_state
                .ask_request
                .is_some()
            {
                // queue it instead so the user answers current before the next
                app.conversation.pending_ask_queue.push_back(number);
                Task::none()
            } else {
                // auto-switch to a background tab that issues an ask
                switch_tab(app, number)
            }
        } else {
            Task::none()
        };
    // `switch_tab` only changes `viewing`, never reorders tabs, so `pos` stays valid.
    let viewing = pos == app.conversation.viewing;

    // Compute cost and context window from the tab's session model BEFORE mutably borrowing the tab.
    let tab_model_label = app.conversation.session_tabs[pos].selected_model.clone();
    let model_config = app.conversation.session_tabs[pos]
        .session
        .model
        .clone()
        .or_else(|| app.models.get_config(&tab_model_label).cloned());
    let cost = model_config
        .as_ref()
        .and_then(|cfg| app.models.get_model(cfg))
        .map(|m| m.cost.clone());
    let context_window = model_config.as_ref().map(|cfg| cfg.context_window);

    let finished = matches!(event, SessionEvent::Done(_));

    // Remember whether this tab had an active ask so we can detect a clear.
    let had_ask = app.conversation.session_tabs[pos]
        .session_state
        .ask_request
        .is_some();

    let tab = &mut app.conversation.session_tabs[pos];
    let fill_ratio_threshold = app.settings.fill_ratio_threshold;
    let update_task = session_state::update(
        event,
        tab,
        cost,
        context_window,
        fill_ratio_threshold,
        viewing,
    );

    // If this tab's ask was just resolved (user answer, timeout, Done, …),
    // remove it from the queue and, when the viewing tab has no active ask,
    // process remaining pending asks.
    let ask_cleared = had_ask
        && app.conversation.session_tabs[pos]
            .session_state
            .ask_request
            .is_none();
    let mut queue_task = Task::none();
    if ask_cleared {
        app.conversation.pending_ask_queue.retain(|&n| n != number);
        if app
            .conversation
            .viewing()
            .session_state
            .ask_request
            .is_none()
        {
            queue_task = process_pending_ask_queue(app);
        }
    }

    // Auto-dispatch a prompt that was injected too late for the just-ended stream.
    let dispatch_task = if finished {
        dispatch_pending(app, pos)
    } else {
        Task::none()
    };

    switch_task
        .chain(update_task.discard())
        .chain(dispatch_task)
        .chain(queue_task)
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

    let user_prompt = build_user_prompt(app, content);
    app.prompt.files.enabled = false;
    app.prompt.user_prompt.clear();

    let tab_pos = app.conversation.viewing;
    let tab = app.conversation.viewing_mut();

    // If current tab is running, inject to streaming.
    if tab.session_state.phase != DialogPhase::Idle {
        tab.session_state.inject_prompt(user_prompt);
        return Task::none();
    }

    launch_dialog(app, tab_pos, &model, user_prompt)
}

/// Build a `UserPrompt` from `content` using the current prompt settings.
fn build_user_prompt(app: &App, content: String) -> UserPrompt {
    let mode = app.prompt.workmode_enabled.then_some(app.prompt.workmode);
    let workspace_tree = if app.prompt.files.enabled {
        let tree = app.prompt.files.content.text();
        (!tree.is_empty()).then(|| tree.to_string())
    } else {
        None
    };
    UserPrompt::new(mode, content, workspace_tree)
}

/// Set up a new dialog from `content` on the given tab and start streaming.
fn launch_dialog(
    app: &mut App,
    tab_pos: usize,
    model: &ModelConfig,
    user_prompt: UserPrompt,
) -> Task<Message> {
    let tab = &mut app.conversation.session_tabs[tab_pos];
    tab.center_pane_title = user_prompt.content.clone();
    let dialog_index = tab.session.dialogs.len();
    tab.expanded_dialogs.clear();
    tab.expanded_dialogs.insert(dialog_index);
    tab.session.add_dialog(
        Session::derive_title(&user_prompt.content),
        user_prompt.mode,
    );
    tab.session
        .push_turn(Turn::user(user_prompt.content.clone()));
    // `tab` borrow ends here (NLL); start_dialog takes a fresh &mut App.
    start_dialog(app, tab_pos, model, Some(user_prompt))
}

/// Auto-dispatch a prompt that was injected too late for the just-ended stream.
fn dispatch_pending(app: &mut App, tab_pos: usize) -> Task<Message> {
    // Use the tab's own model (session model takes precedence over the saved label).
    let tab = &app.conversation.session_tabs[tab_pos];
    let model = tab
        .session
        .model
        .clone()
        .or_else(|| app.models.get_config(&tab.selected_model).cloned());
    // Validate guards BEFORE taking the parked prompt so it isn't lost on failure.
    let Some(model) = model else {
        return Task::none();
    };
    if app.prompt.workspace.1.as_os_str().is_empty() {
        app.overlay.show_workspace_dialog = true;
        return Task::none();
    }
    let Some(user_prompt) = app.conversation.take_pending_prompt(tab_pos) else {
        return Task::none();
    };
    app.prompt.files.enabled = false;
    launch_dialog(app, tab_pos, &model, user_prompt)
}

fn resend_session(app: &mut App) -> Task<Message> {
    let viewing_is_streaming = app.conversation.viewing_is_streaming();
    let is_new = app.conversation.viewing().center_pane_title == "New session";

    if viewing_is_streaming || is_new {
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
    tab.session_state.start_index = tab.session.total_turns();
    tab.session_state.auto_scroll.store(true, Ordering::Relaxed);

    // Create a fresh mpsc channel for this stream's ask-tool responses.
    let (ask_tx, ask_rx) = tokio::sync::mpsc::unbounded_channel();
    tab.session_state.ask_sender = ask_tx;

    let mut tools = app
        .tools
        .tool_registry
        .enabled_tools(&app.tools.enabled_tools, &app.tools.enabled_mcp_servers);
    // Bind the todo tool to this tab's own list so parallel sessions don't clobber each other's todos.
    if let Some(pos) = tools.iter().position(|t| t.name() == "todo") {
        tools[pos] = std::sync::Arc::new(crate::tools::todo::TodoTool::new(tab.todo_items.clone()));
    }
    let config = crate::llm::SendConfig {
        model,
        workspace: app.prompt.workspace.1.clone(),
        system_prompt: app.prompt.get_system_prompt(),
        user_prompt,
        tools,
        injected_prompt: tab.session_state.injected_prompt.clone(),
        ask_receiver: ask_rx,
        user_agent: crabot::app_title().to_string(),
        cancel_token: tab.session_state.cancel_token.clone(),
    };

    let history = tab.session.history.clone();

    tab.session_state.phase = DialogPhase::LlmLoading;
    tab.end_status = None;
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
