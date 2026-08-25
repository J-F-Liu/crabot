# Repository Guidelines

## Project Overview

Crabot is a pure-Rust native GUI coding agent using [iced](https://iced.rs) v0.14 (Elm architecture) and [genai](https://crates.io/crates/genai) `0.7.0-beta.18` for multi-provider LLM. Chat UI with 12 built-in tools — the `bash` tool runs on an in-process [bashkit](https://crates.io/crates/bashkit) interpreter (works natively on Windows, no bash required) — plus user-defined custom tools, MCP server tools, and multi-tab sessions.

---

## Architecture

### Three-pane Iced GUI

```
+----------------------------+---------------------------+-------------------------+
|| LEFT (~300px, scrollable) | CENTER (fills remaining)  | RIGHT (~280px)          |
|| Model config tabs         | Session tabs bar          | Theme toggle (top)      |
|| System prompt sections    | Session header + dialogs  | Context window stats    |
|| Session picker            | Search bar (Ctrl+F)       | Token usage & cost      |
|| Work mode tabs & toggle   | Turn bubbles (User/       | Last response time      |
|| User prompt textarea      |    Assistant/Tool)        | Todo list               |
|| Recipe dropdown           | Ask tool controls         | Modified files list     |
|| Tool list (Builtin/Custom | Status bar + stop button  |   (Revert / Revert All) |
||   MCP)                    |                           | Restart button (bottom) |
+----------------------------+---------------------------+-------------------------+
```

All panes live in `src/views/`. Left: `left_pane`, `model_config`, `user_prompt`, `session_list`, `tool_list`. Center: `center_pane`, `session_tabs`, `tool_message`, `search_bar`, `modal`. Right: `right_pane`. Shared: `theme`, `styles`, `icons`, `update`, `export`, `system_prompt`, `settings/` (7 tabs).

### Data Flow

- **UI → State:** `App::update` dispatches `Message` variants to domain handlers in `src/app/` (`layout`, `conversation`, `prompt`, `tool_state`, `settings`, `overlay`, `session_state`, `snapshot`).
- **LLM streaming:** `ConversationEvent::SendPrompt` → `llm::send_stream` agent loop (≤ `max_iterations`): request with system prompt + tools + history → stream chunks via callbacks → execute tool calls (parallel batches, serial barriers) → append results → loop. Cancellation via `tokio::select!`; transient errors auto-retried; stall detection.
- **Persistence:** RON config in `~/.crabot/` (`settings.ron`, `models.ron`, `tools.ron`, `mcp.ron`); sessions as JSONL in `.agent/sessions/YYYY-MM/{id}.jsonl` — first line = metadata, then one line per message, saved incrementally after every complete message (no whole-turn rewrites); a `Tally` line snapshots cumulative usage on terminal stream events; legacy `.json` still loadable and migrates on the next save.
- **Snapshots:** `write`/`edit` targets are pre-imaged before tool execution into `.agent/snapshots/{id}/`; the right-pane Revert / Revert All buttons restore them in the background.
- **MCP:** loads `~/.crabot/mcp.ron`, connects via `rmcp` 2 (stdio/HTTP), auto-discovers tools; connections held in `LazyLock<Mutex<HashMap<String, McpConnection>>>`, each owning a `RunningService` whose `DropGuard` cancels it.

### Agent Loop (`llm::send_stream`)

1. `PhaseChange(LlmLoading)`; move the single rolling cache breakpoint to the tail message (`mark_cache_tail`; system prompt keeps `Ephemeral1h`).
2. `stream_attempt` — race connect against cancellation; stream chunks (text + reasoning); a silent stream fails after `stream_stall_timeout_secs` (0 = off; Anthropic heartbeats keep long thinking alive).
3. Transient failures (429 / 5xx / connection) auto-retry up to 5 attempts with an on-screen countdown (`RetryCountdown`); other errors emit `SessionEvent::Error`.
4. Record each complete message immediately (`MessageReady`) — per-message persistence, so a crash mid-turn loses nothing; empty assistant messages (no text/reasoning/tool calls) are never persisted.
5. No tool calls → inject any user prompt queued during streaming (`injected_prompt`) → done.
6. Emit `ToolCalls`, then snapshot `write`/`edit` targets (`app/snapshot.rs`) before any tool runs.
7. Execute tools: independent calls run in **parallel batches**; serial tools (`ask`, `renew`, `write`, `edit`, `bash`, `process`) act as barriers. `ask` → mpsc with a shared extendable deadline; `renew` → `SessionEvent::RenewRequest` (new session tab; only the first `renew` is effective and it is moved last among parallel calls); `task` → `SessionEvent::TaskRequest` (background sub-agent tab, final report awaited as tool result); MCP → `block_in_place`; builtin/custom → `spawn_blocking`; `bash`/`process` stream live via `ToolOutput`.
8. Append results + injected prompts, loop — renew stops the current session after its turn.

### Module Map

| Path                                                      | Role                                                                                                                                                                                         |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/main.rs`                                             | Entry point; wires `iced::application` with `App::boot/update/view/subscription`, window icon, hides the console in release builds                                                           |
| `src/app.rs`                                              | Root `App` (8 state fields), hierarchical `Message` (12 variants), boot + view                                                                                                               |
| `src/app/{layout,overlay,subscription,attention}.rs`      | Window geometry/focus, banners & dialogs, input subscriptions, taskbar attention                                                                                                             |
| `src/app/conversation.rs`                                 | Session tab lifecycle, send/resend, stream orchestration, search, ask UI                                                                                                                     |
| `src/app/session_tab.rs` + `session_state.rs`             | Per-tab state: streaming lifecycle, placeholders, auto-scroll, token totals, `SessionEvent` handling                                                                                         |
| `src/app/snapshot.rs`                                     | Per-file raw pre-images backing right-pane Revert / Revert All                                                                                                                               |
| `src/app/prompt.rs` + `tool_state.rs`                     | System-prompt composition (preamble/rules/workspace/AGENTS.md), tool enablement                                                                                                              |
| `src/lib.rs`                                              | `HashSetExt::set()`, `lock()`, `BoundedCapture` (head/tail window), truncation marker                                                                                                        |
| `src/settings.rs`                                         | Persistable RON state; prompt recipes, tool limits, task models, fill-ratio threshold                                                                                                        |
| `src/model.rs` + `model_database.rs`                      | `ModelList`/`ModelConfig`/`Provider`/`TaskModels` types; ~500-model read-only embedded DB                                                                                                    |
| `src/chat.rs` + `session.rs`                              | `Turn`/`Dialog` UI types; raw history + derived dialogs, JSONL persistence + tally, todo extraction                                                                                          |
| `src/llm/`                                                | Streaming engine: `mod.rs` (agent loop + `SendConfig`), `stream.rs` (acquisition, retry/stall, cache), `tool_call.rs` (parallel/serial tool calls + ask), `client.rs` (genai client)         |
| `src/setup.rs` + `workspace.rs` + `fonts.rs` + `user.rs`  | First-boot seeding + logging; gitignore-aware tree scan; CJK font detection; `WorkMode`                                                                                                      |
| `src/tools/mod.rs`                                        | Re-export hub — keeps the legacy `crate::tools::<item>` paths for all submodules                                                                                                             |
| `src/tools/tool.rs`                                       | `Tool` trait (incl. streaming variants), `OutputSink`, shared constants                                                                                                                      |
| `src/tools/registry.rs`                                   | `ToolRegistry`, genai declaration builder, unknown-tool suggestions                                                                                                                          |
| `src/tools/builtin/`                                      | 12 built-in tools, one file per tool (`super::` resolves via the re-exports)                                                                                                                 |
| `src/tools/{exec,paths,limits,capture,charset,schema}.rs` | Process plumbing; path/text helpers; limits/truncation; output forwarding; charset decoding; strict schemas                                                                                  |
| `src/tools/bash_kit.rs`                                   | In-process bashkit interpreter + host-command bridge for the `bash` tool                                                                                                                     |
| `src/tools/custom.rs` + `mcp.rs`                          | Custom tool loader (TinyTemplate, typed params, pipes); MCP client                                                                                                                           |
| `src/views/`                                              | UI pane modules; `settings/` with 7 tabs (ai_models, prompt_recipes, builtin_tools, custom_tools, mcp_servers, tool_playground, about); `update.rs` (self-update), `export.rs` (HTML export) |
| `src/widgets/`                                            | Custom `TextArea` (undo/redo), `DropDown`, `PopupMenu`                                                                                                                                       |
| `assets/`                                                 | Bundled preambles, `workmode.md`, `rules/`, models, tools, mcp, images                                                                                                                       |
| `~/.crabot/`                                              | User config: `settings.ron`, `models.ron`, `tools.ron`, `mcp.ron`, `preamble/`, `rules/`, `logs/`                                                                                            |
| `.agent/sessions/`                                        | Session JSONL, grouped in `YYYY-MM/` subdirs; legacy `.json` migrates on save                                                                                                                |
| `.agent/snapshots/`                                       | Per-file raw pre-images (`.agent/snapshots/{id}/`) backing the right-pane Revert actions                                                                                                     |
| `tests/`                                                  | Integration tests: `bash`, `process`, `tools`, `session`, `chat`, `bounded_capture`, `edit`, `find`, `fetch`                                                                                 |

---

## Tool System

### `Tool` trait (`src/tools/tool.rs`)

```rust
pub type ToolRef = Arc<dyn Tool>;
pub type OutputSink = Arc<dyn Fn(&str) + Send + Sync>; // incremental output chunks

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn instruction(&self) -> &str;
    fn schema(&self) -> Value;

    /// Cancel-aware wrapper: checks the token *before* delegating to `execute_inner`.
    fn execute(&self, args: &Value, workspace: &Path, cancel: &CancellationToken)
        -> Result<String, String>;
    fn execute_inner(&self, args: &Value, workspace: &Path, cancel: &CancellationToken)
        -> Result<String, String>;

    /// Streaming variant — live-output tools forward incremental chunks to `sink`
    /// (default falls back to `execute_inner`; chunks are raw text, newline-normalized).
    fn execute_streaming(&self, args: &Value, workspace: &Path, cancel: &CancellationToken,
        sink: &OutputSink) -> Result<String, String>;
    fn execute_streaming_inner(&self, args: &Value, workspace: &Path, cancel: &CancellationToken,
        sink: &OutputSink) -> Result<String, String>;

    /// Full tool declaration suitable for genai ChatRequest (strict mode via `make_strict_schema`).
    fn tool_declaration(&self, strict: bool) -> GenaiTool { ... }
}
```

Shared constants: `CANCEL_REASON` ("Cancelled by user"), `COALESCE_BYTES` (4 KB) / `COALESCE_MS` (100 ms) chunk coalescing.

`ToolRegistry` (`src/tools/registry.rs`) owns `builtin: Vec<ToolRef>`, `custom: Vec<CustomTool>`, and MCP tools grouped by server (`mcp: Vec<(String, Vec<McpTool>)>`). Key methods: `register_custom()`, `register_mcp_group()`, `unregister_mcp_group()`, `find_mcp_server()`, `custom_names()`, `all_names()`, `enabled_tools(enabled, enabled_servers)`, `mcp_server_has_enabled_tool()`, `find_tool()`, `snapshot_todo()`, `clear_todo()`; free helpers `build_tools()` (genai declarations) and `unknown_tool_message()` (maps `grep`→`search`, `cat`→`read`, `ls`→`find`, `curl`→`fetch`, etc.).

### Tool Infrastructure (`src/tools/`)

| File          | Role                                                                                                                                                    |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tool.rs`     | `Tool` trait, `OutputSink`, `ToolRef`, shared constants                                                                                                 |
| `registry.rs` | `ToolRegistry` + genai declaration builder + unknown-tool hints                                                                                         |
| `schema.rs`   | `make_strict_schema` (every property required, optional → nullable unions), `decode_stringified_args` (restore strict-coerced JSON)                     |
| `paths.rs`    | `arg_path`/`arg_str`/`arg_u64`, `resolve_path[_partial]`, `convert_path_to_unix_style` (MSYS `/c/...`), `normalize_newlines`, `make_workspace_relative` |
| `exec.rs`     | Pipe plumbing, bounded stdin, process-tree signals, env sanitization, exit/output formatting                                                            |
| `capture.rs`  | `ChunkForwarder` (live streaming), `wait_with_timeout` (polling + drain + kill), `WaitError` classification                                             |
| `limits.rs`   | `ToolLimits` (configurable), `truncate_output` (head+tail), `StreamingCap` + `capping_sink` (live cap), truncation markers                              |
| `charset.rs`  | `decode_bytes` / `StreamDecoder` — BOM, UTF-8 passthrough, `chardetng`+`encoding_rs` fallback (GBK, Shift_JIS, windows-1252…)                           |
| `bash_kit.rs` | In-process bashkit interpreter: static command analysis, host-command bridge, embedded Python, VFS sandbox                                              |

