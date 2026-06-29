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
