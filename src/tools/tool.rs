//! The [`Tool`] trait, its type aliases, and the constants shared by tool
//! implementations and the LLM loop.

use genai::chat::Tool as GenaiTool;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::schema::make_strict_schema;

/// Coalesce small chunks until this many bytes accumulate before flushing.
pub const COALESCE_BYTES: usize = 4 * 1024;
/// Coalesce small chunks for at most this long before flushing.
pub const COALESCE_MS: Duration = Duration::from_millis(100);

/// User-cancel reason shared by tools and the LLM loop.
pub const CANCEL_REASON: &str = "Cancelled by user";

/// How long timeout/cancel errors wait for a detached host command's final
/// drain (the forwarder lock) before reporting without partial output.
pub(crate) const CAPTURE_GRACE: Duration = Duration::from_secs(2);

/// Stdin chunk size for [`write_stdin_bounded`](super::write_stdin_bounded):
/// a cancelled/timed-out input stops within one chunk.
pub(crate) const STDIN_CHUNK: usize = 16 * 1024;
/// Sleep between `WouldBlock` retries on non-blocking stdin pipes.
pub(crate) const STDIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub type ToolRef = Arc<dyn Tool>;

/// Sink for incremental tool output (e.g. bash stdout/stderr chunks).
pub type OutputSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Trait implemented by every tool (built-in or custom).
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn instruction(&self) -> &str;
    fn schema(&self) -> Value;

    /// Cancel-aware wrapper: checks the token *before* delegating to
    /// [`execute_inner`](Self::execute_inner); tools may also poll it while running.
    fn execute(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        self.execute_inner(args, workspace, cancel)
    }

    /// Implement this instead of [`execute`](Self::execute) — the default
    /// `execute` wrapper already handles the pre-execution cancel check.
    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String>;

    /// Streaming variant of [`execute`](Self::execute): the same cancel-aware
    /// wrapper, but live-output tools forward incremental chunks to `sink`.
    /// The default ignores `sink` and behaves like `execute`. Chunks are raw
    /// text (UTF-8, newline normalized); the final result stays authoritative.
    fn execute_streaming(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
        sink: &OutputSink,
    ) -> Result<String, String> {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        self.execute_streaming_inner(args, workspace, cancel, sink)
    }

    /// Implement this instead of [`execute_streaming`](Self::execute_streaming)
    /// to stream output live; the default ignores `sink` and falls back to
    /// [`execute_inner`](Self::execute_inner).
    fn execute_streaming_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
        _sink: &OutputSink,
    ) -> Result<String, String> {
        self.execute_inner(args, workspace, cancel)
    }

    /// Full tool declaration suitable for genai ChatRequest.
    fn tool_declaration(&self, strict: bool) -> GenaiTool {
        let mut schema = self.schema();
        if strict {
            make_strict_schema(&mut schema);
        }
        let tool = GenaiTool::new(self.name())
            .with_description(self.description())
            .with_schema(schema);
        if strict { tool.with_strict(true) } else { tool }
    }
}
