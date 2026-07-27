use iced::{Task, widget::text_editor};

use crabot::HashSetExt;
use crabot::chat::Turn;
use crabot::model::{Cost, ModelConfig};
use crabot::session::Session;
use crabot::user::UserPrompt;
use futures::{SinkExt, future::FutureExt};
use std::sync::atomic::Ordering;

use crate::app::session_state::{self, AskAction, SessionEvent};
use crate::app::{App, ConversationEvent, ConversationState, FocusedTarget, Message, ToolState};
use crate::llm::DialogPhase;
use crate::views::{self, SCROLL_STEP, scroll_to_end};
use crate::widgets::textarea::TextArea;

pub(crate) fn update(app: &mut App, event: ConversationEvent) -> Task<Message> {
    match event {
        ConversationEvent::NavigateSession(up) => {
            return navigate_session(&mut app.conversation, &app.layout.focused, up);
        }
        ConversationEvent::ResendSessionHistory => return resend_session(app),
        ConversationEvent::SessionEvent(event) => {
            let cost = app.current_model_cost();
            return session_event(
                &mut app.conversation,
                &mut app.tools,
                &mut app.prompt.user_prompt,
                cost,
                event,
            );
        }
        ConversationEvent::SearchEvent(event) => {
            return views::search_bar::update(event, &mut app.conversation)
                .map(Message::Conversation);
        }
        ConversationEvent::CopySessionTitle => {
            return iced::clipboard::write(app.conversation.center_pane_title.clone());
        }
        ConversationEvent::AppClosing => {
            app.conversation
                .session_state
                .cancel_token
                .store(true, Ordering::Release);
            app.save_settings();
            return iced::exit();
        }
        ConversationEvent::NewSession => new_session(app),
        ConversationEvent::LoadSession(entry) => {
            load_session(&mut app.conversation, &mut app.tools, entry)
        }
        ConversationEvent::AskAction(action) => ask_action(&mut app.conversation, action),
        ConversationEvent::SessionListLoaded(entries) => {
            app.conversation.session_list = entries;
        }
        ConversationEvent::ToggleTurnExpand(index, sub_index) => {
            let key = (index, sub_index);
            let expanded = app.conversation.expanded_turns.contains(&key);
            app.conversation.expanded_turns.set(key, !expanded);
            app.conversation.search.invalidate_offsets();
            app.layout.focused = None;
        }
        ConversationEvent::ToggleDialogExpand(index) => {
            let expanded = app.conversation.expanded_dialogs.contains(&index);
            app.conversation.expanded_dialogs.set(index, !expanded);
            app.conversation.search.invalidate_offsets();
            app.layout.focused = None;
        }
        ConversationEvent::ToggleAllDialogsExpand => {
            if app.conversation.expanded_dialogs.is_empty() {
                app.conversation
                    .expanded_dialogs
                    .extend(0..app.conversation.session.dialogs.len());
            } else {
                app.conversation.expanded_dialogs.clear();
            }
            app.conversation.search.invalidate_offsets();
            app.layout.focused = None;
        }
        ConversationEvent::SessionPickerFocused => {
            app.layout.focused = Some(FocusedTarget::SessionPicker);
        }
        ConversationEvent::DefocusSessionPicker => {
            app.layout.focused = None;
        }
        ConversationEvent::AskInputChanged(input) => {
            app.conversation.session_state.ask_input = input;
        }
        ConversationEvent::ToggleSelectableMode(index) => match index {
            Some(index) => {
                let selected = app.conversation.selectable_msgs.contains(&index);
                app.conversation.selectable_msgs.set(index, !selected);
            }
            None => app.conversation.selectable_msgs.clear(),
        },
        ConversationEvent::TurnOffsetsMeasured(generation, offsets) => {
            app.conversation.search.handle_offsets(generation, offsets);
        }
    }
    Task::none()
}

fn new_session(app: &mut App) {
    app.conversation.session = Session::new();
    app.conversation.session_state = session_state::SessionState::new();
    app.conversation.center_pane_title = "New session".into();
    app.conversation.last_usage = genai::chat::Usage::default();
    app.conversation.expanded_turns.clear();
    app.conversation.expanded_dialogs.clear();
    app.conversation.selectable_msgs.clear();
    app.conversation.search.reset();
    app.tools.cached_todo_items.clear();
    app.tools.tool_registry.clear_todo();

    let workspace = app.prompt.workspace.1.clone();
    let tree = crabot::workspace::build_files_tree(&workspace);
    app.prompt.files.content = text_editor::Content::with_text(&tree);
    app.prompt.files.enabled = true;
    let (exists, content) = crate::app::prompt::load_agents_md(&workspace);
    app.prompt.agents_md_exists = exists;
    app.prompt.agents_md.1 = content;
}

