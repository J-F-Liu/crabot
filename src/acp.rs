//! ACP (Agent Client Protocol) HTTP server bridge.
//!
//! When enabled, crabot binds a loopback-only HTTP server speaking the
//! [Agent Client Protocol](https://agentclientprotocol.com) v1, so ACP clients
//! (Zed, VS Code ACP extensions, …) can create sessions, send prompts, and
//! stream assistant text. Each ACP session maps to a foreground session tab.
//! The bridge is three pieces:
//!
//! - `build_agent_connection` — the SDK `Agent` builder with JSON-RPC handlers.
//!   Long-running handlers (`session/prompt`) hand work to a spawned task so
//!   the dispatch loop stays free for `session/cancel`.
//! - A command queue (handler → UI thread) pinged through a broadcast channel;
//!   the permanent `events()` subscription tick drives `App::update`.
//! - Per-session feed channels (UI thread → handler): the stream callback in
//!   `conversation::start_dialog` calls `forward_event`, which fans each
//!   `Content`/terminal event out to every feed registered for that session.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock,
    ContentChunk, InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse,
    PromptRequest, PromptResponse, SessionId, SessionNotification, SessionUpdate,
    SetSessionModeRequest, SetSessionModeResponse, StopReason,
};
use agent_client_protocol::{Agent, Client, ConnectTo, Error, Handled};
use agent_client_protocol_http::AcpHttpServer;
use futures::stream::BoxStream;
use iced::Task;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::app::conversation;
use crate::app::session_state::SessionEvent;
use crate::app::{App, Message};
use crabot::lock;
use crabot::user::UserPrompt;

// ── Bridge state (handler thread ↔ UI thread) ─────────────────────

/// Commands from the ACP server handlers to the UI thread.
enum AcpCommand {
    NewSession {
        cwd: PathBuf,
        reply: oneshot::Sender<Result<String, String>>,
    },
    Prompt {
        session_id: String,
        text: String,
        feed: mpsc::UnboundedSender<AcpEvent>,
    },
    Cancel {
        session_id: String,
    },
}

/// Events streamed from a session tab back to a waiting ACP prompt handler.
#[derive(Clone)]
pub(crate) enum AcpEvent {
    /// Assistant text chunk.
    Text(String),
    /// Assistant reasoning chunk.
    Thought(String),
    /// Abort the turn with a JSON-RPC error.
    Error(String),
    /// The turn ended; `cancelled` maps to `StopReason::Cancelled`.
    Done { cancelled: bool },
}

static COMMAND_QUEUE: LazyLock<Mutex<VecDeque<AcpCommand>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
static COMMAND_PING: LazyLock<broadcast::Sender<()>> = LazyLock::new(|| broadcast::channel(16).0);
/// Feeds registered per session id.
static FEED_REGISTRY: LazyLock<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<AcpEvent>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Latest server's serve task, awaited by the next `start` so a rapid off→on
/// toggle cannot bind while the old listener still holds the port.
static SERVE_HANDLE: LazyLock<Mutex<Option<JoinHandle<()>>>> = LazyLock::new(|| Mutex::new(None));
/// Sessions whose turn an ACP client asked to cancel.
static CANCELLED_SESSIONS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn push_command(command: AcpCommand) {
    lock(&COMMAND_QUEUE).push_back(command);
    let _ = COMMAND_PING.send(());
}

/// Stream yielding one tick on subscribe plus one tick per queued command, so
/// the UI thread drains commands promptly.
pub(crate) fn events() -> BoxStream<'static, ()> {
    crabot::broadcast_ticks(&COMMAND_PING)
}

// ── ACP agent connection ──────────────────────────────────────────

