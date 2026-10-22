# Repository Guidelines

## Project Overview

Crabot is a pure-Rust native GUI coding agent built with [iced](https://iced.rs) (Elm architecture) and [genai](https://crates.io/crates/genai) for multi-provider LLM access. It ships with 12 built-in tools — the `bash` tool runs on an in-process [bashkit](https://crates.io/crates/bashkit) interpreter, so it works natively on Windows without a real bash — plus prompt attachments, user-defined custom tools, MCP server tools, and multi-tab sessions.

Everything configurable is meant to be visible and toggleable in the UI: model, work mode, tools, and the individual components of the system prompt.

## Architecture

### Three-pane GUI

A single window with a resizable left pane (model config, system-prompt sections, session picker, work mode, prompt editor, tool list), a center pane (session tabs, conversation turns, search bar, status bar), and a right pane (collapsible from the tab bar; theme toggle, context-window/token stats, todo list, modified files with Revert / Revert All and click-to-diff, process list, restart). All panes and the settings dialog live in `src/views/`.

### Data flow

- **UI → state:** `App::update` (`src/app.rs`) dispatches domain messages to handlers in `src/app/` (`layout`, `conversation`, `prompt`, `tool_state`, `settings`, `overlay`, `session_state`, `snapshot`, `diff`).
- **Per-tab pane options:** the live left-pane state sits in `App.prompt` / `App.tools` and mirrors the viewing tab. Each `SessionTab` keeps a `PromptOptions` snapshot (`src/app/prompt_options.rs`) of the model/preamble picks, system-prompt toggles, skills, work mode, workspace (with its `AGENTS.md`), and enabled tools; `App::set_viewing` captures the outgoing tab and restores the target's, including its workspace scans and any missing MCP connections. The prompt draft and attachments stay shared, and MCP connections are process-wide. A background tab (task sub-agent) composes its prompt from its own snapshot, and its tools section is generated from its own enabled tools.
- **Agent loop:** `llm::send_stream` sends a request with system prompt + tool declarations + history, streams reply chunks, executes tool calls, appends results, and repeats until the model stops calling tools. It handles cancellation, retries transient failures (honoring provider `retry-after` / `x-should-retry`), bounds response-header waits with the stall timeout, and detects stalled streams. Independent tool calls run in parallel batches; interactive or file-mutating tools act as serial barriers.
- **Persistence:** RON config in `~/.crabot/` (settings, models, tools, mcp); sessions as JSONL under `.agent/sessions/YYYY-MM/`, appended one message at a time so a crash mid-turn loses nothing; prompt attachments are recorded as path markers and materialized when a request is built. Provider entries may carry extra HTTP headers and an `extra_body` JSON object, both merged into every request.
- **Snapshots:** `write`/`edit` targets are pre-imaged before tool execution into `.agent/snapshots/`, backing the right-pane Revert actions; clicking a modified file spawns `code --diff` on a copy of that pre-image against the current file (`src/app/diff.rs`).
- **Assets:** bundled with `include_dir!` and seeded to `~/.crabot/` on first boot.
- **Extras:** MCP servers connect over stdio/HTTP via `rmcp`; an optional ACP bridge exposes sessions to external editors; `CRABOT_TRACE_RAW_FRAMES=1` logs raw provider frames for debugging.

### Where things live

| Path                                                  | Role                                                                     |
| ----------------------------------------------------- | ------------------------------------------------------------------------ |
| `src/main.rs`, `src/app.rs`                           | Entry point; root `App` state, messages, boot/view/subscription          |
| `src/app/`                                            | Domain handlers split by concern (see data flow)                         |
| `src/views/`                                          | UI panes, custom styling, settings dialog tabs, HTML export, self-update |
| `src/widgets/`                                        | Custom widgets: `TextArea` (undo/redo), `DropDown`, `PopupMenu`          |
| `src/llm/`                                            | Streaming agent loop, retry/stall handling, parallel tool-call execution, attachment/image materialization |
| `src/tools/`                                          | Tool trait, registry, built-ins, custom/MCP tools, process plumbing      |
| `src/chat.rs`, `src/session.rs`                       | Conversation UI types; raw history, JSONL persistence, todos             |
| `src/model.rs`, `src/model_database.rs`               | Model/provider/task-model types; embedded model database generated from models.dev/models-cn |
| `src/settings.rs`, `src/setup.rs`, `src/workspace.rs` | Persisted settings; first-boot seeding and logging; workspace scanning   |
| `src/user.rs`, `src/i18n.rs`, `src/fonts.rs`          | Work modes, user prompt, and attachments; translations; CJK font handling |
| `src/acp.rs`                                          | ACP bridge for external editor clients                                   |
| `assets/`                                             | Bundled preambles, skills, default config, images                        |
| `examples/`                                           | `update_models` regenerates the model database; session stats and JSONL conversion |
| `tests/`                                              | Integration tests (`bash`, `process`, `tools`, `session`, `chat`, …)     |

## Tool System

Tools implement a small trait with a name, description, JSON schema, and a blocking `execute` plus a streaming variant for live output. The registry groups them into built-in, custom, and per-MCP-server sets, and builds the declarations sent to the LLM; tools can be enabled or disabled per session.

| Category      | Source                | Notes                                                            |
| ------------- | --------------------- | ---------------------------------------------------------------- |
| Built-in (12) | `src/tools/builtin/`  | File I/O, shell, process lifecycle, interaction, web, delegation |
| Custom        | `~/.crabot/tools.ron` | User-defined CLI tools: templated command + typed parameters     |
| MCP           | `~/.crabot/mcp.ron`   | Remote tools from stdio/HTTP servers, auto-discovered on startup |

Built-ins: `read`, `write`, `edit` (file I/O with pagination, truncation, and overlap checks; `read` attaches image files as pictures instead of returning bytes, and one shared image policy in `src/tools/image.rs` decides the budget, the delivered size, and the encoding for both the tool result and the request); `find`, `search` (gitignore-aware glob and regex search); `bash` (interpreter-backed shell with timeouts and cancellation); `process` (long-running process lifecycle); `ask` (interactive question to the user); `todo` (shared task list); `task` (delegate a subtask to a background session tab); `renew` (hand off to a fresh session when context is nearly full); `fetch` (web page → Markdown). See the README for parameters.

## Conventions

- **Errors:** plain `Result<_, String>` or `Result<_, Box<dyn Error>>`; no `anyhow`/`thiserror`; missing or malformed config falls back to defaults.
- **Logging:** `tracing` + `tracing-subscriber`, daily-rolling files under `~/.crabot/logs`, panic hook mirrors to a panic log.
- **Async:** Tokio integrated with iced streams; cancellation via `CancellationToken`; interactive tools talk to the UI over channels.
- **State & UI:** one root `App` with grouped state (models, settings, layout, prompt, tools, conversation, settings dialog, overlay) and a hierarchical message enum; each session tab owns its own streaming/search/scroll state plus its `PromptOptions` (model, preamble, system-prompt toggles, skills, work mode, workspace, enabled tools), restored when switching back to it.
- **Streaming UX:** placeholders are pushed when a turn starts and updated in place; `bash` and `process` forward live output, coalesced and capped before rendering.
- **Tool output normalization:** grep-style runs that repeat a file path (`path:12:text`, `path-12-text` for context) collapse into a `path:` heading plus the original `line…` tails (`src/tools/normalize.rs`), applied once in `build_tool_result` so the model, the UI, the session file, and the HTML export all carry the same text; the live stream stays raw.
- **Images & attachments:** history stores only path markers. When a request is built, `src/llm/attachment.rs` reads text attachments (bounded by the `read` limit and truncated with a marker) and `src/llm/image.rs` encodes images under a per-request allowance, spending it on the newest pictures and degrading the rest to a text notice; chat previews are decoded lazily in `src/app/conversation.rs`.
- **Model database:** `assets/models.json` is generated from models.dev/models-cn by `cargo run --example update_models`; don't hand-edit it.
- **Style:** short, purposeful comments; keep functions small and names meaningful; run `cargo fmt` and `cargo clippy` before wrapping up.

## Repository

- **CI:** `.github/workflows/rust.yml` (push/PR → `cargo build --release` + `cargo clippy -- -D warnings`), `.github/workflows/release.yml` (`v*` tag → GitHub Release).
- **Ignored:** `/target`, `/tmp`, `/.agent`, and local tooling directories (`/.reasonix`, `/.codebase-memory`, `/.codegraph`, `justfile`, `reasonix.toml`, `nul`).
- **`AGENTS.md`** at the workspace root is auto-detected by crabot and can be injected into the system prompt — keep it accurate and concise.
