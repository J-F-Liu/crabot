//! Process-wide tool limits, output truncation, and live-stream capping.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, RwLock};

use super::tool::OutputSink;
use crate::lock;

/// Configurable limits for the built-in tools.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolLimits {
    /// `bash`: default timeout (ms) when no explicit `timeout` argument is given.
    pub command_timeout_ms: u64,
    /// `bash`: hard maximum timeout (ms) for a single command.
    pub max_command_timeout_ms: u64,
    /// Bytes kept from head and tail when truncating oversized tool output.
    pub head_tail_bytes: usize,
    /// Maximum output bytes for tool results before truncation.
    pub max_output_bytes: usize,
    /// `read`: default and maximum lines per call.
    pub read_max_lines: usize,
    /// `read`: byte budget per call.
    pub read_max_bytes: usize,
    /// `find`: maximum result lines.
    pub find_max_lines: usize,
    /// `search`: maximum result lines.
    pub search_max_lines: usize,
    /// `fetch`: maximum downloaded body bytes.
    pub fetch_max_body_bytes: usize,
    /// `fetch`: HTTP request timeout (ms).
    pub fetch_timeout_ms: u64,
    /// `mcp`: connection (handshake) timeout (ms).
    pub mcp_connect_timeout_ms: u64,
    /// `mcp`: single tool-call timeout (ms).
    pub mcp_call_timeout_ms: u64,
}

impl ToolLimits {
    /// Built-in default limits.
    pub const fn new() -> Self {
        Self {
            command_timeout_ms: 120_000,     // 2 minutes
            max_command_timeout_ms: 600_000, // 10 minutes
            head_tail_bytes: 3 * 1024,       // 3 KB each
            max_output_bytes: 100 * 1024,    // 100 KB
            read_max_lines: 2000,
            read_max_bytes: 64 * 1024, // 64 KB
            find_max_lines: 100,
            search_max_lines: 500,
            fetch_max_body_bytes: 8 * 1024 * 1024, // 8 MB
            fetch_timeout_ms: 60_000,              // 1 minute
            mcp_connect_timeout_ms: 60_000,        // 1 minute
            mcp_call_timeout_ms: 300_000,          // 5 minutes
        }
    }

    /// Keep both timeout fields in the valid `1000..=max` range; the bash
    /// tool's `clamp` panics and its schema breaks when `max < 1000`.
    pub fn sanitize(&mut self) {
        self.max_command_timeout_ms = self.max_command_timeout_ms.max(1000);
        self.command_timeout_ms = self
            .command_timeout_ms
            .clamp(1000, self.max_command_timeout_ms);
    }
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide tool limits, applied from settings at startup and replaceable
/// at runtime (e.g. from the settings dialog).  `RwLock::read()` is
/// near-zero-cost on the uncontended fast-path, so the hot read path in tool
/// executions stays cheap while writes remain rare.
static TOOL_LIMITS: RwLock<ToolLimits> = RwLock::new(ToolLimits::new());

/// Apply tool limits from settings; invalid values are sanitized first.
pub fn init_tool_limits(mut limits: ToolLimits) {
    limits.sanitize();
    if let Ok(mut guard) = TOOL_LIMITS.write() {
        *guard = limits;
    }
}

/// The current effective tool limits (defaults until first `init` call).
pub fn tool_limits() -> ToolLimits {
    TOOL_LIMITS.read().map(|g| *g).unwrap_or_default()
}

/// Truncate output that exceeds the configured maximum, keeping head and tail.
pub(crate) fn truncate_output(s: String) -> String {
    let limits = tool_limits();
    let max = limits.max_output_bytes.max(1);
    if s.len() <= max {
        return s;
    }

    let total = s.len();
    // Shrink head/tail proportionally so tiny caps never underflow or overlap.
    let head_tail = limits.head_tail_bytes.min(max / 2);
    let skipped = total - head_tail * 2;

    let head_end = s.floor_char_boundary(head_tail);
    let tail_start = s.floor_char_boundary(total - head_tail);

    let head = &s[..head_end];
    let tail = &s[tail_start..];

    let mut truncated = String::with_capacity(head_tail * 2 + 128);
    truncated.push_str(head);
    truncated.push_str(&crate::truncation_marker(
        skipped,
        total,
        Some(("max", max)),
    ));
    truncated.push_str(tail);
    truncated
}

/// Marker for live output cut at `cap`; the final result replaces the view on finish.
pub fn streaming_truncation_marker(cap: usize) -> String {
    format!(
        "\n\n… [streaming output truncated at {cap} bytes — the final result includes the newest output] …\n"
    )
}

/// Cap a live chunk stream: forward until `cap` bytes, then emit
/// [`streaming_truncation_marker`] once and mute. A stream ending exactly at
/// `cap` gets no marker. Cuts on UTF-8 boundaries.
pub struct StreamingCap {
    cap: usize,
    sent: usize,
    cut: bool,
}

impl StreamingCap {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            sent: 0,
            cut: false,
        }
    }

    /// The chunk to forward, or `None` once the stream has been cut.
    pub fn push(&mut self, chunk: &str) -> Option<String> {
        if self.cut {
            return None;
        }
        let room = self.cap - self.sent;
        if chunk.len() <= room {
            self.sent += chunk.len();
            return Some(chunk.to_string());
        }
        // Straddles the cap: keep the first `room` bytes, mark the cut, mute.
        let mut out = chunk[..chunk.floor_char_boundary(room)].to_string();
        out.push_str(&streaming_truncation_marker(self.cap));
        self.cut = true;
        Some(out)
    }
}

/// [`OutputSink`] forwarding through a [`StreamingCap`] at `max_output_bytes`
/// — the single live-output cap shared by every streaming tool.
pub fn capping_sink(forward: impl Fn(String) + Send + Sync + 'static) -> OutputSink {
    let cap = Mutex::new(StreamingCap::new(tool_limits().max_output_bytes));
    Arc::new(move |chunk| {
        let Some(out) = lock(&cap).push(chunk) else {
            return;
        };
        forward(out);
    })
}
