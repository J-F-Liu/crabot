# Repository Guidelines

## Project Overview

Crabot is a pure-Rust native GUI coding agent using [iced](https://iced.rs) v0.14 (Elm architecture) and [genai](https://crates.io/crates/genai) v0.7.0-beta.13 for multi-provider LLM. Chat interface with 9 built-in tools (read, write, edit, find, search, bash, ask, todo, fetch), user-defined custom tools, and MCP server tools.

---

## Architecture

### Three-pane Iced GUI

```
+----------------------------+---------------------------+-------------------------+
| LEFT (~280px, scrollable)  | CENTER (fills remaining)  | RIGHT (~260px)          |
| Model config tabs          | Session header + dialogs  | Context window stats    |
| System prompt sections     | Turn bubbles (User/       | Cumulative token usage  |
| Session picker             |   Assistant/Tool)         | Session cost            |
| User prompt textarea       | Search bar (Ctrl+F)       | Modified files list     |
| Work mode tabs & toggle    | Ask tool controls         | Todo list               |
| Recipe dropdown            | Status bar + stop button  | Restart button          |
| Tool enable checkboxes     |                           |                         |
+----------------------------+---------------------------+-------------------------+
```

All panes live in `src/views/` as separate modules. Left pane modules: `left_pane`, `model_config`, `user_prompt`, `session_list`. Center: `center_pane`, `tool_message`, `search_bar`, `modal`. Right: `right_pane`. Shared: `theme`, `styles`, `icons`, `update`, `system_prompt`, `settings/` (5 tabs).

### Data Flow

**UI → State:** `App::update` dispatches `Message` variants to domain-specific update functions (`layout::update`, `conversation::update`, `prompt::update`, `tool_state::update`, `settings::update`, `overlay::update`) in `src/app/`, each mutating their state group.

**LLM streaming:** `ConversationEvent::SendPrompt` → `llm::send_stream` (agent loop up to 100 iterations): send request with system prompt + tools + history → stream response chunks via callbacks → execute tool calls → append results → loop. Uses `tokio::select!` for cancellation races.

**Persistence:** RON → `~/.crabot/settings.ron`, `~/.crabot/models.ron`, `~/.crabot/mcp.ron`. Sessions → JSON in `.agent/sessions/id.json`.

**MCP:** Loads `~/.crabot/mcp.ron`, connects via `rmcp` (stdio/HTTP), auto-discovers tools, holds connections in `LazyLock<Mutex<HashMap<String, McpConnection>>>`.

### Agent Loop (`llm::send_stream`)

1. Set rolling cache breakpoint on tail message
2. Send request with system prompt + tools + history
3. Race connect against cancellation
4. Stream response chunks (text + reasoning) via callbacks
5. If no tool calls → check injected user prompt → done
6. Signal `ToolExecuting` phase, yield for UI
7. Execute tools (ask uses mpsc; MCP uses `block_in_place`; builtin/custom use `spawn_blocking`)
8. Append results + injected prompts to history, loop

### Module Map

| Path | Role |
|------|------|
| `src/main.rs` | Entry point, wires `iced::application` with `App::boot`/`update`/`view`/`subscription` |
| `src/app.rs` | Root `App` struct (6 domain state groups), hierarchical `Message` enum (6 variants), boot + view |
| `src/app/layout.rs` | Window geometry, cursor, dividers, zoom, keyboard modifiers, scrolling |
| `src/app/conversation.rs` | Session lifecycle, send/resend, stream orchestration, search, ask tool UI |
| `src/app/prompt.rs` | System-prompt composition (7 named components), workspace switching, work-mode, recipe dropdown |
| `src/app/tool_state.rs` | Tool/MCP enable/disable, discovery results, tools summary refresh |
| `src/app/settings.rs` | Settings dialog lifecycle, save/apply, playground execution |
| `src/app/overlay.rs` | Update banner, external links, empty-workspace confirmation |
| `src/app/subscription.rs` | Mouse, keyboard, window close → domain `Message`s |
| `src/app/session_state.rs` | Streaming lifecycle, placeholder management, auto-scroll, token accumulation |
| `src/lib.rs` | `HashSetExt::set()` trait for ergonomic toggle |
| `src/settings.rs` | Persistable state, RON serialization; `prompt_recipes` for per-work-mode templates |
| `src/model.rs` | `ModelList`, `ModelConfig`, `Provider`, `Model`, `Cost`, `TokenAmount` |
| `src/model_database.rs` | ~500 models from embedded 1.8 MB JSON; lazy `OnceLock` cache with pricing, context windows, aliases |
| `src/chat.rs` | `Turn`, `TurnBody`, `Dialog` — conversation types with Markdown caching, emoji replacement |
| `src/session.rs` | Raw `ChatMessage` history + derived UI dialogs + usage/cost + modified files + todo extraction |
| `src/llm.rs` | Streaming engine, agent loop, `DialogPhase` (Idle→LlmLoading→LlmThinking→ToolExecuting), cache mgmt |
| `src/setup.rs` | Seeds `~/.crabot/` with bundled assets on first boot |
| `src/workspace.rs` | Tree scanner respecting `.gitignore`/`.ignore`/hidden files, mtime-sorted layout |
| `src/user.rs` | `UserPrompt` wraps text in `<work-mode>` tags; `WorkMode` parsed from `workmode.md` |
| `src/fonts.rs` | System fonts + CJK auto-detection via `fontdb` + bundled monospace |
| `src/tools/mod.rs` | `Tool` trait, `ToolRegistry`, strict schema, process helpers, cancel support |
| `src/tools/{read,write,edit,find,search,bash,ask,todo,fetch}.rs` | 9 built-in tools |
| `src/tools/custom.rs` | Custom tool loader with TinyTemplate commands, typed params, pipe-based I/O |
| `src/tools/mcp.rs` | MCP client — server connection, tool discovery, `McpTool` wrapper |
| `src/views/` | UI pane modules + `settings/` (5 tabs: ai_models, prompt_recipes, custom_tools, mcp_servers, tool_playground) |
| `src/views/update.rs` | Version-check banner via crates.io polling on startup |
| `src/views/theme.rs` | Color palette, layout metrics, dialog radii |
| `src/widgets/` | Custom `TextArea` (undo/redo, 100-deep, edit coalescing) + custom `DropDown` |
| `assets/` | Bundled: `preamble.md`, `workmode.md`, `rules/`, `models.ron`, `models.json`, `tools.ron`, `mcp.ron`, `images/` |
| `~/.crabot/` | User config: `settings.ron`, `models.ron`, `tools.ron`, `mcp.ron`, `preamble/`, `rules/` |
| `.agent/sessions/` | Session JSON (one file per conversation) |

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

`ToolRegistry` owns `Vec<ToolRef>` for builtin + MCP, plus `Vec<CustomTool>` for custom. Methods: `builtin_names()`, `custom_names()`, `all_names()`, `enabled_tools()`, `snapshot_todo()`, `clear_todo()`.

### Tool Categories

| Category | Source | Description |
|----------|--------|-------------|
| Builtin (9) | `src/tools/{read,write,edit,find,search,bash,ask,todo,fetch}.rs` | File I/O, shell, interaction, web — always available |
| Custom | `~/.crabot/tools.ron` | User-defined CLI tools with TinyTemplate + JSON Schema params |
| MCP | `~/.crabot/mcp.ron` → rmcp discovery | Remote tools from MCP servers (stdio or HTTP) |

### Built-in Tools

| Tool | Description |
|------|-------------|
| `read` | File read with offset/limit, 64KB cap, smart truncation |
| `write` | File write with parent dir creation |
| `edit` | Exact-string replacement via byte-range offsets, overlap detection |
| `find` | Glob finder, gitignore-aware, 100-line cap |
| `search` | Regex search across files, gitignore-aware |
| `bash` | Shell with timeout (default 120s), process-group kill |
| `ask` | Interactive prompt — intercepted by engine, routed to UI via mpsc |
| `todo` | Shared todo list (written by tool, displayed in right pane) |
| `fetch` | HTTP fetch with Markdown extraction via `dom_smoothie` |

### Custom Tools

Defined in `~/.crabot/tools.ron`. Each has: `command` (TinyTemplate), typed `parameters` (String/Integer/Number/Boolean/Array/Object/Union), and `instruction`. Spawn via `interprocess` pipes — no reader threads.

### MCP Tools

Configured in `~/.crabot/mcp.ron`. Each server: transport (`Stdio("cmd args")` or `Http("url")`), `qualify_tool_names`, optional `env` and `prompt`. Auto-connect on startup via `rmcp`; connections held in `LazyLock<Mutex<HashMap<String, McpConnection>>>` with `DropGuard` cleanup.

### Process Helpers (`src/tools/mod.rs`)

- Pipe I/O via `interprocess` (no reader threads)
- `wait_with_timeout()`: polls + drains pipes + kills process group on timeout
- `kill_process_tree()`: Unix `kill -9 -pgid` / Windows `taskkill /F /T`
- `truncate_output()`: 100KB cap, 3KB head + tail with notice
- `format_command_output()`: stdout + stderr (prefixed) + exit code
- `resolve_path()` / `resolve_path_partial()`: handles Unix-style `/c/...` and workspace-relative paths

---

## Conventions & Patterns

### Error Handling
- `Result<_, Box<dyn Error>>` or `Result<_, String>` — no `thiserror`/`anyhow`
- Tool `execute()` returns `Result<String, String>`
- `Settings::load()` → `Option<Settings>` (graceful fallback)
- Startup failures use `expect()`; path resolution via `candidate_path()`

### Async Patterns
- **Tokio** (Iced's built-in integration)
- **Channel streaming**: `iced::stream::channel` wraps task; `SessionEvent` → `ConversationEvent::SessionEvent(...)`
- **Tool execution**: builtin/custom → `spawn_blocking`; MCP → `block_in_place` + `handle.block_on`; ask → mpsc channel
- **Cancellation**: `AtomicBool` flag + `tokio::select!` races
- **Pending prompt**: `Arc<Mutex<Option<String>>>` for interrupt-and-resend

### State Management
- **6 domain state groups** in `App`: `LayoutState`, `PromptWorkspaceState`, `ToolState`, `ConversationState`, `ModelSettingsState`, `OverlayState`
- **Hierarchical `Message`** enum (6 variants): `Layout(LayoutEvent)`, `Prompt(PromptEvent)`, `Tools(ToolEvent)`, `Conversation(ConversationEvent)`, `Overlay(OverlayEvent)`, `ModelSettings(ModelSettingsEvent)`
- **`FocusedTarget`** enum: setting one implicitly clears others
- **Dual session**: `Session.history` (raw `Vec<ChatMessage>` for API) + `Session.dialogs` (UI `Vec<Dialog>`); `rebuild_dialogs()` syncs them
- **Placeholder streaming**: empty `Turn::assistant("")` pushed on `LlmThinking`, chunks appended, `handle_stream_done()` finalizes
- **Work modes**: Plan/Code/Review parsed from `workmode.md`; togglable; per-mode recipe templates

### Key Patterns
- **Domain event enums**: each `src/app/` sub-module defines its event type; `From` impls for `.into()` conversion
- **`ToolRegistry`** owns all tools; shared `TodoList` (`Arc<Mutex<Vec<TodoItem>>>`)
- **Cancel-aware `Tool::execute`**: default checks `AtomicBool` before `execute_inner`
- **Strict schema**: `make_strict_schema()` post-processes for models requiring strict tool calling
- **Triple persistence**: Models from RON (primary), OMP YAML, or PI JSON — cached as RON
- **Custom widgets**: `TextArea` (undo/redo, 100-deep `VecDeque<Snapshot>`, edit coalescing); `DropDown` (`on_open`, disabled style)
- **gh-emoji + json-escape**: emoji in chat with code-region awareness; JSON-safe tool output
- **CJK fonts**: auto-detection via `fontdb`
- **RFD file dialogs** for native file/workspace selection
- **Search bar**: Ctrl+F in center pane; case-insensitive across all turns (incl. reasoning); highlighted spans; scroll-to-match
- **Cache**: Anthropic rolling ephemeral breakpoint at conversation tail; system prompt `Ephemeral1h` TTL
- **Workspace modal**: in-app confirmation when workspace empty
- **Prompt Recipes**: per-work-mode templates in Settings → dropdown in left pane
- **Tool Playground**: interactive testing of any tool with JSON args, no LLM needed
- **Update notification**: crates.io poll via `reqwest` on startup; banner with release link

### Assets & Config
- Bundled via `include_dir!`; seeded to `~/.crabot/` on first boot
- API keys from env vars only, never stored on disk
- `AGENTS.md` in workspace: auto-detected, injectable into system prompt

---

## Runtime Preferences

| Requirement | Value |
|-------------|-------|
| Rust toolchain | Edition 2024, stable |
| Build | Cargo |
| Deps | `cargo add` |
| Format | `cargo fmt` |
| Lint | `cargo clippy` |
| Docs | `cargo doc --no-deps --document-private-items` (no `--open`) |
| OS | Linux, macOS, Windows (CREATE_NO_WINDOW) |
| Env vars | API keys via environment (`DEEPSEEK_API_KEY`, `OPENAI_API_KEY`, etc.) |

### CI
- `rust.yml`: push/PR → `cargo build --release` + `cargo test --verbose` on ubuntu-latest
- `release.yml`: `v*` tag → GitHub Release

### .gitignore
`/target`, `/tmp`, `/.agent`, `/.reasonix`, `reasonix.toml`, `nul`