### Tool Categories

| Category     | Source                                                                                      | Description                                                                                 |
| ------------ | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Builtin (12) | `src/tools/builtin/{read,write,edit,find,search,bash,process,ask,todo,task,renew,fetch}.rs` | File I/O, shell, process lifecycle, interaction, web, session handoff, sub-agent delegation |
| Custom       | `~/.crabot/tools.ron`                                                                       | User-defined CLI tools (TinyTemplate command + JSON Schema params)                          |
| MCP          | `~/.crabot/mcp.ron` → rmcp 2                                                                | Remote tools from stdio/HTTP servers                                                        |

### Built-in Tools

| Tool      | Description                                                                                                                                                                                                              |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `read`    | File read with offset/limit pagination; 2000-line / 64 KB budgets (configurable), truncation marker                                                                                                                      |
| `write`   | File write with parent dir creation                                                                                                                                                                                      |
| `edit`    | Exact-string replacement via byte-range offsets, overlap detection; collects all argument errors before reporting                                                                                                        |
| `find`    | Glob finder, gitignore-aware, 100-line cap; smart-case (uppercase in pattern switches to exact case)                                                                                                                     |
| `search`  | Regex search via `grep-regex`/`grep-searcher` (streaming scan, binary detection, lossy UTF-8), 500-line cap                                                                                                              |
| `bash`    | In-process bashkit interpreter (host commands bridged, embedded Python, VFS sandbox, open `curl`/`wget`); falls back to real `bash -c`; default 120 s timeout (clamped ≥ 1000 ms, max configurable); streams live output |
| `process` | Long-running process lifecycle (`start`/`list`/`status`/`logs`/`input`/`wait`/`stop`/`restart`) addressed by OS `pid`, app-global registry                                                                               |
| `ask`     | Interactive prompt — intercepted by engine, routed to UI via mpsc; live countdown with "Extend +5 min", "Enter my answer", "You decide"                                                                                  |
| `todo`    | Shared todo list (registry-held, rendered in right pane)                                                                                                                                                                 |
| `task`    | Delegate subtask to a new background session tab; final report arrives as the tool result                                                                                                                                |
| `renew`   | New session tab seeded with condensed summary — intercepted when context is nearly full; only the first call is effective                                                                                                |
| `fetch`   | HTTP fetch with Markdown extraction via `dom_smoothie`; 8 MB body / 60 s limits (configurable)                                                                                                                           |

