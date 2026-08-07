# Repository Guidelines

## Project Overview

Crabot is a pure-Rust native GUI coding agent using [iced](https://iced.rs) v0.14 (Elm architecture) and [genai](https://crates.io/crates/genai) v0.7.0-beta.15 for multi-provider LLM. Chat UI with 11 built-in tools, user-defined custom tools, MCP server tools, and multi-tab sessions.

---

## Architecture

### Three-pane Iced GUI

```
+----------------------------+---------------------------+-------------------------+
| LEFT (~280px, scrollable)  | CENTER (fills remaining)  | RIGHT (~260px)          |
| Model config tabs          | Session tabs bar          | Theme toggle (top)      |
| System prompt sections     | Session header + dialogs  | Context window stats    |
| Session picker             | Search bar (Ctrl+F)       | Token usage & cost      |
| Work mode tabs & toggle    | Turn bubbles (User/       | Todo list               |
| User prompt textarea       |    Assistant/Tool)        | Modified files list     |
| Recipe dropdown            | Ask tool controls         | Restart button (bottom) |
| Tool list (Builtin/Custom/ | Status bar + stop button  |                         |
|   MCP)                     |                           |                         |
+----------------------------+---------------------------+-------------------------+
```

All panes live in `src/views/`. Left: `left_pane`, `model_config`, `user_prompt`, `session_list`, `tool_list`. Center: `center_pane`, `session_tabs`, `tool_message`, `search_bar`, `modal`. Right: `right_pane`. Shared: `theme`, `styles`, `icons`, `update`, `system_prompt`, `settings/` (7 tabs).

### Data Flow

- **UI → State:** `App::update` dispatches `Message` variants to domain handlers in `src/app/` (`layout`, `conversation`, `prompt`, `tool_state`, `settings`, `overlay`, `session_state`).
- **LLM streaming:** `ConversationEvent::SendPrompt` → `llm::send_stream` agent loop (≤ `max_iterations`): request with system prompt + tools + history → stream chunks via callbacks → execute tool calls → append results → loop. Cancellation via `tokio::select!`.
- **Persistence:** RON config in `~/.crabot/` (`settings.ron`, `models.ron`, `tools.ron`, `mcp.ron`); sessions as JSONL in `.agent/sessions/YYYY-MM/{id}.jsonl` (first line = metadata, then incremental messages + cumulative usage tally; legacy `.json` still loadable); per-file raw snapshots (Revert) in `.agent/snapshots/{id}/`.
- **MCP:** loads `~/.crabot/mcp.ron`, connects via `rmcp` (stdio/HTTP), auto-discovers tools; connections held in `LazyLock<Mutex<HashMap<String, McpConnection>>>`.

### Agent Loop (`llm::send_stream`)

1. Set rolling cache breakpoint on tail message
2. Send request; race connect against cancellation
3. Stream chunks (text + reasoning)
4. No tool calls → check injected user prompt → done
5. Signal `ToolExecuting` phase, yield for UI
6. Execute tools — ask → mpsc; renew → `SessionEvent::RenewRequest` (new session tab); task → `SessionEvent::TaskRequest` (sub-agent tab, final report awaited as tool result); MCP → `block_in_place`; builtin/custom → `spawn_blocking`
7. Append results + injected prompts, loop — renew stops the current session after its turn

### Module Map

| Path                                                                        | Role                                                                                                                                   |
| --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `src/main.rs`                                                               | Entry point; wires `iced::application` with `App::boot/update/view/subscription`                                                       |
| `src/app.rs`                                                                | Root `App` (6 domain state groups), hierarchical `Message` (7 variants), boot + view                                                   |
| `src/app/{layout,overlay,subscription,settings}.rs`                         | Window geometry/focus, banners & dialogs, input subscriptions, settings dialog                                                         |
| `src/app/conversation.rs`                                                   | Session tab lifecycle, send/resend, stream orchestration, search, ask UI                                                               |
| `src/app/session_tab.rs` + `session_state.rs`                               | Per-tab state: streaming lifecycle, placeholders, auto-scroll, token totals                                                            |
| `src/app/prompt.rs` + `tool_state.rs`                                       | System-prompt composition (preamble/rules/workspace/AGENTS.md), tool enablement                                                        |
| `src/lib.rs`                                                                | `HashSetExt::set()` toggle helper                                                                                                      |
| `src/settings.rs`                                                           | Persistable RON state; `prompt_recipes` per work mode                                                                                  |
| `src/model.rs` + `model_database.rs`                                        | `ModelList`/`ModelConfig`/`Provider` types; ~500-model read-only embedded DB                                                           |
| `src/chat.rs` + `session.rs`                                                | `Turn`/`Dialog` UI types; raw history + derived dialogs, usage, todo extraction                                                        |
| `src/llm.rs`                                                                | Streaming engine, agent loop, `DialogPhase`, cache management                                                                          |
| `src/setup.rs` + `workspace.rs` + `fonts.rs` + `user.rs`                    | First-boot seeding; gitignore-aware tree scan; CJK font detection; `WorkMode`                                                          |
| `src/tools/mod.rs`                                                          | `Tool` trait, `ToolRegistry`, strict schema, process helpers, cancel support                                                           |
| `src/tools/{read,write,edit,find,search,bash,ask,todo,task,renew,fetch}.rs` | 11 built-in tools                                                                                                                      |
| `src/tools/custom.rs` + `mcp.rs`                                            | Custom tool loader (TinyTemplate, typed params, pipes); MCP client                                                                     |
| `src/views/`                                                                | UI pane modules; `settings/` with 7 tabs (ai_models, prompt_recipes, builtin_tools, custom_tools, mcp_servers, tool_playground, about) |
| `src/widgets/`                                                              | Custom `TextArea` (undo/redo), `DropDown`, `PopupMenu`                                                                                 |
| `assets/`                                                                   | Bundled preambles, `workmode.md`, `rules/`, models, tools, mcp, images                                                                 |
| `~/.crabot/`                                                                | User config: `settings.ron`, `models.ron`, `tools.ron`, `mcp.ron`, `preamble/`, `rules/`                                               |
| `.agent/sessions/`                                                          | Session JSONL, grouped in `YYYY-MM/` subdirs; legacy `.json` still loadable                                                           |
| `.agent/snapshots/`                                                         | Per-file raw pre-images (`.agent/snapshots/{id}/`) backing the right-pane Revert actions                                               |

---

## Tool System

### `Tool` trait (`src/tools/mod.rs`)

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn instruction(&self) -> &str;
    fn schema(&self) -> Value;
    fn execute(&self, args: &Value, workspace: &Path, cancel: &AtomicBool) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) { return Err("Cancelled by user".into()); }
        self.execute_inner(args, workspace, cancel)
    }
    fn execute_inner(&self, args: &Value, workspace: &Path, cancel: &AtomicBool) -> Result<String, String>;
    fn tool_declaration(&self, strict: bool) -> GenaiTool { ... }
}
```

`ToolRegistry` owns builtin + MCP tools plus `Vec<CustomTool>`; key methods: `custom_names()`, `all_names()`, `enabled_tools()`, `get_mcp_tool_names()`, `find_tool()`, `snapshot_todo()`, `clear_todo()`.

### Tool Categories

| Category     | Source                                                                      | Description                                                              |
| ------------ | --------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Builtin (11) | `src/tools/{read,write,edit,find,search,bash,ask,todo,task,renew,fetch}.rs` | File I/O, shell, interaction, web, session handoff, sub-agent delegation |
| Custom       | `~/.crabot/tools.ron`                                                       | User-defined CLI tools (TinyTemplate command + JSON Schema params)       |
| MCP          | `~/.crabot/mcp.ron` → rmcp                                                  | Remote tools from stdio/HTTP servers                                     |

### Built-in Tools

| Tool     | Description                                                                                          |
| -------- | ---------------------------------------------------------------------------------------------------- |
| `read`   | File read with offset/limit, 64KB cap, smart truncation                                              |
| `write`  | File write with parent dir creation                                                                  |
| `edit`   | Exact-string replacement via byte-range offsets, overlap detection                                   |
| `find`   | Glob finder, gitignore-aware, 100-line cap                                                           |
| `search` | Regex search across files, gitignore-aware                                                           |
| `bash`   | Shell with timeout (default 120s), process-group kill                                                |
| `ask`    | Interactive prompt — intercepted by engine, routed to UI via mpsc                                    |
| `todo`   | Shared todo list (written by tool, shown in right pane)                                              |
| `task`   | Delegate subtask to a new isolated session tab; blocks until final report arrives as the tool result |
| `renew`  | New session tab seeded with condensed summary — intercepted when context is nearly full              |
| `fetch`  | HTTP fetch with Markdown extraction via `dom_smoothie`                                               |

### Custom Tools

Defined in `~/.crabot/tools.ron`: `command` (TinyTemplate), typed `parameters` (String/Integer/Number/Boolean/Array/Object/Union), `instruction`. Spawn via `interprocess` pipes — no reader threads.

### MCP Tools

Configured in `~/.crabot/mcp.ron`: transport (`Stdio("cmd args")` or `Http("url")`), `qualify_tool_names`, optional `env`/`prompt`. Auto-connect at startup; `DropGuard` cleanup.

### Process Helpers (`src/tools/mod.rs`)

Pipe I/O via `interprocess` (no reader threads); `wait_with_timeout()` (polls + drains + kills on timeout, optional `ChunkForwarder` for live output streaming); `ChunkForwarder` (stdout+stderr merged in arrival order, UTF-8 carry, `\r\n` → `\n`, `max_output_bytes` cap, size/time coalescing); `kill_process_tree()` (Unix pgid / Windows `taskkill /F /T`); `truncate_output()` (100KB cap, head+tail); `format_command_output()`; `resolve_path[_partial]()` (Unix-style `/c/...` and workspace-relative).

---

## Conventions & Patterns

### Errors & Async
- `Result<_, Box<dyn Error>>` / `Result<_, String>`; tools return `Result<String, String>`; `Settings::load()` → `Option`; no `thiserror`/`anyhow`
- Tokio (Iced integration); `iced::stream::channel` for streaming (`SessionEvent` → `ConversationEvent::SessionEvent`)
- Tools: builtin/custom → `spawn_blocking`; MCP → `block_in_place`; ask → mpsc; cancel via `AtomicBool` + `tokio::select!`; pending prompt via `Arc<Mutex<Option<String>>>`

### State & UI
- 6 domain state groups in `App`; hierarchical `Message` (7 variants incl. `RestartApp`); `FocusedTarget` is exclusive
- `ConversationState` owns `Vec<SessionTab>` (per-tab session, streaming, search, todo, scroll, model)
- Dual session data: `Session.history` (raw `Vec<ChatMessage>`) + `Session.dialogs` (UI); `rebuild_dialogs()` syncs
- Placeholder streaming: empty `Turn::assistant("")` pushed on `LlmThinking`, chunks appended, `handle_stream_done()` finalizes
- Tool output streaming: `Tool::execute_streaming` (default delegates to `execute`); bash streams live via `SessionEvent::ToolOutput` → placeholder `ToolResult { streaming: true }` replaced in place on finish (replace-by-`call_id` in `Dialog::push_tool_result`); bashkit host-command route streams from pipe drains, builtin-only scripts via `exec_streaming` callback (skipped when external names exist — would duplicate)
- Work modes Plan/Code/Review parsed from `workmode.md`; togglable, per-mode recipe templates
- Custom widgets: `TextArea` (undo/redo, 100-deep, edit coalescing); `DropDown`; `PopupMenu`
- Emoji rendering with code-region awareness; JSON-safe tool output; CJK fonts via `fontdb`; RFD file dialogs
- Cache: Anthropic rolling ephemeral breakpoint at conversation tail; system prompt `Ephemeral1h` TTL
- Task delegation: child tab lineage via `task_path`; `mode` selects `~/.crabot/preamble/{mode}.md`; `difficulty` picks subtask model (`models.task_models`, empty = inherit); child `Done`/`Error`/`Cancelled` reports via parent's `task_sender` mpsc

### Assets & Config
- Bundled via `include_dir!`, seeded to `~/.crabot/` on first boot; API keys from env vars only
- `AGENTS.md` in workspace: auto-detected, injectable into system prompt

---

## Runtime Preferences

| Requirement    | Value                                                                 |
| -------------- | --------------------------------------------------------------------- |
| Rust toolchain | Edition 2024, stable                                                  |
| Build          | Cargo                                                                 |
| Deps           | `cargo add`                                                           |
| Format         | `cargo fmt`                                                           |
| Lint           | `cargo clippy`                                                        |
| Docs           | `cargo doc --no-deps --document-private-items` (no `--open`)          |
| OS             | Linux, macOS, Windows (CREATE_NO_WINDOW)                              |
| Env vars       | API keys via environment (`DEEPSEEK_API_KEY`, `OPENAI_API_KEY`, etc.) |

### CI
- `rust.yml`: push/PR → `cargo build --release` + `cargo test --verbose` on ubuntu-latest
- `release.yml`: `v*` tag → GitHub Release

### .gitignore
`/target`, `/tmp`, `/.agent`, `/.reasonix`, `/.codebase-memory`, `/.codegraph`, `/tests`, `justfile`, `reasonix.toml`, `nul`
