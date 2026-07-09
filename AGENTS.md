# Repository Guidelines

## Project Overview

Crabot is a pure-Rust native GUI coding agent. Built on [iced](https://iced.rs) (v0.14, Elm-architecture GUI) wrapping the [genai](https://crates.io/crates/genai) crate (v0.6) for multi-provider LLM interactions. Provides a chat-style interface for AI-assisted coding with six built-in tools (read, write, edit, find, search, bash), user-defined custom tools, and MCP (Model Context Protocol) server tools.

---

## Architecture & Data Flow

### Three-pane Iced GUI

```
+------------------------------+---------------------------+-------------------------+
| LEFT (~280px, scrollable)    | CENTER (fills remaining)  | RIGHT (~260px)          |
| Model config (tabbed)        | Session header +          | Context window stats    |
| System prompt sections       | collapsible dialog panels | (tokens, cached, %)     |
| Session picker               |  → turn bubbles           | Cumulative token usage  |
| User prompt textarea         |    (User/Assistant/Tool)  | Session cost            |
| Work mode picklist           | Status bar + stop button  | Modified files list     |
| Tool enable checkboxes       |                           | Restart button          |
| (builtin / custom / MCP)     |                           |                         |
+------------------------------+---------------------------+-------------------------+
```

All panes live in `src/views/` as separate modules (`left_pane`, `center_pane`, `right_pane`, `model_config`, `system_prompt`, `session_view`, `tool_message`, `tool_list`, `user_prompt`, `theme`, `styles`, `modal`).

### Data Flow

```mermaid
flowchart LR
    subgraph UI
        A[App::view] --> B[3-pane layout]
    end
    subgraph State
        F[App::update] -- mutates --> G[App state]
        H[subscriptions] -- events --> F
    end
    subgraph LLM
        I[Message::SendPrompt] --> J[start_dialog]
        J -- channel stream --> K[tokio task: send_stream]
        K -- genai client --> L[LLM API]
        K -- block_in_place + spawn_blocking --> M[Tool execution]
        M -- result callback --> F
        K -- event callbacks --> F
    end
    subgraph Persist
        N[Settings::save] -- RON --> O[~/.crabot/settings.ron]
        P[ModelList::save] -- RON --> Q[~/.crabot/models.ron]
        R[Session::save] -- JSON --> S[.agent/sessions/{id}.json]
    end
    subgraph MCP
        T[McpList::load] -- RON --> U[~/.crabot/mcp.ron]
        T -- spawn child / HTTP --> V[MCP servers]
        V -- rmcp --> W[discovered tools]
    end
    F --> N & P & R
```

### Agent Loop (`llm::send_stream`)

```rust
for _ in 0..50 {
    // 1. Send request with system prompt + tools + history
    // 2. Stream response chunks (text + reasoning) via callbacks
    // 3. If no tool calls → done
    // 4. Execute tool calls (block_in_place + block_on for MCP; spawn_blocking for builtin/custom)
    // 5. Append results to history, loop
}
```

### Key Modules & Their Roles

| Module         | Path               | Responsibility                                                                                              |
| -------------- | ------------------ | ----------------------------------------------------------------------------------------------------------- |
| `main.rs`      | `src/main.rs`      | Entry point, `App` struct (~45 fields), ~45 `Message` variants, startup boot, view + update + subscriptions |
| `lib.rs`       | `src/lib.rs`       | `HashSetExt` trait — `.set()` for ergonomic toggle in HashSet                                               |
| `system.rs`    | `src/system.rs`    | `SystemPrompt` struct (7 toggleable components), prompt concatenation, `tools_summary()`                    |
| `settings.rs`  | `src/settings.rs`  | All persistable app state, RON serialization at `~/.crabot/settings.ron`                                    |
| `model.rs`     | `src/model.rs`     | `ModelList` (providers + models), `ModelConfig`, `Provider`, `Model`, `TokenAmount`                         |
| `chat.rs`      | `src/chat.rs`      | `Turn`, `TurnBody`, `Dialog` — conversation data types with Markdown caching                                |
| `session.rs`   | `src/session.rs`   | `Session` — raw genai `ChatMessage` history + derived UI dialogs + usage/cost                               |
| `llm.rs`       | `src/llm.rs`       | Streaming engine, agent loop, `StreamState` (Idle→LlmLoading→LlmThinking→ToolExecuting)                     |
| `setup.rs`     | `src/setup.rs`     | `ensure_default_files()` — seeds `~/.crabot/` with bundled assets                                           |
| `workspace.rs` | `src/workspace.rs` | Workspace tree scanner, respect `.gitignore` / `.ignore` / hidden files                                     |
| `user.rs`      | `src/user.rs`      | `UserPrompt` — wraps text in `<work-mode>` tags; `WorkMode` enum (Plan/Code/Review)                         |
| `fonts.rs`     | `src/fonts.rs`     | System font loading with CJK auto-detection via `fontdb`                                                    |
| `tools/`       | `src/tools/`       | `Tool` trait + `ToolRegistry` + 6 built-in tools + custom loader + MCP client                               |
| `views/`       | `src/views/`       | UI pane components                                                                                          |
| `widgets/`     | `src/widgets/`     | Custom `TextArea` with undo/redo (100-deep)                                                                 |

---

## Key Directories

| Path                 | Purpose                                                                                                                                                                            |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/`               | Main source code                                                                                                                                                                   |
| `src/tools/`         | Tool implementations: `mod.rs` (trait + registry + process helpers), `read.rs`, `write.rs`, `edit.rs`, `find.rs`, `search.rs`, `bash.rs`, `custom.rs`, `mcp.rs`                    |
| `src/views/`         | UI pane modules: `left_pane`, `center_pane`, `right_pane`, `model_config`, `system_prompt`, `session_view`, `tool_message`, `tool_list`, `user_prompt`, `theme`, `styles`, `modal` |
| `src/widgets/`       | Custom `TextArea` widget (`textarea.rs`)                                                                                                                                           |
| `assets/`            | Bundled defaults: `preamble.md`, `rules/rust.md`, `rules/web.md`, `models.ron`, `tools.ron`, `mcp.ron`                                                                             |
| `~/.crabot/`         | User config directory: `settings.ron`, `models.ron`, `tools.ron`, `mcp.ron`                                                                                                        |
| `.agent/sessions/`   | Session JSON persistence (one file per conversation)                                                                                                                               |
| `vendor/`            | Empty placeholder for vendored deps                                                                                                                                                |
| `.github/workflows/` | CI: `rust.yml` (build+test on push/PR), `release.yml` (tag-based release)                                                                                                          |

### Tool System

The `Tool` trait is defined in `src/tools/mod.rs`:

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn instruction(&self) -> &str;
    fn schema(&self) -> Value;
    fn execute(&self, args: &Value, workspace: &Path) -> Result<String, String>;
    fn tool_declaration(&self, strict: bool) -> GenaiTool { ... }
}
```

Tools are registered in a `ToolRegistry` struct (not global statics) with three categories:

| Category | Source                                 | Description                                                     |
| -------- | -------------------------------------- | --------------------------------------------------------------- |
| Builtin  | `read.rs`, `write.rs`, `edit.rs`, etc. | 6 file-system + shell tools (always available)                  |
| Custom   | `~/.crabot/tools.ron`                  | User-defined CLI tools with TinyTemplate commands + JSON Schema |
| MCP      | `~/.crabot/mcp.ron` → rmcp discovery   | Remote tools auto-discovered from MCP servers (stdio or HTTP)   |

The `ToolRegistry` owns three `Vec<ToolRef>` collections and exposes `builtin_names()`, `custom_names()`, `mcp_names()`, `all_names()`, and `enabled_tools(&HashSet)`.

#### Built-in Tools

| Tool       | File                  | Description                                                        |
| ---------- | --------------------- | ------------------------------------------------------------------ |
| ReadTool   | `src/tools/read.rs`   | File read with offset/limit, 64KB cap, smart truncation            |
| WriteTool  | `src/tools/write.rs`  | File write with parent dir creation                                |
| EditTool   | `src/tools/edit.rs`   | Exact-string replacement via byte-range offsets, overlap detection |
| FindTool   | `src/tools/find.rs`   | Glob file finder, respects `.gitignore`, capped at 100 lines       |
| SearchTool | `src/tools/search.rs` | Regex search across files, gitignore-aware                         |
| BashTool   | `src/tools/bash.rs`   | Shell executor with timeout (default 120s), process-group kill     |

#### Custom Tools

User-defined tools in `~/.crabot/tools.ron` using RON format. Each `CustomTool` has:
- A `command` string using [TinyTemplate](https://docs.rs/tinytemplate/) syntax for argument substitution and conditionals
- Typed `parameters` (`String`, `Integer`, `Number`, `Boolean`, `Array`, `Object`, `Union`) with JSON Schema generation
- An `instruction` string for LLM guidance

Custom tools spawn child processes with unnamed pipes (via `interprocess`) for stdout/stderr capture in non-blocking mode — no reader threads.

#### MCP Tools

MCP (Model Context Protocol) servers are configured in `~/.crabot/mcp.ron`. Each server specifies:
- A transport: `Stdio("command args")` (spawns child process) or `Http("http://...")` (streamable HTTP)
- `qualify_tool_names`: whether to prefix tool names with the server name
- Optional `env` variables for the child process

On startup, Crabot connects to each server via `rmcp`, auto-discovers tools, and registers them as `McpTool` implementations. Connections are retained in a `LazyLock<Mutex<Vec<McpConnection>>>` for the lifetime of the process.

### Process Execution Helpers (`src/tools/mod.rs`)

- **Pipe-based I/O**: Unnamed pipes (`interprocess` crate) capture stdout/stderr in non-blocking mode — no reader threads, avoiding thread leaks from surviving grandchildren
- **`wait_with_timeout()`**: Polls child process with pipe draining; kills process group on timeout
- **`kill_process_tree()`**: Unix `kill -9 -pgid` / Windows `taskkill /F /T`
- **`truncate_output()`**: 100KB cap, keeping 3KB head + tail with truncation notice
- **`format_command_output()`**: Combines stdout, stderr (with `STDERR:` prefix), and exit code

---

## Code Conventions & Common Patterns

### Error Handling

- **No `thiserror` or `anyhow`** — errors propagate via `Result<_, Box<dyn Error>>` or `Result<_, String>`
- Tool `execute()` returns `Result<String, String>` for ergonomic error display
- `Settings::load()` returns `Option<Settings>` — graceful fallback on missing/malformed files
- Startup failures handled with `expect()` in `main()` (early exit on missing essentials)
- Tool path resolution uses `candidate_path()` — handles Unix-style paths on Windows (`/c/Users/...`), native absolute, and workspace-relative — falls back to original path string

### Async Patterns

- **Tokio runtime** (Iced's built-in tokio integration)
- **Channel-based streaming**: `futures::channel::mpsc` Sender pushes `Message` into the Iced event loop; `iced::stream::channel` wraps the streaming task
- **Tool execution**:
  - Built-in and custom tools: `tokio::task::spawn_blocking` — keeps UI responsive
  - MCP tools: `tokio::task::block_in_place` + `handle.block_on` — MCP calls are async (rmcp Peer is `Send + Sync` but not `Sync` across blocking threads)
- **Callback-as-channel**: `send_stream()` takes a closure returning `BoxFuture<'static, bool>`; returning `false` signals cancellation (checked against `AtomicBool`)
- **AtomicBool** for cancellation flag (shared between stream task and UI)
- **Pending user prompt**: `Arc<Mutex<Option<String>>>` shared slot — checked in the agent loop for interrupt-and-resend

### State Management

- **Monolithic App struct** in `src/main.rs` — all state in ~45 fields of `App`
- **Unified `Message` enum** (~45 variants) — every user event, stream event, and internal action
- **FocusedTarget enum**: centralised keyboard focus — setting one target implicitly clears all others (no manual `set_focused(false)`)
- **ModelConfigEvent sub-reducer**: `views::model_config::update()` handles nested model configuration state
- **Dual session representation**: `Session.history` (raw `Vec<ChatMessage>` for API) + `Session.dialogs` (UI-friendly `Vec<Dialog>`); `rebuild_dialogs()` reconstructs UI format from raw history
- **Placeholder-based streaming**: empty `Turn::assistant("")` pushed on `LlmThinking`, chunks appended via `push_str`, `handle_stream_done()` backfills final content
- **Work modes**: Plan / Code / Review — user text wrapped in `<work-mode>...</work-mode>` tags

### Key Structural Patterns

- **`ToolRegistry`** owns all tools (builtin + custom + MCP) — replaces old `LazyLock<IndexMap>` globals
- **Strict schema enforcement**: `make_strict_schema()` post-processes tool JSON schemas to make all properties required + nullable optionals, for models requiring strict tool calling
- **Triple persistence format**: Models from RON (primary), OMP YAML, or PI JSON — read-once from OMP/PI then cached as RON
- **Workspace-relative tools**: `candidate_path()` handles `/c/...` (Windows MSYS), native absolute, and workspace-relative paths; `resolve_path()` / `resolve_path_partial()` for canonicalized/non-existent paths
- **Custom TextArea widget**: wraps `iced::text_editor::Content` with 100-deep undo/redo via `VecDeque<Snapshot>` stacks + cursor snapshotting
- **gh-emoji + json-escape**: emoji rendering in chat, JSON-safe tool output escaping
- **CJK font detection**: auto-loads system CJK fonts via `fontdb`
- **RFD file dialogs**: native system dialogs for file/workspace selection
- **Workspace modal**: in-app confirmation dialog when workspace is empty (prompts before defaulting to `~/.crabot`)

### Assets & Configuration

- **Bundled via `include_dir!`** — `assets/` compiled into binary, seeded to `~/.crabot/` on first boot
- **`~/.crabot/settings.ron`** — all persistent app state (RON format)
- **`~/.crabot/models.ron`** — LLM provider configs (fallbacks: `~/.omp/agent/models.yml`, `~/.pi/agent/models.json`)
- **`~/.crabot/tools.ron`** — custom tool definitions
- **`~/.crabot/mcp.ron`** — MCP server configurations
- **`assets/preamble.md`** — system prompt for the coding agent
- **`assets/rules/`** — domain-specific coding conventions (`rust.md`, `web.md`)
- **AGENTS.md in workspace** — auto-detected and injectable into system prompt via checkbox
- **API keys**: env vars only — never stored on disk

---

## Important Files

| File                  | Why it matters                                                         |
| --------------------- | ---------------------------------------------------------------------- |
| `src/main.rs`         | Entry point, App struct, all ~45 Message variants, view/update/startup |
| `src/llm.rs`          | Agent loop, streaming, tool orchestration — the core AI pipeline       |
| `src/session.rs`      | Persistence format — raw history + derived dialogs + token accounting  |
| `src/tools/mod.rs`    | `Tool` trait, `ToolRegistry`, strict schema, process helpers           |
| `src/tools/mcp.rs`    | MCP client — server connection, tool discovery, `McpTool` wrapper      |
| `src/tools/custom.rs` | Custom tool loader with TinyTemplate commands and typed parameters     |
| `src/settings.rs`     | All persistable UI state (window, panes, selections, history)          |
| `src/model.rs`        | Multi-provider model configuration and token accounting                |
| `assets/models.ron`   | Bundled LLM provider/model defaults (4 providers, 8 models)            |
| `assets/preamble.md`  | Bootstrap agent system prompt                                          |
| `Cargo.toml`          | Dependency manifest — iced 0.14, genai 0.6, rmcp 2, 45+ crates         |
| `CHANGELOG.md`        | Version history with Keep a Changelog format                           |

## Runtime / Tooling Preferences

| Requirement          | Value                                                                          |
| -------------------- | ------------------------------------------------------------------------------ |
| **Rust toolchain**   | Edition 2024, stable channel                                                   |
| **Build system**     | Cargo                                                                          |
| **No Node/Bun/Deno** | Pure Rust desktop app — no JS runtime needed                                   |
| **Package manager**  | `cargo add` for deps                                                           |
| **Formatter**        | `cargo fmt` (standard rustfmt config)                                          |
| **Linter**           | `cargo clippy`                                                                 |
| **Documentation**    | `cargo doc --no-deps --document-private-items` (no `--open`)                   |
| **OS support**       | Linux, macOS, Windows (ActiveScript handles Windows paths)                     |
| **Env vars**         | API keys via environment variables (e.g. `DEEPSEEK_API_KEY`, `OPENAI_API_KEY`) |

### CI Pipeline

Two GitHub Actions workflows:

1. **`rust.yml`** — on push/PR to `main`: `cargo build --release` + `cargo test --verbose` (ubuntu-latest)
2. **`release.yml`** — on `v*` tag push: creates GitHub Release with auto-generated notes

---

### .gitignore

Ignores: `/target`, `/tmp`, `/.agent`, `/.reasonix`, `reasonix.toml`, `nul` (Windows sentinel).