/// Build a fresh agent connection for each ACP client connection.
fn build_agent_connection() -> impl ConnectTo<Client> {
    Agent
        .builder()
        .name("crabot")
        // initialize — advertise v1 only; `loadSession` stays unset (false)
        // because this bridge cannot replay session history yet.
        .on_receive_request(
            async |_req: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(ProtocolVersion::V1)
                        .agent_capabilities(AgentCapabilities::default()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        // authenticate — no auth methods advertised; acknowledge any call.
        .on_receive_request(
            async |_req: AuthenticateRequest, responder, _cx| {
                responder.respond(AuthenticateResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // session/new — create a crabot session tab in the foreground.
        .on_receive_request(
            async |req: NewSessionRequest, responder, _cx| {
                let (reply_tx, reply_rx) = oneshot::channel();
                push_command(AcpCommand::NewSession {
                    cwd: req.cwd.clone(),
                    reply: reply_tx,
                });
                match reply_rx.await {
                    Ok(Ok(session_id)) => {
                        responder.respond(NewSessionResponse::new(SessionId::new(session_id)))
                    }
                    Ok(Err(message)) => {
                        responder.respond_with_error(Error::invalid_params().data(message))
                    }
                    Err(_) => {
                        responder.respond_with_internal_error("crabot closed the session request")
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // session/prompt — runs for the whole turn, so it must not block the
        // dispatch loop (otherwise `session/cancel` could never be processed).
        // Hand off to a spawned task that streams chunks back as notifications
        // and answers once the turn ends.
        .on_receive_request(
            async |req: PromptRequest, responder, cx| {
                let text: String = req
                    .prompt
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let (feed_tx, mut feed_rx) = mpsc::unbounded_channel();
                let session_id = req.session_id.to_string();
                cx.spawn({
                    let cx = cx.clone();
                    async move {
                        push_command(AcpCommand::Prompt {
                            session_id,
                            text,
                            feed: feed_tx,
                        });
                        let mut stop_reason = StopReason::EndTurn;
                        while let Some(event) = feed_rx.recv().await {
                            let update = match event {
                                AcpEvent::Text(chunk) => SessionUpdate::AgentMessageChunk(
                                    ContentChunk::new(chunk.into()),
                                ),
                                AcpEvent::Thought(chunk) => SessionUpdate::AgentThoughtChunk(
                                    ContentChunk::new(chunk.into()),
                                ),
                                AcpEvent::Error(message) => {
                                    let _ = responder
                                        .respond_with_error(Error::internal_error().data(message));
                                    return Ok(());
                                }
                                AcpEvent::Done { cancelled } => {
                                    if cancelled {
                                        stop_reason = StopReason::Cancelled;
                                    }
                                    break;
                                }
                            };
                            if cx
                                .send_notification(SessionNotification::new(
                                    req.session_id.clone(),
                                    update,
                                ))
                                .is_err()
                            {
                                // Client went away — cancel the turn so it
                                // doesn't keep burning tokens; the response is
                                // dropped with the connection.
                                push_command(AcpCommand::Cancel {
                                    session_id: req.session_id.to_string(),
                                });
                                break;
                            }
                        }
                        let _ = responder.respond(PromptResponse::new(stop_reason));
                        Ok(())
                    }
                })?;
                Ok(Handled::Yes)
            },
            agent_client_protocol::on_receive_request!(),
        )
        // session/set_mode — no session modes; acknowledge as a no-op.
        .on_receive_request(
            async |_req: SetSessionModeRequest, responder, _cx| {
                responder.respond(SetSessionModeResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        // session/cancel — stop the tab's stream; its terminal `Cancelled`
        // event resolves the pending prompt with StopReason::Cancelled.
        .on_receive_notification(
            async |notif: CancelNotification, _cx| {
                push_command(AcpCommand::Cancel {
                    session_id: notif.session_id.to_string(),
                });
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
}

// ── Server lifecycle ──────────────────────────────────────────────

/// Live ACP server state shown in the right pane.
#[derive(Debug, Default)]
pub(crate) struct AcpState {
    /// User toggle state (persisted in settings).
    pub(crate) enabled: bool,
    /// Whether the HTTP listener is up.
    pub(crate) running: bool,
    /// Last bound address, e.g. `127.0.0.1:8787`.
    pub(crate) addr: String,
    /// Bind failure message, if the server could not start.
    pub(crate) error: Option<String>,
    /// Cancels the HTTP server task on shutdown; the serve future is dropped
    /// so the port is released without waiting for open connections.
    shutdown: Option<CancellationToken>,
    /// Bumped on every start/stop; stale bind results from superseded cycles
    /// carry an older generation and are ignored.
    generation: u64,
}

impl AcpState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }
}

/// Messages routed to the ACP bridge.
#[derive(Clone)]
pub(crate) enum AcpMessage {
    Toggle(bool),
    /// At least one command is waiting in the queue (subscription tick).
    CommandTick,
    /// Bind result of the HTTP server; `generation` identifies the start/stop
    /// cycle that produced it.
    ServerStarted(u64, Result<String, String>),
}

pub(crate) fn update(app: &mut App, event: AcpMessage) -> Task<Message> {
    match event {
        AcpMessage::Toggle(enabled) => toggle(app, enabled),
        AcpMessage::CommandTick => drain_commands(app),
        AcpMessage::ServerStarted(generation, result) => {
            // Ignore bind results from a superseded start/stop cycle (e.g.
            // toggled off while the bind was in flight).
            if generation != app.acp.generation {
                return Task::none();
            }
            match result {
                Ok(addr) => {
                    app.acp.addr = addr;
                    app.acp.running = true;
                }
                Err(error) => {
                    app.acp.error = Some(error);
                    app.acp.shutdown = None;
                }
            }
            Task::none()
        }
    }
}

pub(crate) fn toggle(app: &mut App, enabled: bool) -> Task<Message> {
    if app.acp.enabled == enabled {
        return Task::none();
    }
    app.acp.enabled = enabled;
    app.settings.acp_server_enabled = enabled;
    // Persisted on exit via `App::save_settings`, like the theme toggle.
    if enabled {
        start(app)
    } else {
        stop(app);
        Task::none()
    }
}

/// Bind the loopback-only ACP HTTP server in the background. The address is
/// reported through [`AcpMessage::ServerStarted`].
pub(crate) fn start(app: &mut App) -> Task<Message> {
    app.acp.running = false;
    app.acp.error = None;
    app.acp.generation += 1;
    let generation = app.acp.generation;
    let port = app.settings.acp_server_port.max(1);
    let shutdown = CancellationToken::new();
    app.acp.shutdown = Some(shutdown.clone());
    Task::perform(
        async move {
            // Await the previous server task so a rapid off→on toggle never
            // binds while the old listener still holds the port. `stop` drops
            // the serve future immediately, so this completes promptly.
            let previous = lock(&SERVE_HANDLE).take();
            if let Some(previous) = previous {
                let _ = tokio::time::timeout(Duration::from_secs(3), previous).await;
            }
            let listener = match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => listener,
                Err(error) => return Err(format!("bind 127.0.0.1:{port}: {error}")),
            };
            let addr = listener
                .local_addr()
                .map(|addr| addr.to_string())
                .unwrap_or_else(|_| format!("127.0.0.1:{port}"));
            let router = AcpHttpServer::new(build_agent_connection).into_router();
            *lock(&SERVE_HANDLE) = Some(tokio::spawn(async move {
                let serve = axum::serve(listener, router).into_future();
                tokio::pin!(serve);
                tokio::select! {
                    result = &mut serve => {
                        if let Err(error) = result {
                            tracing::error!("acp http server error: {error}");
                        }
                    }
                    // Abrupt shutdown: dropping the serve future releases the
                    // listener without waiting for open SSE connections.
                    _ = shutdown.cancelled_owned() => {}
                }
            }));
            Ok(addr)
        },
        move |result| Message::Acp(AcpMessage::ServerStarted(generation, result)),
    )
}

/// Stop the HTTP server and release any prompt handlers still waiting on feeds.
pub(crate) fn stop(app: &mut App) {
    if let Some(token) = app.acp.shutdown.take() {
        token.cancel();
    }
    app.acp.running = false;
    app.acp.error = None;
    // Invalidate bind results still in flight from the stopped server.
    app.acp.generation += 1;
    let registry = std::mem::take(&mut *lock(&FEED_REGISTRY));
    resolve_feeds(
        registry.into_values().flatten(),
        &AcpEvent::Done { cancelled: true },
    );
}

// ── Command processing (UI thread) ────────────────────────────────

/// Pop and handle every queued command; batch any spawned tasks.
pub(crate) fn drain_commands(app: &mut App) -> Task<Message> {
    let mut tasks = Vec::new();
    while let Some(command) = lock(&COMMAND_QUEUE).pop_front() {
        if let Some(task) = handle_command(app, command) {
            tasks.push(task);
        }
    }
    Task::batch(tasks)
}

fn handle_command(app: &mut App, command: AcpCommand) -> Option<Task<Message>> {
    match command {
        AcpCommand::NewSession { cwd, reply } => new_session(app, cwd, reply),
        AcpCommand::Prompt {
            session_id,
            text,
            feed,
        } => prompt(app, session_id, text, feed),
        AcpCommand::Cancel { session_id } => {
            if let Some(tab) = app
                .conversation
                .session_tabs
                .iter_mut()
                .find(|tab| tab.session.id == session_id)
            {
                // Resolve waiting prompt feeds and flag the session so a
                // pending-prompt relaunch in the terminal-event race window
                // is suppressed.
                end_session(&session_id, true, None);
                mark_cancelled(&session_id);
                tab.session_state.stop();
            }
            None
        }
    }
}

/// Create a foreground session tab for the ACP client.
///
/// The client's `cwd` becomes the app workspace whenever it differs from the
/// current one (saving the outgoing workspace's AGENTS.md preference); a
/// non-directory `cwd` is rejected instead of breaking the workspace.
fn new_session(
    app: &mut App,
    cwd: PathBuf,
    reply: oneshot::Sender<Result<String, String>>,
) -> Option<Task<Message>> {
    let workspace = app.prompt.workspace.1.clone();
    let adopt_task = if cwd.as_os_str().is_empty() || same_dir(&cwd, &workspace) {
        None
    } else if !cwd.is_dir() {
        let _ = reply.send(Err(format!("cwd is not a directory: {}", cwd.display())));
        return None;
    } else {
        Some(crate::app::prompt::apply_workspace(app, cwd, true))
    };
    let (model, preamble) = {
        let viewing = app.conversation.viewing();
        (
            viewing.selected_model.clone(),
            viewing.selected_preamble.clone(),
        )
    };
    let new_tab_task = conversation::new_session(app, model, preamble, String::new());
    let session_id = app.conversation.viewing().session.id.clone();
    tracing::info!(session_id, "acp: new session tab");
    let _ = reply.send(Ok(session_id));
    Some(match adopt_task {
        Some(adopt_task) => adopt_task.chain(new_tab_task),
        None => new_tab_task,
    })
}

/// True when both paths point at the same directory, comparing canonicalized
/// forms so `..` segments and Windows casing differences compare equal.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || dunce::canonicalize(a)
            .ok()
            .zip(dunce::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
}

/// Register the prompt's feed on its session tab and launch or inject the
/// prompt. Validation failures go back through the feed so the waiting
/// handler can respond with a JSON-RPC error.
fn prompt(
    app: &mut App,
    session_id: String,
    text: String,
    feed: mpsc::UnboundedSender<AcpEvent>,
) -> Option<Task<Message>> {
    let Some(pos) = app
        .conversation
        .session_tabs
        .iter()
        .position(|tab| tab.session.id == session_id)
    else {
        let _ = feed.send(AcpEvent::Error(format!("session not found: {session_id}")));
        return None;
    };
    let model = match conversation::prompt_dispatch_guard(app, pos, false) {
        Ok(model) => model,
        Err(message) => {
            let _ = feed.send(AcpEvent::Error(message));
            return None;
        }
    };
    lock(&FEED_REGISTRY)
        .entry(session_id)
        .or_default()
        .push(feed);
    Some(conversation::dispatch_prompt(
        app,
        pos,
        &model,
        UserPrompt::new(None, text, None),
    ))
}

// ── Stream tap ────────────────────────────────────────────────────

/// Forward a session stream event to every ACP feed registered for the
/// session. Called from the stream callback in `conversation::start_dialog`;
/// never blocks (feeds are unbounded channels).
pub(crate) fn forward_event(session_id: &str, event: &SessionEvent) {
    let event = match event {
        SessionEvent::Content(chunk) => AcpEvent::Text(chunk.clone()),
        SessionEvent::Reasoning(chunk) => AcpEvent::Thought(chunk.clone()),
        SessionEvent::Done => AcpEvent::Done { cancelled: false },
        SessionEvent::Cancelled => AcpEvent::Done { cancelled: true },
        SessionEvent::Error(message) => AcpEvent::Error(message.clone()),
        _ => return,
    };
    let mut registry = lock(&FEED_REGISTRY);
    let Some(list) = registry.get_mut(session_id) else {
        return;
    };
    list.retain(|feed| feed.send(event.clone()).is_ok());
    // Terminal events end every prompt waiting on this session.
    if matches!(event, AcpEvent::Done { .. } | AcpEvent::Error(_)) {
        registry.remove(session_id);
    }
}

/// Resolve ACP feeds orphaned by the terminal-event race: the stream task
/// forwards terminal events and removes the registry entry *before* the UI
/// thread processes them, so a `session/prompt` drained in that window
/// registers a feed into a turn that will never run again. Called when a turn
/// ends cancelled/errored, when a relaunch is suppressed, on tab close, and on
/// `session/cancel`. `Done` turns are excluded because the pending-prompt path
/// relaunches the stream and adopts the feed.
pub(crate) fn end_session(session_id: &str, cancelled: bool, error: Option<String>) {
    let event = match error {
        Some(message) => AcpEvent::Error(message),
        None => AcpEvent::Done { cancelled },
    };
    let feeds = lock(&FEED_REGISTRY).remove(session_id);
    if let Some(feeds) = feeds {
        resolve_feeds(feeds, &event);
    }
}

/// Send a terminal event to every feed, best-effort.
fn resolve_feeds(
    feeds: impl IntoIterator<Item = mpsc::UnboundedSender<AcpEvent>>,
    event: &AcpEvent,
) {
    for feed in feeds {
        let _ = feed.send(event.clone());
    }
}

/// Flag a session whose turn an ACP client cancelled. [`take_cancelled`] is
/// consumed by `dispatch_pending` to suppress a late pending-prompt relaunch.
pub(crate) fn mark_cancelled(session_id: &str) {
    lock(&CANCELLED_SESSIONS).insert(session_id.to_string());
}

/// Remove and return the session's ACP cancel flag.
pub(crate) fn take_cancelled(session_id: &str) -> bool {
    lock(&CANCELLED_SESSIONS).remove(session_id)
}
