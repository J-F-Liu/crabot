# Repository Guidelines

## Project Overview

Crabot is a pure-Rust native GUI coding agent. Built on [iced](https://iced.rs) (v0.14, Elm-architecture GUI) wrapping the [genai](https://crates.io/crates/genai) crate (v0.6) for multi-provider LLM interactions. Provides a chat-style interface for AI-assisted coding with six built-in tools (read, write, edit, find, search, bash) plus user-defined custom tools.

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
+------------------------------+---------------------------+-------------------------+
```

All panes live in `src/views/` as separate modules (`left_pane`, `center_pane`, `right_pane`, `model_config`, `system_prompt`, `session_view`, `tool_message`, `tool_list`, `user_prompt`, `theme`, `styles`).

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
        K -- spawn_blocking --> M[Tool execution]
        M -- result callback --> F
        K -- event callbacks --> F
    end
    subgraph Persist
        N[Settings::save] -- RON --> O[~/.crabot/settings.ron]
        P[ModelList::save] -- RON --> Q[~/.crabot/models.ron]
        R[Session::save] -- JSON --> S[.agent/sessions/{id}.json]
    end
    F --> N & P & R
```

### Agent Loop (`llm::send_stream`)

```rust
for _ in 0..50 {
    // 1. Send request with system prompt + tools + history
    // 2. Stream response chunks (text + reasoning) via callbacks
    // 3. If no tool calls → done
    // 4. Execute tool calls on blocking threads (spawn_blocking)
    // 5. Append results to history, loop
}
```

### Key Modules & Their Roles

| Module        | Path              | Responsibility                                                                                              |
| ------------- | ----------------- | ----------------------------------------------------------------------------------------------------------- |
| `main.rs`     | `src/main.rs`     | Entry point, `App` struct (~40 fields), ~40 `Message` variants, startup boot, view + update + subscriptions |
| `lib.rs`      | `src/lib.rs`      | `HashSetExt` trait — `.set()` for ergonomic toggle in HashSet                                               |
| `system.rs`   | `src/system.rs`   | `SystemPrompt` struct (7 toggleable components), prompt concatenation, `tools_summary()`                    |
| `settings.rs` | `src/settings.rs` | All persistable app state, RON serialization at `~/.crabot/settings.ron`                                    |
| `model.rs`    | `src/model.rs`    | `ModelList` (providers + models), `ModelConfig`, `Provider`, `Model`, `TokenAmount`                         |
| `chat.rs`     | `src/chat.rs`     | `Turn`, `TurnBody`, `Dialog` — conversation data types with Markdown caching                                |
| `session.rs`  | `src/session.rs`  | `Session` — raw genai `ChatMessage` history + derived UI dialogs + usage/cost                               |
| `llm.rs`      | `src/llm.rs`      | Streaming engine, agent loop, `StreamState` (Idle→LlmLoading→LlmThinking→ToolExecuting)                     |
| `setup.rs`    | `src/setup.rs`    | `ensure_default_files()` — seeds `~/.crabot/` with bundled assets                                           |
| `tools/`      | `src/tools/`      | Tool trait + 6 built-in tools + custom tool loader                                                          |
| `views/`      | `src/views/`      | UI pane components                                                                                          |
| `widgets/`    | `src/widgets/`    | Custom `TextArea` with undo/redo (100-deep)                                                                 |

---

## Key Directories

| Path                 | Purpose                                                                                                                                                                   |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/`               | Main source code                                                                                                                                                          |
| `src/tools/`         | Tool implementations: `mod.rs` (trait + registry), `read.rs`, `write.rs`, `edit.rs`, `find.rs`, `search.rs`, `bash.rs`, `custom.rs`                                       |
| `src/views/`         | UI pane modules: `left_pane`, `center_pane`, `right_pane`, `model_config`, `system_prompt`, `session_view`, `tool_message`, `tool_list`, `user_prompt`, `theme`, `styles` |
| `src/widgets/`       | Custom `TextArea` widget (`textarea.rs`)                                                                                                                                  |
| `assets/`            | Bundled defaults: `preamble.md`, `rules/rust.md`, `rules/web.md`, `models.ron`                                                                                            |
| `.agent/sessions/`   | Session JSON persistence (one file per conversation)                                                                                                                      |
| `tmp/`               | Dynamic runtime state (`tools.ron` for custom tool definitions)                                                                                                           |
| `vendor/`            | Empty placeholder for vendored deps                                                                                                                                       |
| `.github/workflows/` | CI: `rust.yml` (build+test on push/PR), `release.yml` (tag-based release)                                                                                                 |

### Tool System

| Tool       | File                  | Description                                                              |
| ---------- | --------------------- | ------------------------------------------------------------------------ |
| ReadTool   | `src/tools/read.rs`   | File read with offset/limit, 64KB cap, smart truncation                  |
| WriteTool  | `src/tools/write.rs`  | File write with parent dir creation                                      |
| EditTool   | `src/tools/edit.rs`   | Exact-string replacement via byte-range offsets, overlap detection       |
| FindTool   | `src/tools/find.rs`   | Glob file finder, respects `.gitignore`, capped at 100 lines             |
| SearchTool | `src/tools/search.rs` | Regex search across files, gitignore-aware                               |
| BashTool   | `src/tools/bash.rs`   | Shell executor with timeout (default 120s), process-group kill           |
| CustomTool | `src/tools/custom.rs` | User-defined RON-based CLI tools with TinyTemplate commands, JSON schema |

Tools implement a `Tool` trait (name, description, instruction, schema, execute). Global registries via `LazyLock`: `BUILTIN_TOOLS` (static `IndexMap`) + `CUSTOM_TOOLS` (dynamic `RwLock<IndexMap>`). `enabled_tools()` filters both by a `HashSet<String>`.

---

## Code Conventions & Common Patterns

### Error Handling

- **No `thiserror` or `anyhow`** detected — errors propagate via `Result<_, Box<dyn Error>>` or custom enums
- Tool `execute()` returns `Result<String, Box<dyn Error + Send + Sync>>`
- `Settings::load()` returns `Option<Settings>` — graceful fallback on missing/malformed files
- Startup failures handled with `expect()` in `main()` (early exit on missing essentials)
- Tool path resolution uses `candidate_path()` — handles Unix-style paths on Windows (`/c/Users/...`), native absolute, and workspace-relative — falls back to original path string

### Async Patterns

- **Tokio runtime** (Iced's built-in tokio integration)
- **Channel-based streaming**: `futures::channel::mpsc` Sender pushes `Message` into the Iced event loop; `iced::stream::channel` wraps the streaming task
- **Blocking tool execution**: `tokio::task::spawn_blocking` for tool `execute()` — keeps the UI responsive
- **Callback-as-channel**: `send_stream()` takes a closure returning `BoxFuture<'static, bool>`; returning `false` signals cancellation (checked against `AtomicBool`)
- **AtomicBool** for cancellation flag (shared between stream task and UI)

### State Management

- **Monolithic App struct** in `src/main.rs` — all state in ~40 fields of `App`
- **Unified `Message` enum** (~40 variants) — every user event, stream event, and internal action
- **No sub-reducers or component-local state** — standard Iced 0.14 pattern
- **Dual session representation**: `Session.history` (raw `Vec<ChatMessage>` for API) + `Session.dialogs` (UI-friendly `Vec<Dialog>`); `rebuild_dialogs()` reconstructs UI format from raw history
- **Placeholder-based streaming**: empty `Turn::assistant("")` pushed on send, chunks appended via `push_str`, `handle_stream_done()` backfills final content
- **Work modes**: Plan / Code / Review — user text wrapped in `<work-mode>...</work-mode>` tags

### Key Structural Patterns

- **LazyLock registries**: `BUILTIN_TOOLS` and `CUSTOM_TOOLS` as global statics populated at startup; `find_tool()` checks built-in first then custom
- **Strict schema enforcement**: `make_strict_schema()` post-processes tool JSON schemas to make all properties required (for models requiring strict tool calling)
- **Triple persistence format**: Models from RON (primary), OMP YAML, or PI JSON — read-once from OMP/PI then cached as RON
- **Workspace-relative tools**: `candidate_path()` handles `/c/...` (Windows MSYS), native absolute, and workspace-relative paths; output paths converted to Unix-style for LLM
- **Custom TextArea widget**: wraps `iced::text_editor::Content` with 100-deep undo/redo via `VecDeque<Snapshot>` stacks + cursor snapshotting
- **gh-emoji + json-escape**: emoji rendering in chat, JSON-safe tool output escaping
- **CJK font detection**: auto-loads system CJK fonts via `fontdb`
- **RFD file dialogs**: native system dialogs for file/workspace selection

### Assets & Configuration

- **Bundled via `include_dir!`** — `assets/` compiled into binary, seeded to `~/.crabot/` on first boot
- **`~/.crabot/settings.ron`** — all persistent app state (RON format)
- **`~/.crabot/models.ron`** — LLM provider configs (fallbacks: `~/.omp/agent/models.yml`, `~/.pi/agent/models.json`)
- **`~/.crabot/tools.ron`** — custom tool definitions
- **`assets/preamble.md`** — system prompt for the coding agent
- **`assets/rules/`** — domain-specific coding conventions (`rust.md`, `web.md`)
- **API keys**: env vars only — never stored on disk

---

## Important Files

| File                 | Why it matters                                                         |
| -------------------- | ---------------------------------------------------------------------- |
| `src/main.rs`        | Entry point, App struct, all ~40 Message variants, view/update/startup |
| `src/llm.rs`         | Agent loop, streaming, tool orchestration — the core AI pipeline       |
| `src/session.rs`     | Persistence format — raw history + derived dialogs + token accounting  |
| `src/tools/mod.rs`   | `Tool` trait definition, registry, strict schema, process helpers      |
| `src/settings.rs`    | All persistable UI state (window, panes, selections, history)          |
| `src/model.rs`       | Multi-provider model configuration and token accounting                |
| `assets/models.ron`  | Bundled LLM provider/model defaults (4 providers, 8 models)            |
| `assets/preamble.md` | Bootstrap agent system prompt                                          |
| `Cargo.toml`         | Dependency manifest — iced 0.14, genai 0.6, 30+ crates                 |
| `reasonix.toml`      | Reasonix agent harness configuration                                   |
| `CHANGELOG.md`       | Version history with Keep a Changelog format                           |

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
| **OS support**       | Linux, macOS, Windows (ActiveScript handles Windows paths)                     |  |
| **Env vars**         | API keys via environment variables (e.g. `DEEPSEEK_API_KEY`, `OPENAI_API_KEY`) |

### CI Pipeline

Two GitHub Actions workflows:

1. **`rust.yml`** — on push/PR to `main`: `cargo build --release` + `cargo test --verbose` (ubuntu-latest)
2. **`release.yml`** — on `v*` tag push: creates GitHub Release with auto-generated notes

---

### .gitignore

Ignores: `/target`, `/tmp`, `/.agent`, `/.reasonix`, `reasonix.toml`, `nul` (Windows sentinel).
