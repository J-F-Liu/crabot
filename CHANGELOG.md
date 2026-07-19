# Crabot v0.5.0

- **Three new built-in tools** — `fetch` (download web pages and convert to Markdown), `ask` (interactive user prompt for agent confirmation), and `todo` (manage and display task lists). Todo items are rendered in a table in the conversation pane.
- **Prompt recipes** — quickly populate the user prompt from a dropdown of predefined recipe templates, saving time on common tasks.
- **Update notifications** — Crabot checks for new releases on startup and notifies you when an update is available.
- **Session metadata** — each session now displays its model ID and creation time for better tracking.
- **Max output tokens** — configure the maximum number of output tokens per request in model settings for finer control.
- **Anthropic cache control** — explicit cache control management in LLM interactions, taking effect in the Anthropic provider.
- **Improved context window** — the "Window used" percentage is now more accurate and shows one decimal place.
- **Enhanced TextArea** — undo now coalesces by run rather than single keystrokes, making text recovery more intuitive.
- **Smoother streaming** — markdown is no longer refreshed on every chunk event, keeping the UI responsive during fast streaming.
- **Branded app** — window and executable now include the Crabot logo and icon.
- **Early stop in LLM wait** — the Stop button now works while waiting for the LLM response to begin, not just during generation.
- **Prompt recipes and optional work modes** — work mode is now optional in system and user prompts, and prompt recipes are selectable from a dropdown.
- **Better search** — session dialog search is now case-insensitive and safer on non-ASCII text.
- **Improved error handling** — empty `old_text` is validated before search in EditTool, SearchTool and TodoTool have better error messages, and work mode extraction is more robust.
- **Normalized line endings** — user prompt, edit, and write tools now normalize line endings for cross-platform consistency.
- **Edit message numbering** — edit tool messages now use 1-based numbering for clarity.
- **Token cost accuracy** — cache write costs are now properly accounted for in token cost calculations.
- **Signal cancellation on close** — ensures proper cleanup when closing the app while a session is active.