### Custom Tools

Defined in `~/.crabot/tools.ron`: `command` (TinyTemplate), typed `parameters` (String/Integer/Number/Boolean/Array/Object/Union), `instruction`. Spawned via `interprocess` pipes with bounded capture — no reader threads.

### MCP Tools

Configured in `~/.crabot/mcp.ron`: transport (`Stdio("cmd args")` with optional `env_vars`, or `Http("url")` with headers), `qualify_tool_names`, optional `prompt`. Auto-connect at startup into a `LazyLock<Mutex<HashMap<String, McpConnection>>>`; each connection's `DropGuard` cancels its `RunningService`. Tool calls run via `block_in_place` with `mcp_call_timeout_ms`; strict-mode stringified arguments are decoded back to JSON before forwarding.

### Process Helpers (`src/tools/exec.rs` + `capture.rs`)

- `exec.rs`: `create_pipe_pair()` (interprocess, no reader threads); `write_stdin_bounded()` (16 KB chunks, `WouldBlock` retries, bounded by cancel + deadline); `signal_process_tree()` (Unix `kill(-pgid)` — avoids broken `kill` binaries; Windows `taskkill`-style, `interrupt` degrades to terminate); `detach_child()`; `sanitize_child_env()`/`is_secret_env_key()` (drops API-key-like vars); `pipe_to_stdio()`; `set_pipe_nonblocking()`; `exit_code_of()`; `format_command_output()`.
- `capture.rs`: `ChunkForwarder` — stdout/stderr merged in arrival order, `\r\n` → `\n`, 4 KB / 100 ms coalescing, per-stream `BoundedCapture` windows for partial-output errors; `wait_with_timeout()` — non-blocking polling drains, kills + reaps the process tree on timeout/cancel, classifies failures as `WaitError::{Timeout, Cancelled, Other}` mapped to bash exit codes 124 / 130 / 1, keeps partial output in error messages, and drains up to 2 s after exit so daemonised grandchildren can't leak threads; `try_lock_for()` for contended forwarder locks.

