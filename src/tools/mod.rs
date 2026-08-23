//! Tool infrastructure: the [`Tool`] trait, the [`ToolRegistry`], shared
//! helpers, and the built-in/custom/MCP tool implementations.
//!
//! The implementation lives in per-concern submodules; this file re-exports
//! their items at the legacy `crate::tools::<item>` paths so existing
//! consumers (including the builtin tools' `super::` references) stay intact.

/// In-process bashkit interpreter; `pub` for `tests/bash.rs`.
pub mod bash_kit;
mod charset;
pub mod custom;
pub mod mcp;

// ── Submodules ─────────────────────────────────────────────────────
mod capture;
mod context;
mod exec;
mod limits;
mod paths;
mod registry;
mod schema;
mod tool;

/// One file per tool; `super::` inside them resolves via this module's re-exports.
pub mod builtin;
/// Re-export the tools at the legacy `crate::tools::<tool>` paths.
pub use builtin::{ask, bash, edit, fetch, find, process, read, renew, search, task, todo, write};

pub use renew::move_renews_to_end;
pub use task::{TASK_MODES, TaskRequest, task_request_from_call};

pub(crate) use charset::{StreamDecoder, decode_bytes};
pub use context::{current_tab_number, with_tab_scope};

// ── tool ────────────────────────────────────────────────────────────
pub(crate) use tool::CAPTURE_GRACE;
pub use tool::{CANCEL_REASON, COALESCE_BYTES, COALESCE_MS, OutputSink, Tool, ToolRef};

// ── schema ──────────────────────────────────────────────────────────
pub use schema::{decode_stringified_args, make_strict_schema};

// ── registry ────────────────────────────────────────────────────────
pub use registry::{ToolRegistry, build_tools, unknown_tool_message};

// ── paths ───────────────────────────────────────────────────────────
pub use paths::{
    arg_path, convert_path_to_unix_style, normalize_newlines, resolve_path, resolve_path_partial,
    tmp_host_dir,
};
pub(crate) use paths::{arg_str, arg_u64, make_workspace_relative};
#[cfg(windows)]
pub(crate) use paths::{convert_path_to_windows_style, drive_style_to_windows};

// ── limits ──────────────────────────────────────────────────────────
pub(crate) use limits::truncate_output;
pub use limits::{
    StreamingCap, ToolLimits, capping_sink, init_tool_limits, streaming_truncation_marker,
    tool_limits,
};

// ── exec ────────────────────────────────────────────────────────────
#[cfg(windows)]
pub(crate) use exec::peek_pipe_available;
pub(crate) use exec::{
    ProcessSignal, StdinWriteError, combine_output, create_pipe_pair, detach_child, exit_code_of,
    format_command_output, is_secret_env_key, pipe_to_stdio, sanitize_child_env,
    set_pipe_nonblocking, set_sender_noninheritable, signal_process_tree, write_stdin_bounded,
};

// ── capture ─────────────────────────────────────────────────────────
pub use capture::{ChunkForwarder, OutStream};
pub(crate) use capture::{WaitError, timeout_message, try_lock_for, wait_with_timeout};
