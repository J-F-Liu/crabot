# Repository Guidelines

## Project Overview

Crabot is a pure-Rust native GUI coding agent built with [iced](https://iced.rs) (Elm architecture) and [genai](https://crates.io/crates/genai) for multi-provider LLM access. It ships with 12 built-in tools — the `bash` tool runs on an in-process [bashkit](https://crates.io/crates/bashkit) interpreter, so it works natively on Windows without a real bash — plus user-defined custom tools, MCP server tools, and multi-tab sessions.

Everything configurable is meant to be visible and toggleable in the UI: model, work mode, tools, and the individual components of the system prompt.

## Architecture

### Three-pane GUI

A single window with a resizable left pane (model config, system-prompt sections, session picker, work mode, prompt editor, tool list), a center pane (session tabs, conversation turns, search bar, status bar), and a right pane (theme toggle, context-window/token stats, todo list, modified files with Revert / Revert All, process list, restart). All panes and the settings dialog live in `src/views/`.

### Data flow

- **UI → state:** `App::update` (`src/app.rs`) dispatches domain messages to handlers in `src/app/` (`layout`, `conversation`, `prompt`, `tool_state`, `settings`, `overlay`, `session_state`, `snapshot`).
- **Agent loop:** `llm::send_stream` sends a request with system prompt + tool declarations + history, streams reply chunks, executes tool calls, appends results, and repeats until the model stops calling tools. It handles cancellation, retries transient failures, and detects stalled streams. Independent tool calls run in parallel batches; interactive or file-mutating tools act as serial barriers.
- **Persistence:** RON config in `~/.crabot/` (settings, models, tools, mcp); sessions as JSONL under `.agent/sessions/YYYY-MM/`, appended one message at a time so a crash mid-turn loses nothing.
- **Snapshots:** `write`/`edit` targets are pre-imaged before tool execution into `.agent/snapshots/`, backing the right-pane Revert actions.
- **Assets:** bundled with `include_dir!` and seeded to `~/.crabot/` on first boot.
- **Extras:** MCP servers connect over stdio/HTTP via `rmcp`; an optional ACP bridge exposes sessions to external editors.

### Where things live

| Path                                                  | Role                                                                     |
| ----------------------------------------------------- | ------------------------------------------------------------------------ |
| `src/main.rs`, `src/app.rs`                           | Entry point; root `App` state, messages, boot/view/subscription          |
| `src/app/`                                            | Domain handlers split by concern (see data flow)                         |
| `src/views/`                                          | UI panes, custom styling, settings dialog tabs, HTML export, self-update |
| `src/widgets/`                                        | Custom widgets: `TextArea` (undo/redo), `DropDown`, `PopupMenu`          |
| `src/llm/`                                            | Streaming agent loop, retry/stall handling, parallel tool-call execution |
| `src/tools/`                                          | Tool trait, registry, built-ins, custom/MCP tools, process plumbing      |
| `src/chat.rs`, `src/session.rs`                       | Conversation UI types; raw history, JSONL persistence, todos             |
| `src/model.rs`, `src/model_database.rs`               | Model/provider/task-model types; embedded model database                 |
| `src/settings.rs`, `src/setup.rs`, `src/workspace.rs` | Persisted settings; first-boot seeding and logging; workspace scanning   |
| `src/user.rs`, `src/i18n.rs`, `src/fonts.rs`          | Work modes and user prompt; translations; CJK font handling              |
| `src/acp.rs`                                          | ACP bridge for external editor clients                                   |
| `assets/`                                             | Bundled preambles, skills, default config, images                        |
| `tests/`                                              | Integration tests (`bash`, `process`, `tools`, `session`, `chat`, …)     |

## Tool System

Tools implement a small trait with a name, description, JSON schema, and a blocking `execute` plus a streaming variant for live output. The registry groups them into built-in, custom, and per-MCP-server sets, and builds the declarations sent to the LLM; tools can be enabled or disabled per session.

| Category      | Source                | Notes                                                            |
| ------------- | --------------------- | ---------------------------------------------------------------- |
| Built-in (12) | `src/tools/builtin/`  | File I/O, shell, process lifecycle, interaction, web, delegation |
| Custom        | `~/.crabot/tools.ron` | User-defined CLI tools: templated command + typed parameters     |
| MCP           | `~/.crabot/mcp.ron`   | Remote tools from stdio/HTTP servers, auto-discovered on startup |

Built-ins: `read`, `write`, `edit` (file I/O with pagination, truncation, and overlap checks); `find`, `search` (gitignore-aware glob and regex search); `bash` (interpreter-backed shell with timeouts and cancellation); `process` (long-running process lifecycle); `ask` (interactive question to the user); `todo` (shared task list); `task` (delegate a subtask to a background session tab); `renew` (hand off to a fresh session when context is nearly full); `fetch` (web page → Markdown). See the README for parameters.

## Conventions

- **Errors:** plain `Result<_, String>` or `Result<_, Box<dyn Error>>`; no `anyhow`/`thiserror`; missing or malformed config falls back to defaults.
- **Logging:** `tracing` + `tracing-subscriber`, daily-rolling files under `~/.crabot/logs`, panic hook mirrors to a panic log.
- **Async:** Tokio integrated with iced streams; cancellation via `CancellationToken`; interactive tools talk to the UI over channels.
- **State & UI:** one root `App` with grouped state (models, settings, layout, prompt, tools, conversation, settings dialog, overlay) and a hierarchical message enum; each session tab owns its own streaming/search/scroll/model state.
- **Streaming UX:** placeholders are pushed when a turn starts and updated in place; `bash` and `process` forward live output, coalesced and capped before rendering.
- **Style:** short, purposeful comments; keep functions small and names meaningful; run `cargo fmt` and `cargo clippy` before wrapping up.

## Repository

- **CI:** `.github/workflows/rust.yml` (push/PR → `cargo build --release` + `cargo clippy -- -D warnings`), `.github/workflows/release.yml` (`v*` tag → GitHub Release).
- **Ignored:** `/target`, `/tmp`, `/.agent`, and local tooling directories (`/.reasonix`, `/.codebase-memory`, `/.codegraph`, `justfile`, `reasonix.toml`, `nul`).
- **`AGENTS.md`** at the workspace root is auto-detected by crabot and can be injected into the system prompt — keep it accurate and concise.
