use iced::{Task, widget};

use crabot::HashSetExt;
use crabot::chat::{Turn, TurnBody};
use crabot::model::ModelConfig;
use crabot::session::Session;
use crabot::user::{UserPrompt, WorkMode};
use futures::{SinkExt, future::FutureExt};
use genai::chat::ChatRole;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::app::session_state::{self, AskAction, SessionEvent, TaskRequest};
use crate::app::session_tab::SessionTab;
use crate::app::snapshot;
use crate::app::{App, ConversationEvent, FocusedTarget, Message, TabBarScrollState};
use crate::llm::DialogPhase;
use crate::tools::TASK_MODES;
use crate::views::export;
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
        ConversationEvent::ExportSessionHtml => {
            return export_session_html(app);
        }
        ConversationEvent::ExportSessionHtmlDone(outcome) => match outcome {
            export::ExportOutcome::Cancelled | export::ExportOutcome::Saved => {}
            export::ExportOutcome::SavedButNotOpened(error) => {
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("Export saved")
                    .set_description(&error)
                    .show();
            }
            export::ExportOutcome::Failed(error) => {
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("Export session failed")
                    .set_description(&error)
                    .show();
            }
        },
        ConversationEvent::AppClosing => {
            app.conversation.stop();
            app.save_settings();
            snapshot::cleanup_all(app);
            return iced::exit();
        }
        ConversationEvent::NewSession => {
            let tab = app.conversation.viewing();
            let model = tab.selected_model.clone();
            let preamble = tab.selected_preamble.clone();
            return new_session(app, model, preamble, String::new());
        }
        ConversationEvent::LoadSession(entry) => return load_session(app, entry),
        ConversationEvent::SwitchTab(number) => return switch_tab(app, number),
        ConversationEvent::SwitchTabByDigit(digit) => return switch_tab_by_digit(app, digit),
        ConversationEvent::CloseTab(number) => return close_tab(app, number),
        ConversationEvent::CloseCurrentTab => {
            let number = app.conversation.viewing_tab_number();
            return close_tab(app, number);
        }
        ConversationEvent::AskAction(action) => return ask_action(app, action),
        ConversationEvent::SessionListLoaded(workspace, entries) => {
            if workspace == app.prompt.workspace.1 {
                app.conversation.session_list = entries.clone();
                app.conversation.session_list_loading = false;
            }
            app.conversation
                .session_list_cache
                .insert(workspace, entries);
        }
        ConversationEvent::WorkspaceContentReady(scan) => {
            // Only apply if the workspace hasn't changed while the scan was in flight.
            if scan.workspace == app.prompt.workspace.1 {
                app.prompt.files.content =
                    widget::text_editor::Content::with_text(&scan.files_tree);
                app.prompt.agents_md_exists = scan.agents_md_exists;
                app.prompt.agents_md.1 = scan.agents_md_content;
                if let Some(preferred) = scan.agents_md_preferred {
                    app.prompt.agents_md.0 = preferred && scan.agents_md_exists;
                }
            }
            return Task::none();
        }
        ConversationEvent::TaskSpawnReady(spawn) => return continue_task_spawn(app, *spawn),
        ConversationEvent::RenewSpawnReady(spawn) => return continue_renew_spawn(app, *spawn),
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
            if expanded {
                // Collapsing a dialog also collapses its tool turns, so
                // re-expanding shows compact headers instead of full results.
                collapse_tool_turns(tab, Some(index));
            }
            tab.search.invalidate_offsets();
            app.layout.focused = None;
        }
        ConversationEvent::ToggleAllDialogsExpand => {
            let tab = app.conversation.viewing_mut();
            if tab.expanded_dialogs.is_empty() {
                tab.expanded_dialogs.extend(0..tab.session.dialogs.len());
            } else {
                tab.expanded_dialogs.clear();
                // Collapsing every dialog also collapses every tool turn.
                collapse_tool_turns(tab, None);
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

/// Open a save dialog and export the viewing session to an HTML file.
fn export_session_html(app: &App) -> Task<Message> {
    let tab = app.conversation.viewing();
    let title = tab.center_pane_title.clone();
    let file_name = export::default_export_filename(&title);
    let session = tab.session.clone();
    let expanded_dialogs = tab.expanded_dialogs.clone();
    let expanded_turns = tab.expanded_turns.clone();

    Task::perform(
        async move {
            let Some(path) = rfd::FileDialog::new().set_file_name(file_name).save_file() else {
                return export::ExportOutcome::Cancelled;
            };
            let html =
                export::render_session_html(&session, &title, &expanded_dialogs, &expanded_turns);
            if let Err(e) = std::fs::write(&path, html) {
                return export::ExportOutcome::Failed(format!(
                    "Failed to write {}: {e}",
                    path.display()
                ));
            }
            match open::that(&path) {
                Ok(()) => export::ExportOutcome::Saved,
                Err(e) => export::ExportOutcome::SavedButNotOpened(format!(
                    "Wrote {} but could not open it in a browser: {e}",
                    path.display()
                )),
            }
        },
        |result| Message::Conversation(ConversationEvent::ExportSessionHtmlDone(result)),
    )
}

/// New session tab that becomes the viewing one (user/renew tabs);
/// `parent` is the spawning session's id, empty for user tabs.
fn new_session(
    app: &mut App,
    selected_model: String,
    selected_preamble: String,
    parent: String,
) -> Task<Message> {
    let number = app.conversation.next_tab_number();
    let mut tab = SessionTab::new(number, selected_model, selected_preamble);
    tab.session.parent = parent;
    app.conversation.session_tabs.push(tab);
    app.layout.focused = None;

    app.conversation.viewing = app.conversation.session_tabs.len() - 1;

    // Refresh workspace-dependent fields (files tree + AGENTS.md land async).
    let content_task = crate::app::prompt::refresh_workspace_content(app);

    // Fresh tab has no saved scroll offset — scroll to top.
    views::scroll_to_start().discard().chain(content_task)
}

/// Background session tab (task sub-agents) — Returns the new tab's position.
fn new_background_session(
    app: &mut App,
    selected_model: String,
    selected_preamble: String,
    parent: String,
) -> usize {
    let number = app.conversation.next_tab_number();
    let mut tab = SessionTab::new(number, selected_model, selected_preamble);
    tab.session.parent = parent;
    app.conversation.session_tabs.push(tab);
    app.layout.focused = None;
    app.conversation.session_tabs.len() - 1
}

/// Synchronise the left-pane workspace fields to match the session's workspace.
fn sync_prompt_workspace(app: &mut App, workspace: PathBuf) -> Task<Message> {
    // Skip fresh tabs (empty workspace) and tabs already in sync.
    if workspace.as_os_str().is_empty()
        || !workspace.is_dir()
        || workspace == app.prompt.workspace.1
    {
        return Task::none();
    }
    // Recents are left untouched for this tab-switch driven sync.
    crate::app::prompt::apply_workspace(app, workspace, false)
}

/// Restore app state to match the newly-viewed tab.
fn restore_viewing_tab(app: &mut App) -> Task<Message> {
    let (session_workspace, scroll_offset) = {
        let tab = app.conversation.viewing();
        (tab.session.workspace.clone(), tab.scroll_offset)
    };
    let workspace_task = sync_prompt_workspace(app, session_workspace);
    views::scroll_to(scroll_offset)
        .discard()
        .chain(workspace_task)
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

    let restore_task = restore_viewing_tab(app);

    // Focus the ask input so the user can answer the ask tool immediately.
    let focus_task = widget::operation::focus(ASK_INPUT.clone());
    restore_task.chain(focus_task)
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
    let removed_model = app.conversation.session_tabs[pos].selected_model.clone();
    let removed_preamble = app.conversation.session_tabs[pos].selected_preamble.clone();
    let removed_session = &app.conversation.session_tabs[pos].session;
    snapshot::cleanup(&removed_session.workspace, &removed_session.id);
    app.conversation.session_tabs.remove(pos);
    // Clean up the pending-ask queue — the tab is gone.
    app.conversation.pending_ask_queue.retain(|&n| n != number);

    if app.conversation.session_tabs.is_empty() {
        let number = app.conversation.next_tab_number();
        app.conversation.session_tabs.push(SessionTab::new(
            number,
            removed_model,
            removed_preamble,
        ));
        app.conversation.viewing = 0;
    } else if pos < app.conversation.viewing {
        app.conversation.viewing -= 1;
    } else if app.conversation.viewing >= app.conversation.session_tabs.len() {
        app.conversation.viewing = app.conversation.session_tabs.len().saturating_sub(1);
    }

    app.layout.focused = None;

    // Restore the newly-viewed tab's state if the closed tab was viewing.
    if was_viewing {
        return restore_viewing_tab(app);
    }
    Task::none()
}

/// Remove tool-turn expansion keys from `expanded_turns`.
/// `dialog_index` of `None` covers every dialog.
fn collapse_tool_turns(tab: &mut SessionTab, dialog_index: Option<usize>) {
    let mut flat_idx: usize = 0;
    for (di, dialog) in tab.session.dialogs.iter().enumerate() {
        if dialog_index.is_none_or(|target| target == di) {
            for (offset, turn) in dialog.turns.iter().enumerate() {
                if let TurnBody::Tool(trs) = &turn.body {
                    for sub in 0..trs.len() {
                        tab.expanded_turns.remove(&(flat_idx + offset, sub));
                    }
                }
            }
        }
        flat_idx += dialog.turns.len();
    }
}

fn load_session(app: &mut App, entry: views::session_list::SessionEntry) -> Task<Message> {
    // If the session is already open in a tab, just switch to it.
    if let Some(existing) = app
        .conversation
        .session_tabs
        .iter()
        .position(|t| t.session.id == entry.id)
    {
        let tab = &mut app.conversation.session_tabs[existing];
        tab.expanded_dialogs.clear();
        tab.search.invalidate_offsets();
        if existing != app.conversation.viewing {
            let task = switch_tab(app, app.conversation.session_tabs[existing].number);
            app.layout.focused = Some(FocusedTarget::SessionPicker);
            return task;
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
            // If the loaded session has a stored model, find the matching model name;
            // otherwise keep the viewing tab's current selection.
            let selected_model = if let Some(ref model_config) = session.model {
                app.find_model_label(model_config)
            } else {
                app.conversation.viewing().selected_model.clone()
            };
            let selected_preamble = app.conversation.viewing().selected_preamble.clone();
            let tab = app.conversation.viewing_mut();
            *tab = SessionTab::from_session(number, session, selected_model, selected_preamble);
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
        AskAction::Extend => {
            app.conversation
                .viewing_mut()
                .session_state
                .extend_ask_deadline();
            return Task::none();
        }
        AskAction::Ok => Ok(app.conversation.viewing().session_state.ask_input.clone()),
        AskAction::Skip => Ok("No preference. Use your best judgment.".into()),
        AskAction::NoneApply => Ok("None of the options apply.".into()),
    };
    let _ = app
        .conversation
        .viewing_mut()
        .session_state
        .ask_sender
        .as_ref()
        .map(|sender| sender.send(result));
    app.conversation.viewing_mut().session_state.ask_request = None;

    // After answered, switch to the next pending tab that issued an ask.
    process_pending_ask_queue(app)
}

// ── streaming events ───────────────────────────────────────────────

/// The tab's effective model config: the session's model, or tab's selected model label.
fn tab_model_config(app: &App, tab: &SessionTab) -> Option<ModelConfig> {
    tab.session
        .model
        .clone()
        .or_else(|| app.models.get_config(&tab.selected_model).cloned())
}

/// The workspace a tab's session actually runs in: the session's stored
/// workspace, falling back to the current prompt workspace.
fn tab_workspace(app: &App, tab: &SessionTab) -> PathBuf {
    let ws = &tab.session.workspace;
    if ws.as_os_str().is_empty() || !ws.is_dir() {
        app.prompt.workspace.1.clone()
    } else {
        ws.clone()
    }
}

/// Whether the session started with a workspace layout — a successor session
/// should then receive a freshly rebuilt tree. Cheap: no filesystem access.
fn session_started_with_tree(tab: &SessionTab) -> bool {
    tab.session.history.first().is_some_and(|m| {
        m.role == ChatRole::User
            && m.content.parts().iter().any(|p| {
                p.as_text()
                    .is_some_and(|t| t.starts_with("Working directory layout"))
            })
    })
}

/// Point the prompt workspace at `workspace` before spawning a successor
/// session — `start_dialog` derives the stream workspace from it.
fn sync_spawn_workspace(app: &mut App, workspace: &Path) -> Task<Message> {
    if workspace.as_os_str().is_empty() || workspace == app.prompt.workspace.1 {
        Task::none()
    } else {
        crate::app::prompt::sync_workspace(app, workspace.to_path_buf())
    }
}

/// Send the task-tool result, tagged with its call_id, to the parent tab's
/// waiting stream (if still open).
fn deliver_task_report(
    app: &mut App,
    parent_number: usize,
    call_id: String,
    result: Result<String, String>,
) {
    if let Some(pos) = app.conversation.tab_pos(parent_number)
        && let Some(sender) = app.conversation.session_tabs[pos]
            .session_state
            .task_sender
            .as_ref()
    {
        let _ = sender.send((call_id, result));
    }
}

/// Deliver a failure report for a task spawn and stop.
fn fail_task_spawn(
    app: &mut App,
    number: usize,
    call_id: String,
    message: String,
) -> Task<Message> {
    deliver_task_report(app, number, call_id, Err(message));
    Task::none()
}

/// Which tool triggered this successor spawn — each carries its own payload.
#[derive(Debug, Clone)]
pub(crate) enum SpawnKind {
    Renew {
        mode: Option<WorkMode>,
    },
    Task {
        call_id: String,
        title: Option<String>,
        preamble: Option<String>,
    },
}

/// Prepared successor-session spawn (renew/task tool) whose blocking workspace
/// scan has completed off the UI thread; the continuation message completes the spawn.
#[derive(Debug, Clone)]
pub(crate) struct SuccessorSpawn {
    /// Origin/parent tab number.
    pub(crate) number: usize,
    pub(crate) selected_model: String,
    pub(crate) selected_preamble: String,
    pub(crate) model: ModelConfig,
    pub(crate) workspace: PathBuf,
    /// Rebuilt "Working directory layout" tree (None when the origin session
    /// didn't start with one).
    pub(crate) workspace_tree: Option<String>,
    /// Which tool triggered the spawn and its mode-specific data.
    pub(crate) kind: SpawnKind,
    /// Prompt to launch in the new session.
    pub(crate) prompt: String,
}

/// Handle a renew-tool request: create a new session tab and launch the
/// continuation prompt on it, using the same model and work mode as the
/// originating session. The workspace tree is rebuilt off the UI thread; the
/// spawn itself happens in [`continue_renew_spawn`] once the scan lands.
fn handle_renew(app: &mut App, number: usize, prompt: String) -> Task<Message> {
    let (model, work_mode, workspace, need_tree, selected_model, selected_preamble) = {
        let Some(pos) = app.conversation.tab_pos(number) else {
            return Task::none();
        };
        let tab = &app.conversation.session_tabs[pos];
        let Some(model) = tab_model_config(app, tab) else {
            return Task::none();
        };
        (
            model,
            tab.session.dialogs.last().and_then(|d| d.mode),
            tab_workspace(app, tab),
            session_started_with_tree(tab),
            tab.selected_model.clone(),
            tab.selected_preamble.clone(),
        )
    };
    let ws = workspace.clone();
    Task::perform(
        async move {
            if need_tree {
                tokio::task::spawn_blocking(move || crabot::workspace::build_files_tree(&ws))
                    .await
                    .ok()
            } else {
                None
            }
        },
        move |workspace_tree| {
            Message::Conversation(ConversationEvent::RenewSpawnReady(Box::new(
                SuccessorSpawn {
                    number,
                    selected_model,
                    selected_preamble,
                    model,
                    workspace,
                    workspace_tree,
                    kind: SpawnKind::Renew { mode: work_mode },
                    prompt,
                },
            )))
        },
    )
}

/// Complete a renew spawn once its workspace scan has landed off-thread.
fn continue_renew_spawn(app: &mut App, spawn: SuccessorSpawn) -> Task<Message> {
    let workspace_task = sync_spawn_workspace(app, &spawn.workspace);
    let parent_id = app
        .conversation
        .tab_pos(spawn.number)
        .map(|p| app.conversation.session_tabs[p].session.id.clone())
        .unwrap_or_default();
    let new_task = new_session(
        app,
        spawn.selected_model,
        spawn.selected_preamble,
        parent_id,
    );
    let tab_pos = app.conversation.viewing;
    let SpawnKind::Renew { mode } = &spawn.kind else {
        return Task::none();
    };
    let user_prompt = UserPrompt::new(*mode, spawn.prompt, spawn.workspace_tree);
    let launch_task = launch_dialog(app, tab_pos, &spawn.model, user_prompt, None);
    workspace_task.chain(new_task).chain(launch_task)
}

// ── task tool (sub-agent) ─────────────────────────────────────────

/// Handle a task-tool request: spawn a sub-agent session tab with a mode
/// preamble and difficulty-selected model, delivering its final report to the
/// parent on completion. The workspace scan runs off the UI thread; the spawn
/// itself happens in [`continue_task_spawn`].
fn handle_task_request(app: &mut App, number: usize, request: TaskRequest) -> Task<Message> {
    let Some(pos) = app.conversation.tab_pos(number) else {
        return Task::none();
    };
    let TaskRequest {
        call_id,
        title,
        prompt,
        mode,
        difficulty,
    } = request;
    // Resolve the parent tab's context (cheap — no filesystem access).
    let (parent_selected, parent_selected_preamble, parent_model, workspace, need_tree) = {
        let parent = &app.conversation.session_tabs[pos];
        (
            parent.selected_model.clone(),
            parent.selected_preamble.clone(),
            tab_model_config(app, parent),
            tab_workspace(app, parent),
            session_started_with_tree(parent),
        )
    };

    // Difficulty → configured subtask model (fallback: the parent's model).
    let difficulty = difficulty.as_deref().unwrap_or("medium");
    let configured = app.settings.task_models.get_config(difficulty);
    let (selected_model, model) = if configured.is_empty() {
        // Empty config means "inherit the parent session's model".
        match parent_model {
            Some(cfg) => (parent_selected, cfg),
            None => {
                return fail_task_spawn(
                    app,
                    number,
                    call_id.clone(),
                    "No model available for the subtask.".into(),
                );
            }
        }
    } else {
        (app.find_model_label(configured), configured.clone())
    };
    // Stale config would silently no-op `start_dialog` and strand the parent.
    if app.models.get_model_info(&model).is_none() {
        return fail_task_spawn(
            app,
            number,
            call_id,
            format!(
                "Subtask model is no longer resolvable: '{}' (provider '{}' / model '{}').",
                selected_model, model.provider_id, model.model_id
            ),
        );
    }

    // The sub-agent inherits the parent's workspace; the tree and mode
    // preamble are scanned off the UI thread.
    let ws = workspace.clone();
    Task::perform(
        async move {
            let tree = if need_tree {
                tokio::task::spawn_blocking(move || crabot::workspace::build_files_tree(&ws))
                    .await
                    .ok()
            } else {
                None
            };
            // Load the mode preamble; record the mode name only when a preamble file exists.
            let (preamble, preamble_name) = tokio::task::spawn_blocking(move || {
                let name = mode.as_deref().map(str::to_ascii_lowercase);
                let content = task_mode_preamble(name.as_deref());
                let name_used = name.filter(|_| content.is_some());
                (content, name_used)
            })
            .await
            .ok()
            .unwrap_or((None, None));
            (tree, preamble, preamble_name)
        },
        move |(workspace_tree, preamble, preamble_name)| {
            Message::Conversation(ConversationEvent::TaskSpawnReady(Box::new(
                SuccessorSpawn {
                    number,
                    selected_model,
                    selected_preamble: preamble_name.unwrap_or(parent_selected_preamble),
                    model,
                    workspace,
                    workspace_tree,
                    kind: SpawnKind::Task {
                        call_id,
                        title,
                        preamble,
                    },
                    prompt,
                },
            )))
        },
    )
}

/// Complete a task-tool spawn once its workspace scan and preamble read finish.
fn continue_task_spawn(app: &mut App, spawn: SuccessorSpawn) -> Task<Message> {
    // Parent tab may have been closed while the scan was in flight.
    let Some(parent_pos) = app.conversation.tab_pos(spawn.number) else {
        return Task::none();
    };
    let SpawnKind::Task {
        call_id,
        title,
        preamble,
    } = spawn.kind
    else {
        return Task::none();
    };
    let system_prompt = preamble.as_deref().map(|preamble| {
        app.prompt
            .compose_system_prompt(Some(preamble), &app.settings.selected_rules)
    });
    let workspace_task = sync_spawn_workspace(app, &spawn.workspace);
    let parent = &app.conversation.session_tabs[parent_pos];
    let parent_path = parent.task_path.clone();
    let parent_id = parent.session.id.clone();
    // Spawn in the background — the parent keeps the view; the new tab shows
    // a running indicator and can be clicked to inspect.
    let tab_pos = new_background_session(
        app,
        spawn.selected_model,
        spawn.selected_preamble,
        parent_id,
    );
    let tab_model_label = app.find_model_label(&spawn.model);
    {
        let tab = &mut app.conversation.session_tabs[tab_pos];
        let mut task_path = parent_path.unwrap_or_else(|| vec![spawn.number]);
        task_path.push(tab.number);
        tab.task_path = Some(task_path);
        tab.task_call_id = Some(call_id);
        tab.selected_model = tab_model_label;
    }
    let user_prompt = UserPrompt::new(None, spawn.prompt, spawn.workspace_tree);
    let launch_task = launch_dialog(app, tab_pos, &spawn.model, user_prompt, system_prompt);
    // Prefer the tool-provided title for the tab heading when available.
    if let Some(title) = title.filter(|t| !t.trim().is_empty()) {
        let tab = &mut app.conversation.session_tabs[tab_pos];
        tab.center_pane_title = title.clone();
        tab.session.title = title;
    }
    workspace_task.chain(launch_task)
}

/// Load the mode-specific sub-agent preamble (`~/.crabot/preamble/{mode}.md`).
/// Only the modes from the task schema are recognized; anything else
/// (including path-traversal attempts) falls back to the default prompt.
fn task_mode_preamble(mode: Option<&str>) -> Option<String> {
    let mode = mode?.to_ascii_lowercase();
    if !TASK_MODES.contains(&mode.as_str()) {
        return None;
    }
    let path = crabot::setup::default_workspace_path()
        .join("preamble")
        .join(format!("{mode}.md"));
    std::fs::read_to_string(path)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Extract the last non-empty assistant text from a finished session — the
/// sub-agent's final report returned to the parent as the task tool result.
fn final_assistant_text(session: &Session) -> Option<String> {
    session
        .history
        .iter()
        .rev()
        .filter(|m| m.role == ChatRole::Assistant)
        .find_map(|m| {
            let text = m.content.joined_texts().unwrap_or_default();
            (!text.trim().is_empty()).then_some(text)
        })
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
    // Handle TaskRequest before the normal flow — it spawns a sub-agent session.
    if let SessionEvent::TaskRequest(ref request) = event {
        return handle_task_request(app, number, request.clone());
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
    let model_config = tab_model_config(app, &app.conversation.session_tabs[pos]);
    let cost = model_config
        .as_ref()
        .and_then(|cfg| app.models.get_model(cfg))
        .map(|m| m.cost.clone());
    let context_window = model_config.as_ref().map(|cfg| cfg.context_window);

    let finished = matches!(event, SessionEvent::Done(_));
    let is_cancelled = matches!(event, SessionEvent::Cancelled(_));
    let task_error = match &event {
        SessionEvent::Error(err, _) => Some(err.clone()),
        _ => None,
    };

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

    // Task sub-agent that just terminated — deliver its final report to the parent.
    if let Some(parent) = app.conversation.session_tabs[pos].task_parent()
        && (finished || is_cancelled || task_error.is_some())
        && let Some(call_id) = app.conversation.session_tabs[pos].task_call_id.take()
    {
        let result = if let Some(err) = task_error {
            Err(format!("Subtask failed: {err}"))
        } else if is_cancelled {
            Err("Subtask was cancelled.".into())
        } else {
            Ok(
                final_assistant_text(&app.conversation.session_tabs[pos].session).unwrap_or_else(
                    || "Subtask completed without a final text response.".to_string(),
                ),
            )
        };
        deliver_task_report(app, parent, call_id, result);
    }

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

    launch_dialog(app, tab_pos, &model, user_prompt, None)
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
/// `system_prompt_override` replaces the configured system prompt (used by the
/// task tool to inject a mode-specific sub-agent preamble).
fn launch_dialog(
    app: &mut App,
    tab_pos: usize,
    model: &ModelConfig,
    user_prompt: UserPrompt,
    system_prompt_override: Option<String>,
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
    start_dialog(
        app,
        tab_pos,
        model,
        Some(user_prompt),
        system_prompt_override,
    )
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
    launch_dialog(app, tab_pos, &model, user_prompt, None)
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
    start_dialog(app, tab_pos, &model, None, None)
}

// ── Stream orchestration ──────────────────────────────────────────

/// Prepare and launch an LLM dialog stream for the given tab.
/// `system_prompt_override` replaces the configured system prompt when set
/// (used by the task tool to inject a mode-specific sub-agent preamble).
pub(crate) fn start_dialog(
    app: &mut App,
    tab_pos: usize,
    model_config: &ModelConfig,
    user_prompt: Option<UserPrompt>,
    system_prompt_override: Option<String>,
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
        app.conversation
            .session_list_cache
            .entry(app.prompt.workspace.1.clone())
            .or_default()
            .insert(0, entry.clone());
        app.conversation.session_list.insert(0, entry);
    }

    // Compute the system prompt before re-borrowing the tab (needs `&app`).
    // The task tool passes an override (mode preamble); otherwise assemble
    // from the configured components, reading preamble/rules from disk.
    let system_prompt = match system_prompt_override {
        Some(override_prompt) => override_prompt,
        None => app.prompt.get_system_prompt(
            &app.conversation.session_tabs[tab_pos].selected_preamble,
            &app.settings.selected_rules,
        ),
    };

    // Re-borrow for the remaining setup.
    let tab = &mut app.conversation.session_tabs[tab_pos];
    tab.session_state.start_index = tab.session.total_turns();
    tab.session_state.auto_scroll.store(true, Ordering::Relaxed);

    // Fresh ask-response / task-report channels and a fresh ask deadline for this stream.
    let (ask_tx, ask_rx) = tokio::sync::mpsc::unbounded_channel();
    tab.session_state.ask_sender = Some(ask_tx);
    tab.session_state.ask_deadline = Arc::new(Mutex::new(Instant::now()));
    let (task_tx, task_rx) = tokio::sync::mpsc::unbounded_channel();
    tab.session_state.task_sender = Some(task_tx);

    let mut tools = app
        .tools
        .tool_registry
        .enabled_tools(&app.tools.enabled_tools, &app.tools.enabled_mcp_servers);
    // Bind the todo tool to this tab's own list so parallel sessions don't clobber each other's todos.
    if let Some(pos) = tools.iter().position(|t| t.name() == "todo") {
        tools[pos] = std::sync::Arc::new(crate::tools::todo::TodoTool::new(tab.todo_items.clone()));
    }
    // Sub-agent sessions spawned by the task tool cannot call renew.
    if tab.task_path.is_some() {
        tools.retain(|t| t.name() != "renew");
    }
    tab.session_state.phase = DialogPhase::LlmLoading;
    tab.end_status = None;
    // Fresh token per stream: a stale cancel can't leak into the next run.
    tab.session_state.cancel_token = CancellationToken::new();
    let cancel_token = tab.session_state.cancel_token.clone();

    let config = crate::llm::SendConfig {
        model,
        workspace: app.prompt.workspace.1.clone(),
        session_id: tab.session.id.clone(),
        system_prompt,
        user_prompt,
        tools,
        injected_prompt: tab.session_state.injected_prompt.clone(),
        ask_receiver: ask_rx,
        ask_deadline: tab.session_state.ask_deadline.clone(),
        task_receiver: task_rx,
        user_agent: crabot::app_title().to_string(),
        cancel_token: cancel_token.clone(),
        max_iterations: app.settings.max_iterations,
        stream_stall_timeout_secs: app.settings.stream_stall_timeout,
    };

    let history = tab.session.history.clone();

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
                        if cancel.is_cancelled() { false } else { ok }
                    }
                    .boxed()
                }
            };
            crate::llm::send_stream(config, history, &mut callback).await;
        })),
    ])
}

/// Refresh the session list dropdown entries from disk.
pub(crate) fn refresh_session_list(workspace: PathBuf) -> Task<Message> {
    if workspace.as_os_str().is_empty() {
        return Task::none();
    }
    Task::perform(
        async move {
            let path = workspace.clone();
            let entries = tokio::task::spawn_blocking(move || {
                crate::views::session_list::list_entries(&path)
            })
            .await
            .unwrap_or(Ok(Vec::new()))
            .unwrap_or_default();
            (workspace, entries)
        },
        |(workspace, entries)| {
            Message::Conversation(ConversationEvent::SessionListLoaded(workspace, entries))
        },
    )
}
