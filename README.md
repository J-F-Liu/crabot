# Crabot

[![Crates.io](https://img.shields.io/crates/v/crabot.svg)](https://crates.io/crates/crabot)
[![Downloads](https://img.shields.io/crates/d/crabot.svg)](https://crates.io/crates/crabot)
[![CI](https://github.com/J-F-Liu/crabot/actions/workflows/rust.yml/badge.svg)](https://github.com/J-F-Liu/crabot/actions/workflows/rust.yml)

<p align="center">
  <img src="assets/images/logo.png" alt="Crabot Logo" width="200">
</p>

A smart and powerful coding agent.

## Highlights

- [x] No TUI — just a GUI, easy for everyone to use.
- [ ] Muti-tab sessions view.
- [x] Configure through dialogs — no need to write config files by hand.
- [x] An explicit context window, with every detail customizable.
- [x] Native, high-performance built-in tools.
- [x] Built in pure Rust — single native binary, no runtime dependency, zero GC pauses.
- [x] Each session is saved as a json file in workspace `.agent/sessions` folder.

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
