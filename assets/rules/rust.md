## Requirements for Working with Rust Projects
- Use `cargo add` to add dependencies or enable features rather than editing `Cargo.toml` directly.
- Use `cargo doc --no-deps --document-private-items` to inspect APIs if usage is unclear. Never pass the `--open` flag.
- Before completing your task, whenever you modify Rust code:
    1. Run `cargo check` and resolve any compilation errors
    2. Run `cargo clippy` and fix all relevant warnings and lint issues
    3. Run `cargo fmt` to apply standard Rust formatting
  If the change is small run `cargo check && cargo clippy && cargo fmt` in one command.