**Full Changelog**: [`v0.4.0...v0.5.0`](https://github.com/J-F-Liu/crabot/compare/v0.4.0...v0.5.0)

# Crabot v0.4.0

- **MCP (Model Context Protocol) support** — connect to external MCP servers via Stdio or HTTP transport. Tools are auto-discovered from each server, displayed grouped by server name in the tools UI, with per-server toggle checkboxes. Configure servers in `~/.crabot/mcp.ron`.
- **MCP custom prompts** — MCP servers can inject custom prompt text directly into the system prompt, giving tools access to usage instructions.
- **MCP custom HTTP headers** — set per-server headers (e.g. API keys) directly in the MCP server config.
- **Unified ToolRegistry** — all tools (built-in, custom, MCP) are now managed by a central `ToolRegistry`, replacing the old static globals. Enables consistent tool lifecycle and toggle logic.
- **Immediate Stop** — the Stop button now cancels in-progress bash, custom, *and* MCP tool executions instantly.
- **Session search** — `Ctrl+F` opens a search bar for finding keywords in session dialogs. Navigate between matches with arrow buttons and hit counters.
- **Syntax highlighting** — both assistant responses and reasoning blocks are now rendered as full Markdown with syntax highlighting in fenced code blocks.
- **In-app modal dialogs** — replaced the external `rfd::MessageDialog` with native iced modals for workspace confirmation and other prompts, matching Crabot's visual style.
- **Collapsible tool sections** — built-in and custom tool lists are independently collapsible, giving you finer control over the left pane layout.
- **Context window precision** — the window usage percentage now shows one decimal place for more accurate tracking.
- **No console flash on Windows** — MCP Stdio servers and custom tools no longer flash a visible console window at startup.
- **PATH resolution for MCP commands** — bare command names in MCP server configs (e.g. `npx`) now resolve via the system PATH.
- **Bug fix: tool toggle enforcement** — tool enable/disable checkboxes are now correctly respected during agent tool execution.
- **Default tools.ron** — a default `~/.crabot/tools.ron` is auto-created on first boot alongside the other config files.
- **Architecture docs** — a comprehensive `AGENTS.md` now documents the codebase architecture, data flow, and conventions for contributors.

**Full Changelog**: [`v0.3.0...v0.4.0`](https://github.com/J-F-Liu/crabot/compare/v0.3.0...v0.4.0)

# Crabot v0.3.0

- User-defined **custom tools** via `~/.crabot/tools.ron` — CLI commands with TinyTemplate argument substitution and JSON Schema parameters, toggleable in the UI alongside built-in tools.
- **Model tab bar** for one-click switching between configured models — replaces the dropdown with always-visible tabs.
- **OpenAI strict mode** support for tool calling — models receive strict-mode-compatible JSON schemas when enabled in the provider config.
- Bundled **default coding rules** (`rust.md`, `web.md`) for zero-config first boot, selectable from a dropdown picker.
- **AGENTS.md auto-detection** — if an `AGENTS.md` file is present in the workspace, Crabot offers a checkbox to inject it into the system prompt.
- **Bash tool per-call timeout** — configurable timeout parameter with more reliable process-group termination.
- **Grouped tool calls** — multiple tool calls from a single assistant response now display as one collapsible turn group.
- **Interrupt & resend** — send a new prompt while the agent is streaming; the current stream cancels and the new prompt starts immediately.
- **Collapsible right pane** — drag below minimum width to hide, single-click the divider to restore. Divider handles now have hover feedback.
- **Font size shortcuts** — `Ctrl +` / `Ctrl -` adjust the chat font size. A **monospace font** family improves code display.
- **Session picker keyboard navigation** — arrow through and select sessions without the mouse.
- **Thinner vertical scrollbars** — less obtrusive in all panes.
- **Emoji shortcode fix** — `:emoji:` conversion now correctly skips inline code and fenced code blocks.
- **Workspace fallback confirmation** — Crabot prompts before defaulting to `~/.crabot` when no workspace is set.
- **Updated system preamble** with latest conventions and tool descriptions.
- **GitHub Actions release CI** — releases are auto-created when a version tag is pushed.

**Full Changelog**: [`v0.2.0...v0.3.0`](https://github.com/J-F-Liu/crabot/compare/v0.2.0...v0.3.0)

# Crabot v0.2.0

## What's New

### 🚀 Zero-Config First Boot
Crabot now ships with bundled default model configs (`assets/models.ron`) and a preamble (`assets/preamble.md`). On first launch, if no config files exist, these defaults are automatically installed — you can start chatting immediately.

### 💬 Session Management
- **Sessions dropdown** — list and switch between past sessions from a dropdown in the left pane.
- **Collapsible dialogs** — each turn group (user prompt → assistant response → tool calls) is a titled, collapsible dialog with turn counts.
- **Dialog history** — full conversation history is reconstructed from saved sessions.
- **Cumulative cost tracking** — token usage and cost are persisted per session and displayed per-dialog.

### 🛠 Enhanced Built-in Tools
- **Streaming read** — the `read` tool now streams output in chunks for large files.
- **Batch edit** — the `edit` tool can apply multiple edits in a single call.
- **Bash timeout** — shell commands time out after 120 seconds.
- **Output truncation** — long tool outputs are intelligently truncated to save context.
- **Edit diffs in UI** — when the `edit` tool runs, a visual diff is displayed inline.
- **Reduced noise** — `read` tool output is cleaner and more compact.

### 🌲 Workspace Tree
- The workspace tree is now refreshed on each new session.
- Directory scanning respects standard ignore rules: hidden files, `.gitignore`, `.ignore`, and glob-based ignore patterns.

### 📝 Modified Files Panel
- Files modified by the agent during a session are tracked and displayed in the right pane for quick review.

### 🎨 UI & UX Polish
- **Send on Enter**, newline with Shift+Enter.
- **Undo/Redo** in the prompt input (Ctrl+Z / Ctrl+Y).
- **Shift+Click** text selection in the prompt input.
- **Double-click** rendered Markdown to view raw text; press Escape to re-render.
- **Stop button** to cancel an in-progress session.
- **Auto-scroll** pauses during streaming when you scroll up manually.
- **Window position and size** are restored on restart.

### ⚙️ Configuration Improvements
- Settings and models are now saved in **RON format** (nicely structured, human-readable).
- **API keys can reference environment variables** (e.g. `OPENAI_API_KEY`), so keys never touch disk.
- **CJK font auto-detection** — system CJK fonts are automatically discovered and set as the default sans-serif family.

### 💰 Token Cost Display
- Per-response token counts and cost estimates are shown in the right pane.

### 🔍 Status Bar
- The status text now distinguishes four phases: Loading, Thinking, Tool Executing, and Idle.
- Tool call names are shown before execution starts.

### 🎭 Other Improvements
- **GitHub-style emoji** shortcodes (`:tada:`) are rendered in assistant responses.
- The preamble is always loaded fresh from the `.md` file, not from cached settings.
- Input tokens are reported as total (not uncached).

---

**Full Changelog**: [`v0.1...v0.2.0`](https://github.com/J-F-Liu/crabot/compare/v0.1...v0.2.0)

# Crabot v0.1.0

A smart and powerful coding agent with a native GUI, built entirely in Rust.

## Getting Started

```sh
cargo install crabot
```

Or from source:

```sh
cargo install --git https://github.com/J-F-Liu/crabot
```

## Highlights

- **Native GUI** — no terminal UI. A responsive three-pane layout (config / chat / details) built with `iced`, making it approachable for everyone.
- **No config files** — all configuration happens through dialogs and panels in-app. Settings persist automatically to `~/.crabot/settings.json`.
- **Multi-provider LLM support** — auto-discovers providers and models from `~/.omp/agent/models.yml` and `~/.pi/agent/models.json`. Supports custom base URLs, API keys, and adapter types via `genai`.
- **Rich system prompt** — toggle and customize preamble, workspace tree, coding rules, tool descriptions, file paths, and current date. The default preamble (`assets/preamble.md`) sets clear coding and safety rules.
- **Six built-in tools** — `read`, `write`, `edit`, `find` (glob), `search` (regex), and `bash` (shell), all individually toggleable. Tools run natively in Rust — no subprocess overhead (except bash).
- **Work modes** — `Plan`, `Code`, and `Review` modes adjust the agent's behavior.
- **Real-time streaming** — responses stream progressively via `genai`'s async chat stream, with live text, reasoning, and tool-call display.
- **Reasoning / thinking** — toggle thinking mode on supported models, with configurable reasoning effort levels.
- **Markdown rendering** — all chat messages are rendered as Markdown in the conversation pane.
- **Tool result display** — tool arguments shown in a table; results collapsed by default for cleaner conversation view.
- **Token usage** — per-response token counts displayed in the right pane.
- **Session management** — each session saved as a JSON file in `.agent/sessions/` inside your workspace. Create new sessions at any time.
- **Session header** — shows the last-sent prompt with Copy and Resend buttons.
- **Persistent state** — window layout, model selection, enabled tools, work mode, recent workspaces, and system prompt settings are restored on restart.
- **Cross-platform paths** — workspace paths and tool outputs use Unix-style representation everywhere, with automatic Windows ↔ Unix conversion.
- **Pure Rust, single binary** — zero runtime dependencies, no GC pauses, minimal footprint.

## What's Inside

| File                 | Purpose                                                     |
| -------------------- | ----------------------------------------------------------- |
| `src/main.rs`        | Application entry point, GUI layout, message handling       |
| `src/adk.rs`         | LLM client builder, streaming, tool-call loop (genai)       |
| `src/chat.rs`        | Display message types, Markdown caching                     |
| `src/model.rs`       | Provider/model config loading from OMP & PI formats         |
| `src/session.rs`     | Session create / save / load / list                         |
| `src/settings.rs`    | Persistent settings save/restore                            |
| `src/system.rs`      | System prompt panel: preamble, rules, files, workspace tree |
| `src/tool.rs`        | Dev tools toggle panel and summary                          |
| `src/user.rs`        | User prompt editor and work mode picker                     |
| `src/workspace.rs`   | Workspace directory tree scanner                            |
| `src/tools/*.rs`     | Six built-in tool implementations                           |
| `assets/preamble.md` | Default preamble with coding rules and safety guidelines    |

---

**Full Changelog**: [`initial commit...v0.1.0`](https://github.com/J-F-Liu/crabot/commits/v0.1.0)