fn load_session(
    conversation: &mut ConversationState,
    tools: &mut ToolState,
    entry: views::session_list::SessionEntry,
) {
    if conversation.session_state.phase != DialogPhase::Idle {
        return;
    }
    match Session::load(&entry.path) {
        Ok(session) => conversation.session = session,
        Err(error) => {
            conversation.session = Session::new();
            conversation.session.id = entry.id;
            eprintln!("Failed to load session: {error}");
        }
    }
    conversation.last_usage = genai::chat::Usage {
        prompt_tokens: Some(conversation.session.tokens.prompt),
        ..Default::default()
    };
    conversation.center_pane_title = conversation.session.title.clone();
    conversation.expanded_turns.clear();
    conversation.expanded_dialogs.clear();
    conversation.selectable_msgs.clear();
    conversation.search.reset();
    tools.cached_todo_items = conversation.session.last_todo_items();
}

fn navigate_session(
    conversation: &mut ConversationState,
    focused: &Option<FocusedTarget>,
    up: bool,
) -> Task<Message> {
    if *focused != Some(FocusedTarget::SessionPicker)
        || conversation.session_state.phase != DialogPhase::Idle
        || conversation.session_list.is_empty()
    {
        return views::scroll_by(if up { -SCROLL_STEP } else { SCROLL_STEP }).discard();
    }

    let current = conversation
        .session_list
        .iter()
        .position(|entry| entry.id == conversation.session.id);
    let entry = match current {
        Some(index) => Some({
            let next = if up {
                index
                    .checked_sub(1)
                    .unwrap_or_else(|| conversation.session_list.len().saturating_sub(1))
            } else if index + 1 < conversation.session_list.len() {
                index + 1
            } else {
                0
            };
            conversation.session_list[next].clone()
        }),
        None if up => conversation.session_list.last().cloned(),
        None => conversation.session_list.first().cloned(),
    };
    entry.map_or_else(Task::none, |entry| {
        Task::done(Message::Conversation(ConversationEvent::LoadSession(entry)))
    })
}

fn ask_action(conversation: &mut ConversationState, action: AskAction) {
    let result = match action {
        AskAction::OptionSelected(option) => {
            conversation.session_state.ask_input = option;
            return;
        }
        AskAction::Ok => Ok(conversation.session_state.ask_input.clone()),
        AskAction::Skip => Ok("No preference. Use your best judgment.".into()),
    };
    let _ = conversation.session_state.ask_sender.send(result);
    conversation.session_state.ask_request = None;
}

fn session_event(
    conversation: &mut ConversationState,
    tools: &mut ToolState,
    user_prompt: &mut TextArea,
    model_cost: Option<Cost>,
    event: SessionEvent,
) -> Task<Message> {
    if let SessionEvent::ToolResult(ref result) = event
        && result.name == "todo"
    {
        tools.cached_todo_items = tools.tool_registry.snapshot_todo();
    }
    session_state::update(event, conversation, model_cost, user_prompt).discard()
}

pub(crate) fn send_prompt(app: &mut App) -> Task<Message> {
    let raw = app.prompt.user_prompt.text();
    let content = crabot::tools::normalize_newlines(&raw).into_owned();
    if content.trim().is_empty() {
        return Task::none();
    }
    let Some(model) = selected_model_config(app) else {
        return Task::none();
    };
    if app.prompt.workspace.1.as_os_str().is_empty() {
        app.overlay.show_workspace_dialog = true;
        return Task::none();
    }

    let mode = app.prompt.workmode_enabled.then_some(app.prompt.workmode);
    let workspace_tree = if app.prompt.files.enabled {
        let tree = app.prompt.files.content.text();
        (!tree.is_empty()).then(|| tree.to_string())
    } else {
        None
    };
    let user_prompt = UserPrompt::new(mode, content.clone(), workspace_tree);
    app.prompt.files.enabled = false;
    app.prompt.user_prompt.clear();

    if app.conversation.session_state.phase != DialogPhase::Idle {
        if let Ok(mut pending) = app.conversation.session_state.pending_user_prompt.lock() {
            *pending = Some(user_prompt.content.clone());
        }
        app.conversation.session_state.pending_prompt = Some(content);
        return Task::none();
    }

    app.conversation.center_pane_title = content.clone();
    let dialog_index = app.conversation.session.dialogs.len();
    app.conversation.expanded_dialogs.clear();
    app.conversation.expanded_dialogs.insert(dialog_index);
    app.conversation
        .session
        .add_dialog(Session::derive_title(&content));
    app.conversation
        .session
        .push_turn(Turn::user(user_prompt.content.clone()));
    start_dialog(app, &model, Some(user_prompt))
}