---

## Conventions & Patterns

### Errors & Async
- `Result<_, Box<dyn Error>>` / `Result<_, String>`; tools return `Result<String, String>`; `Settings::load()` → `Self` (defaults on missing/malformed file); no `thiserror`/`anyhow`
- Tracing via `tracing` + `tracing-subscriber`: daily-rolling logs in `~/.crabot/logs` (filter with `RUST_LOG`), panic hook mirrors to `panic.log`, debug builds mirror to stderr
- Tokio (Iced integration); `iced::stream::channel` for streaming (`SessionEvent` → `ConversationEvent::SessionEvent`)
- Tool execution: parallel batches via `spawn_blocking` with serial barriers (`ask`/`renew`/`write`/`edit`/`bash`/`process`); MCP → `block_in_place`; ask → mpsc; cancel via `CancellationToken` (`biased` `tokio::select!` on `cancelled()`, sync loops poll `is_cancelled()`; fresh token per stream); pending prompt via `Arc<Mutex<Option<String>>>`

### State & UI
- `App` owns 8 state groups (`models`, `settings`, `layout`, `prompt`, `tools`, `conversation`, `settings_dialog`, `overlay`); hierarchical `Message` (12 variants incl. `RestartApp`, revert outcomes); `FocusedTarget` is exclusive
- `ConversationState` owns `Vec<SessionTab>` (per-tab session, streaming, search, todo, scroll, model)
- Dual session data: `Session.history` (raw `Vec<ChatMessage>`) + `Session.dialogs` (UI); `rebuild_dialogs()` syncs
- Per-message persistence: `MessageReady` appends to the JSONL incrementally; `Tally` line written only on terminal stream events
- Placeholder streaming: empty `Turn::assistant("")` pushed on `LlmThinking`, chunks appended, `handle_stream_done()` finalizes
- Tool output streaming: `Tool::execute_streaming`; `bash`/`process` stream via `SessionEvent::ToolOutput` → placeholder `ToolResult { streaming: true }` replaced in place on finish (replace-by-`call_id` in `Dialog::push_tool_result`); chunks coalesced to ≤ ~10 UI updates/s (4 KB / 100 ms) and capped at `max_output_bytes` via `capping_sink`
- File snapshots: `write`/`edit` targets pre-imaged in `.agent/snapshots/{id}/`; right-pane Revert / Revert All restore in the background
- Work modes Plan/Code/Review parsed from `workmode.md`; togglable, per-mode recipe templates
- Custom widgets: `TextArea` (undo/redo, 100-deep, edit coalescing); `DropDown`; `PopupMenu`
- Emoji rendering with code-region awareness; JSON-safe tool output; CJK fonts via `fontdb`; charset-aware decoding of shell output; RFD file dialogs
- Cache: Anthropic rolling ephemeral breakpoint at conversation tail; system prompt `Ephemeral1h` TTL
- Task delegation: child tab lineage via `task_path`; `mode` selects the per-mode preamble (`explore`/`planning`/`coding`/`review`/`testing`); `difficulty` picks the subtask model (`models.task_models`, empty = inherit); child `Done`/`Error`/`Cancelled` reports via parent's `task_sender` mpsc
- Taskbar attention for background sessions (`app/attention.rs`); GitHub-release update check with one-click self-update and download progress; script-free HTML session export (strict CSP)

### Assets & Config
- Bundled via `include_dir!`, seeded to `~/.crabot/` on first boot; API keys from env vars only
- `AGENTS.md` in workspace: auto-detected, injectable into system prompt

---

## Runtime Preferences

| Requirement    | Value                                                                            |
| -------------- | -------------------------------------------------------------------------------- |
| Rust toolchain | Edition 2024, stable                                                             |
| Build          | Cargo                                                                            |
| Deps           | `cargo add`                                                                      |
| Format         | `cargo fmt`                                                                      |
| Lint           | `cargo clippy`                                                                   |
| Docs           | `cargo doc --no-deps --document-private-items` (no `--open`)                     |
| OS             | Linux, macOS, Windows (CREATE_NO_WINDOW)                                         |
| Env vars       | API keys via environment (e.g. `DEEPSEEK_API_KEY`); `RUST_LOG` for log filtering |

### CI
- `rust.yml`: push/PR → `cargo build --release` + `cargo test --verbose` on ubuntu-latest
- `release.yml`: `v*` tag → GitHub Release

### .gitignore
`/target`, `/tmp`, `/.agent`, `/.reasonix`, `/.codebase-memory`, `/.codegraph`, `justfile`, `reasonix.toml`, `nul`.
