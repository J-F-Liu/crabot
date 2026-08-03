# Crabot

[![Crates.io](https://img.shields.io/crates/v/crabot.svg)](https://crates.io/crates/crabot)
[![Downloads](https://img.shields.io/crates/d/crabot.svg)](https://crates.io/crates/crabot)
[![CI](https://github.com/J-F-Liu/crabot/actions/workflows/rust.yml/badge.svg)](https://github.com/J-F-Liu/crabot/actions/workflows/rust.yml)

<p align="center">
  <img src="assets/images/logo.png" alt="Crabot Logo" width="200">
</p>

A pure-Rust native GUI coding agent using [iced](https://iced.rs) and [genai](https://crates.io/crates/genai) for multi-provider LLM.

## Highlights

- [x] No TUI — just a GUI, easy for everyone to use.
- [x] Multi-tab sessions view — switch between concurrent sessions, or let the agent delegate subtasks to new tabs.
- [x] Configure through dialogs — a settings dialog with 7 tab pages (AI Models, Prompt Recipes, Builtin Tools, Custom Tools, MCP Servers, Tool Playground, About), no need to write config files by hand.
- [x] An explicit context window, with every detail customizable.
- [x] Native, high-performance built-in tools.
- [x] Custom CLI tools and MCP server tools, defined and managed in-app.
- [x] Built in pure Rust — single native binary, no runtime dependency, zero GC pauses.
- [x] Each session is saved as a json file in workspace `.agent/sessions` folder.

If you know the structure of the LLM context window, you will appreciate the UI design of crabot.
<p align="center">
<img src="doc/images/Context%20Window%20Components.webp" alt="Context Window Components" width="600">
<img src="doc/images/screenshot.webp" alt="screen shot" width="800">
</p>

## Built-in Tools

| Tool     | Params                                     | Description                                                                                     |
| -------- | ------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `read`   | `path`, `offset`, `limit`                  | Read any file in your workspace, with smart truncation for large files                          |
| `write`  | `path`, `content`                          | Create or overwrite files, creating missing parent directories automatically                    |
| `edit`   | `path`, `edits` [{`old_text`, `new_text`}] | Make precise text replacements in existing files, with conflict detection for overlapping edits |
| `find`   | `pattern`, `path`                          | Locate files by name pattern, skipping ignored files (`.gitignore`)                             |
| `search` | `pattern`, `path`                          | Search file contents with regular expressions, skipping ignored files (`.gitignore`)            |
| `bash`   | `command`, `timeout`                       | Run shell commands in your workspace, with a timeout and instant cancellation                   |
| `ask`    | `question`, `options`                      | Pause and ask you a question when it needs your input or approval                               |
| `todo`   | `items` [{`text`, `depth`, `status`}]      | Track its task list, shown live in the right pane                                               |
| `task`   | `title`, `prompt`, `mode`, `difficulty`    | Delegate a subtask to a separate session and continue once the final report comes back          |
| `renew`  | `prompt`                                   | Hand off to a fresh session seeded with a summary when the context window is nearly full        |
| `fetch`  | `url`, `format`                            | Download web pages and convert them to clean Markdown                                           |

Beyond the built-ins, you can add your own **custom CLI tools** and connect **MCP servers** (Stdio or HTTP) to expose their tools — everything is managed in-app and toggleable per session.

## Installation

### Download pre-built binaries

Download the latest release from [GitHub Releases](https://github.com/J-F-Liu/crabot/releases/latest).

Choose the archive matching your platform:

- **Linux** (x86_64): `crabot-linux-x86_64.tar.gz`
- **macOS** (x86_64): `crabot-macos-x86_64.tar.gz`
- **macOS** (ARM64): `crabot-macos-aarch64.tar.gz`
- **Windows** (x86_64): `crabot-windows-x86_64.zip`

Extract and place the binary in your `PATH`.

### From crates.io

```sh
cargo install crabot
```

### From latest source

```sh
cargo install --git https://github.com/J-F-Liu/crabot
```