fn resend_session(app: &mut App) -> Task<Message> {
    if app.conversation.session_state.phase != DialogPhase::Idle
        || app.conversation.center_pane_title == "New session"
    {
        return Task::none();
    }
    let Some(model) = selected_model_config(app) else {
        return Task::none();
    };
    app.conversation.expanded_dialogs.clear();
    if let Some(index) = app.conversation.session.dialogs.len().checked_sub(1) {
        app.conversation.expanded_dialogs.insert(index);
    }
    start_dialog(app, &model, None)
}

/// Look up the currently selected model's config, cloned for ownership.
fn selected_model_config(app: &App) -> Option<ModelConfig> {
    app.models.get_config(&app.settings.selected_model).cloned()
}

// ── Stream orchestration ──────────────────────────────────────────

/// Prepare and launch an LLM dialog stream for the current session.
pub(crate) fn start_dialog(
    app: &mut App,
    model_config: &ModelConfig,
    user_prompt: Option<UserPrompt>,
) -> Task<Message> {
    let Some(model) = app.models.get_model_info(model_config) else {
        return Task::none();
    };
    // When continuing with a different model, fork the session.
    let conversation = &mut app.conversation;
    let model_changed = conversation
        .session
        .model
        .as_ref()
        .is_some_and(|m| m.model_id != model_config.model_id);
    let session_forked = if model_changed && conversation.session.history.len() > 1 {
        conversation.session = conversation.session.fork();
        if model_config.model_id.starts_with("deepseek") {
            conversation.session.fix_history();
        }
        true
    } else {
        false
    };
    conversation.session.model = Some(model_config.clone());
    conversation.session.workspace = app.prompt.workspace.1.clone();
    conversation.session.save().ok();

    // Add current session to the dropdown list so it appears immediately.
    if (conversation.session.is_fresh() || session_forked)
        && let Some(path) = conversation.session.save_path()
    {
        let entry = crate::views::session_list::SessionEntry {
            id: conversation.session.id.clone(),
            title: conversation.session.title.clone(),
            path,
        };
        conversation.session_list.insert(0, entry);
    }

    // Clear any stale pending prompt from a previous stream.
    if let Ok(mut pending) = conversation.session_state.pending_user_prompt.lock() {
        *pending = None;
    }
    conversation.session_state.pending_prompt = None;
    conversation.session_state.start_index = conversation.session.total_turns();
    conversation
        .session_state
        .auto_scroll
        .store(true, Ordering::Relaxed);

    // Create a fresh mpsc channel for this stream's ask-tool responses.
    let (ask_tx, ask_rx) = tokio::sync::mpsc::unbounded_channel();
    conversation.session_state.ask_sender = ask_tx;

    let config = crate::llm::SendConfig {
        model,
        workspace: app.prompt.workspace.1.clone(),
        system_prompt: app.prompt.get_prompt(),
        user_prompt,
        tools: app
            .tools
            .tool_registry
            .enabled_tools(&app.tools.enabled_tools, &app.tools.enabled_mcp_servers),
        pending_user_prompt: conversation.session_state.pending_user_prompt.clone(),
        ask_receiver: ask_rx,
        user_agent: crabot::app_title().to_string(),
        cancel_token: conversation.session_state.cancel_token.clone(),
    };

    let history = conversation.session.history.clone();

    conversation.session_state.phase = DialogPhase::LlmLoading;
    conversation
        .session_state
        .cancel_token
        .store(false, Ordering::Relaxed);
    let cancel_token = conversation.session_state.cancel_token.clone();

    Task::batch([
        scroll_to_end().discard(),
        Task::stream(iced::stream::channel(128, async move |sender| {
            let cancel = cancel_token.clone();
            let mut callback = {
                move |msg: SessionEvent| {
                    let cancel = cancel.clone();
                    let mut sender = sender.clone();
                    async move {
                        let ok = sender
                            .send(Message::Conversation(ConversationEvent::SessionEvent(msg)))
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
