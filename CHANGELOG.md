# Crabot v0.8.4

- **Fresh workspace file tree at send time** — the workspace files tree offered to the model is rebuilt when a message is sent, instead of relying on a stale startup snapshot.
- **Preserve token counts** — token usage recorded in the session is kept when an LLM response is missing usage data, so context tracking no longer regresses.
- **More reliable `fetch` tool** — large or slow web pages are fetched more robustly.
- **Resilient model listing** — servers that misbehave when listing models no longer break the model picker.
- **Background HTML export** — session HTML export renders and writes on a background thread, keeping the UI responsive.
- **`ask` focus fix** — switching to a tab focuses the ask input only when an ask is actually pending.
- **Playground & search bar fixes** — the playground preserves raw array item text while typing (no premature reformatting), and the search bar discards stale layout measurements.
- **Custom tool argument safety** — argument values are rendered via internal placeholders substituted after shell-splitting, preventing argv injection and preserving values containing spaces or quotes as single arguments.
- **Fix: "Dark theme" toggle button** — the toggle works again.

**Full Changelog**: [`v0.8.3...v0.8.4`](https://github.com/J-F-Liu/crabot/compare/v0.8.3...v0.8.4)

# Crabot v0.8.3

- **Right pane overhaul** — the right pane is now organized into collapsible sections, with a new **Accessed Files** list alongside file snapshots, and a **Running Processes** section showing live processes started via the `process` tool.
- **Session header popup menu** — header actions are consolidated into a "…" popup menu, which gains two new items: **Fork session** (branch the conversation into a new session) and **Compact session** (condense history to free context window).
- **`ask` tool: custom answers** — beyond picking an option, users can now type a free-form answer and submit it with an "Enter my answer" button.
- **Search bar matches headers** — search now also covers dialog and turn headers, not just message bodies.
- **Inline model parameter editor** — checked models in the AI Models settings dialog can be tuned inline (temperature, max tokens, …) without opening the per-model dialog.
- **Retry empty LLM responses** — empty assistant responses are retried like transient failures instead of ending the turn.
- **Shared `/tmp` across tools** — the `bash` tool and file tools resolve `/tmp` to the same host directory, so temp files written by one are visible to the other.
- **System prompt saved in sessions** — the system prompt is recorded into the session JSONL file for faithful reloads.
- **bashkit & genai updates** — dependency refresh.
- **DeepSeek peak-hour pricing on weekdays only** — the doubled pricing now applies Monday–Friday only.
- **Snapshot cleanup on exit** — `.agent/snapshots` are cleaned up when the app exits.

**Full Changelog**: [`v0.8.2...v0.8.3`](https://github.com/J-F-Liu/crabot/compare/v0.8.2...v0.8.3)

# Crabot v0.8.2

- **Per-message session persistence** — session files are now saved incrementally after every complete message (assistant reply, tool results, injected user prompts) instead of only when the whole turn finishes, so a crash or force-kill mid-task no longer loses the conversation recorded so far.
- **Retry covers mid-stream failures & stalls** — transient 429 / 5xx / connection errors that strike after streaming has started, and stalled streams, are now retried with an on-screen countdown (up to 5 attempts) instead of ending the turn.
- **Panic details are logged** — a crash now appends the panic message and backtrace to `~/.crabot/logs/panic.log`, keeping GUI crashes diagnosable when stderr is hidden.

- **`bash` tool: real identity & platform variables** — `whoami`/`hostname`/`uname -n` now report the actual host user and machine via bashkit's `.username()`/`.hostname()`, and `OSTYPE`/`MACHTYPE`/`HOSTTYPE` are seeded with real values (`msys`, `x86_64-pc-msys`, … on Windows), so scripts probing the platform see the true host instead of bashkit's virtual defaults.
- **`bash` tool: curl/wget with open network** — the bashkit `http_client` feature is now enabled, so `curl`/`wget`/`http` work inside the in-process interpreter with the same open policy as the `fetch` tool (any http(s) destination, private IPs included) and raised limits: 600s timeout (bashkit's cap) and a 64 MB response cap.
- **`bash` tool: host Python preferred** — `python`/`python3` now bridge to the host interpreter when one is on PATH; the embedded Monty interpreter remains the fallback.
- **MSYS2-style argument conversion for bridged commands (Windows)** — VFS absolute paths in host command arguments are now rewritten to native paths before spawning (`git -C /d/... status` works with native git), mirroring MSYS2's automatic path conversion; covers standalone paths, `--opt=/path` forms, `/tmp` mounts, and UNC `//server/share`.

- **Built-in `process` tool** — start, monitor, interact with, and stop long-running processes (servers, watchers, REPLs). Processes are addressed by their OS pid and launched directly via `shell-words` (no platform shell), mirroring the `bash` tool's host-command bridge.

- **Throttled streaming tool output** — live tool output is now coalesced into at most ~10 UI updates per second (or one per 4 KB), so a noisy process (e.g. `cargo test` linking) can no longer flood the UI thread and freeze the window; forwarded output is additionally cut at `max_output_bytes` with a truncation marker, and only the tail window is laid out while a tool is still running, bounding memory and per-frame render cost.
- **Responsive UI under load** — high-frequency cursor-move events are deduplicated, keeping the UI responsive while a heavy stream or tool output is rendering.
- **Smart-case `find` patterns** — glob matching is now case-insensitive by default (`*.rs` matches `FOO.RS`); a pattern containing an uppercase character switches to exact-case matching (`*.RS` matches only uppercase extensions).
- **`search` tool uses ripgrep's engine** — big files, non-UTF-8, and binary files are handled gracefully via streaming scans with binary detection and lossy UTF-8 fallback, instead of whole-file string reads.

- **Taskbar attention for background sessions** — a finished background session flashes the taskbar once (Windows), and an unanswered `ask` keeps flashing until the window regains focus.
- **Accurate DeepSeek pricing** — cost tracking now reflects DeepSeek pricing, which doubles during Beijing peak hours.
- **Update download progress** — the self-update flow now shows streaming download progress while fetching the new release.

**Full Changelog**: [`v0.8.1...v0.8.2`](https://github.com/J-F-Liu/crabot/compare/v0.8.1...v0.8.2)

# Crabot v0.8.1

- **Export session as HTML** — a new download icon in the session header saves the current session as a standalone HTML file and opens it in the browser. The page is script-free with a strict Content-Security-Policy.
- **Persistent logging** — the app now writes daily-rolling logs to `~/.crabot/logs` covering LLM retries/failures, tool executions, session/MCP events, and settings errors; debug builds mirror to stderr. Filter with `RUST_LOG`.
- **Instant, race-free Stop** — cancellation is now event-driven, so Stop takes effect immediately (previously polled with up to 100 ms delay).
- **Bash timeout/cancel keeps partial output** — output produced before a timeout or cancel is now kept in the error message.
- **Fix: bash panic on small max timeout** — a `bash max timeout (ms)` below 1000 no longer panics; limits are sanitized on load.
- **LLM stream stall detection** — a silent stream now fails with a clear error after a configurable timeout (Settings → Builtin Tools, 0 disables) instead of hanging forever; Anthropic heartbeats keep long thinking alive.
- **genai 0.7.0-beta.18** — new AtlasCloud / Qwen Cloud / Kimi providers, SSE heartbeat events, `ReasoningEffort::Zero`, tool prompt caching.
- **bashkit 0.16.0** — byte-native stream handling; the `yaml` builtin is removed (superseded by `yq`).
- **Wrapper-aware bash analysis** — the interpreter now sees through `timeout`/`xargs`/`find -exec` wrappers and runs the wrapped commands in-process; opaque scripts still fall back to real `bash`.
- **Windows: all drives mounted read-only in the `bash` tool** — builtins can read any drive at `/c`, `/d`, … while writes stay confined to workspace, home, and `/tmp`.

# Crabot v0.8.0

- **In-process `bash` tool** — the `bash` tool now runs inside Crabot's own embedded bash interpreter (bashkit) instead of spawning a real bash process, so shell commands work natively on Windows with no bash installed. External commands (cargo/git/…) are bridged to host executables through the sandboxed virtual filesystem, and scripts the interpreter cannot faithfully handle (parse errors, dynamic command names, `eval`/`exec`/`source`) automatically fall back to real `bash -c`.
- **Embedded Python in `bash` tool** — the `python`/`python3` commands now dispatch to bashkit's in-process Monty interpreter (enabled via the bashkit `python` feature): no host Python needed, and `open()`/`pathlib.Path` I/O is bridged to the sandboxed virtual filesystem.
- **Streaming `bash` tool output** — bash commands now stream their output into the tool message live (like LLM text chunks) instead of appearing all at once when the command finishes: the tool row shows a growing `Running…` buffer, then the final result replaces it in place. External commands (cargo/git/…) stream straight from the pipe drains; builtin-only scripts stream per command. Live output is capped at the configured `max_output_bytes` so runaway output cannot flood the UI, and `\r\n` is normalized to `\n` while streaming.
- **Parallel tool calls & background tabs** — independent tool calls from a single assistant response now execute in parallel (results arrive in completion order), and sessions spawned by the `task` tool open in background tabs so the parent session keeps the view.
- **Incremental JSONL session persistence** — sessions are now stored as `{id}.jsonl` with incremental appends instead of full rewrites. The first line holds session metadata (enabling fast session-list scanning), subsequent lines contain raw history messages, and a `Tally` record captures the cumulative usage snapshot. Legacy `.json` sessions are still loadable and migrate transparently on the next save. A new `examples/jsonl_to_json.rs` converts a session file into a whole-document JSON.
- **Per-session file snapshots** — every file modified by the agent is snapshotted per session, and the right pane gains Revert / Revert All actions that restore the original contents in the background.
- **Auto-retry transient LLM errors** — 429 / 5xx / connection failures are retried automatically with an on-screen countdown, so brief provider hiccups no longer interrupt a run.
- **Ask tool prompt improvements** — ask prompts show a live countdown with an "Extend +5 min" button, a new "None apply" button answers "None of the options apply.", and "Skip" is renamed "You decide" ("No preference. Use your best judgment.").
- **Clickable bare URLs** — URLs that appear in messages without Markdown link syntax are now rendered as clickable links, opened with Ctrl+Click.
- **Parent session tracking** — sessions spawned by the `renew`/`task` tools show their spawning parent in the header: the parent's tab label when it is still open, else the parent session id.
- **Last response time** — the right pane now shows when the last assistant response was received.
- **Configurable renew threshold** — the Builtin Tools settings tab adds a context-fill threshold that triggers the `renew` handoff, and the tool-limits layout is now two columns.
- **Faster startup** — the workspace file tree and AGENTS.md scan asynchronously on startup.
- **Update check from GitHub Releases** — version updates are now checked against GitHub releases instead of crates.io.
- **Session picker refinements** — re-selecting an already-open session resets it to the collapsed dialog overview, and pressing Enter in the search bar advances to the next match like the ▼ button.
- **Clearer edit tool errors** — the edit tool now collects all argument errors (including file-read failures) before reporting, so a failed edit produces one complete message.
- **Documentation refresh** — the README now documents every keyboard shortcut, mouse gesture, and dropdown key.
- **Fix: ask tool result lost on Stop** — clicking Stop while an `ask` tool call was pending no longer leaves the call unmatched in session history; the call is resolved with a "Cancelled by user." result that stays paired with it on save and reload.

**Full Changelog**: [`v0.7.0...v0.8.0`](https://github.com/J-F-Liu/crabot/compare/v0.7.0...v0.8.0)

# Crabot v0.7.0

- **Multi-tab session management** — run several sessions in parallel, each in its own tab. Create, close, and switch tabs freely; every session keeps its own conversation, model, and workspace.
- **Built-in `task` tool** — delegate self-contained subtasks to a new isolated agent session that works autonomously in its own tab. Pick a mode (Explore / Plan / Code / Review / Test) and a difficulty tier (Easy / Medium / Hard) that selects the configured sub-agent model; the final report comes back as the tool result. Sub-agents can even spawn further nested subtasks.
- **Built-in `renew` tool** — when the context window is nearly full, hand off to a fresh session seeded with a condensed summary of the remaining work, so long-running tasks continue seamlessly.
- **Per-session model memory** — each session tab remembers its selected model and restores it when switching tabs or loading a session.
- **Persisted context window** — the context window size from the model configuration is now persisted and displayed per session.
- **Session status indicators** — the session tab bar and headers show live status (streaming, thinking, tool executing, idle) with clearer session headers.
- **Tab keyboard shortcuts** — Ctrl+1–9 jump to the Nth conversation tab (Ctrl+0 to the last), Ctrl+N opens a new session tab, and Ctrl+W closes the current one. Duplicate session IDs are prevented.
- **Tab bar scroll arrows** — when many tabs overflow, press-and-hold arrows scroll the session tab bar.
- **Work-mode badges** — each dialog header now displays its work mode as a badge.
- **Always-visible Restart button** — the Restart button is always shown and now relaunches Crabot correctly.
- **Selectable plain-text responses** — plain-text LLM replies skip Markdown rendering, so the text can be directly selected and copied.
- **Builtin Tools settings page** — a new settings tab for the agent loop: max iterations, 12 user-configurable tool limits (command/output caps, line/byte budgets, fetch/MCP timeouts), and per-tier sub-agent models for the `task` tool.
- **Error envelope for tool failures** — failed tool executions are wrapped in a consistent `Error:` envelope, so error states survive session save/load.
- **Edit tool precision** — duplicate/overlap edit failures now report line numbers instead of byte offsets, and parameter validation errors are clearer.
- **Per-tab preamble & workspace** — the left pane shows the active session tab's selected preamble, and the workspace tree syncs to the active session when switching tabs.
- **Per-mode preambles** — the default preamble is split into per-work-mode files (`coding`, `explore`, `planning`, `review`, `testing`, `crabot`) to back the new task-tool modes.
- **Documentation refresh** — README and AGENTS.md updated to reflect the new architecture and features.

**Full Changelog**: [`v0.6.1...v0.7.0`](https://github.com/J-F-Liu/crabot/compare/v0.6.1...v0.7.0)

# Crabot v0.6.0

- **Dark theme** — a new dark color scheme, toggleable with a single button at the top of the right pane. The app now starts in the system's preferred color scheme.
- **Settings dialog overhaul** — the settings dialog now has a left sidebar with five tab pages: **AI Models**, **Prompt Recipes**, **Custom Tools**, **MCP Servers**, and **Tool Playground**. Ok/Cancel buttons make edits atomic and discardable.
- **AI Models settings page** — manage model configs entirely in-app. Embedded `models.json` (~500 models) provides context windows, pricing, and aliases. No more loading from external OMP/PI config files.
- **Tool Playground** — test any tool (built-in, custom, or MCP) with arbitrary JSON arguments directly in the settings dialog. No LLM needed — perfect for debugging tool descriptions and schemas. Todo tool results render in right pane.
- **Prompt Recipes settings page** — create, edit, and manage prompt recipe templates in-app. Recipes are per-work-mode (Plan/Code/Review) and selectable from a dropdown under user prompt inputbox.
- **Custom Tools settings page** — define and manage custom CLI tools with TinyTemplate commands and typed JSON Schema parameters, all through the GUI.
- **MCP Servers settings page** — configure MCP servers (Stdio or HTTP transport), set environment variables, and manage prompts — entirely in-app.
- **One-click self-update** — Crabot checks GitHub releases and can download and replace its own binary with a single click. Always stay current without leaving the app.
- **Session-level cost display** — the right pane now shows a cumulative breakdown: total input/output tokens, request count, and cost in your model's configured currency.
- **Sessions grouped by month** — the session picker dropdown now groups sessions by year-month, and session files are stored in `YYYY-MM` subdirectories for better organization.
- **Ctrl+E expand/collapse all** — toggle all turn dialogs in the current session with a single shortcut.
- **Keyboard scrolling shortcuts** — Home, End, PageUp, PageDown, Up, Down, and Space now scroll the conversation view.
- **Ctrl+Click to open URLs** — URLs in assistant responses can be opened directly in your browser with Ctrl+Click.
- **Atomic settings with Save/Close** — settings changes are only persisted when you click Save; Close discards all edits.
- **Session forking on model switch** — changing the model before sending or resending now forks the session, preserving the original conversation.
- **DeepSeek cross-model resend** — chat history from other models can now be resent to DeepSeek models without compatibility issues.
- **Better edit tool errors** — edit tool parameter validation now produces clearer, more actionable error messages.
- **Consistent user agent** — the `fetch` tool and update checker now use a versioned `Crabot vX.X.Z` user agent string.
- **Major internal refactoring** — the codebase has been restructured to follow iced GUI patterns more closely, with 6 domain-specific state groups and hierarchical message routing. Settings split into 5 dedicated tab modules.
- **Workspace file list in user prompt** — the workspace tree is now injected into the user prompt rather than the system prompt, improving cache hit rates when the workspace changes.
- **Model settings dialog** — configure model parameters (temperature, max tokens, thinking mode) through an in-app dialog rather than editing config files.
- **About tab** — version info, repository link, and credits are now shown in a dedicated About tab within the settings dialog.

**Full Changelog**: [`v0.5.0...v0.6.0`](https://github.com/J-F-Liu/crabot/compare/v0.5.0...v0.6.0)

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
