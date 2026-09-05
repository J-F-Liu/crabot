// Deadlock trap: a mutex guard in an `if let` scrutinee is held for the whole branch.
#![warn(clippy::significant_drop_in_scrutinee)]

pub mod chat;
pub mod i18n;
pub mod model;
pub mod model_database;
pub mod session;
pub mod settings;
pub mod setup;
pub mod tools;
pub mod user;
pub mod workspace;

use std::collections::HashSet;
use std::hash::Hash;
use std::sync::{Mutex, MutexGuard};

pub fn app_title() -> &'static str {
    concat!("Crabot v", env!("CARGO_PKG_VERSION"))
}

pub trait HashSetExt<T> {
    fn set(&mut self, value: T, enabled: bool);
}

impl<T: Eq + Hash> HashSetExt<T> for HashSet<T> {
    fn set(&mut self, value: T, enabled: bool) {
        if enabled {
            self.insert(value);
        } else {
            self.remove(&value);
        }
    }
}

/// Lock a mutex, recovering the payload if the holder panicked.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stream yielding one tick on subscribe plus one tick per broadcast ping on
/// `sender`, so a subscriber stays consistent even if it missed a ping while
/// unsubscribed. Lagged pings coalesce. Safe to subscribe permanently.
pub fn broadcast_ticks(
    sender: &tokio::sync::broadcast::Sender<()>,
) -> futures::stream::BoxStream<'static, ()> {
    use futures::StreamExt;
    use tokio::sync::broadcast::error::RecvError;
    let changes = futures::stream::unfold(sender.subscribe(), |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(()) => return Some(((), rx)),
                // Coalesce lagged pings instead of ending the stream.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    });
    futures::stream::iter([()]).chain(changes).boxed()
}

// ── Bounded output capture ─────────────────────────────────────────

/// Retains the first and last `keep` bytes of a stream and counts the total,
/// so runaway output (e.g. `yes`) costs at most `2 * keep` bytes of memory.
/// The stream is reconstructed losslessly whenever it fits within `2 * keep`
/// bytes.
pub struct BoundedCapture {
    keep: usize,
    /// First `keep` bytes of the stream.
    head: Vec<u8>,
    /// Rolling window of the last `keep` bytes of the stream.
    tail: Vec<u8>,
    /// True total bytes pushed, kept or discarded.
    total: usize,
}

impl BoundedCapture {
    pub fn new(keep: usize) -> Self {
        Self {
            keep,
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len();
        let room = self.keep.saturating_sub(self.head.len());
        self.head.extend_from_slice(&chunk[..chunk.len().min(room)]);
        self.tail.extend_from_slice(chunk);
        let excess = self.tail.len().saturating_sub(self.keep);
        self.tail.drain(..excess);
    }

    /// Total bytes pushed so far, kept or discarded.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Bytes discarded because the stream exceeded `2 * keep`. `0` when the
    /// stream is still fully retained.
    pub fn skipped(&self) -> usize {
        self.total.saturating_sub(2 * self.keep)
    }

    /// head + tail with the overlap dropped, no truncation marker. Used for
    /// lossless reconstruction and partial-output error messages.
    pub fn materialize(&self) -> Vec<u8> {
        let mut out = self.head.clone();
        // Bytes counted in both head and tail when the stream is short.
        let overlap = (self.head.len() + self.tail.len()).saturating_sub(self.total);
        out.extend_from_slice(&self.tail[overlap..]);
        out
    }

    /// Reconstruct the stream: lossless when `total <= 2 * keep`, otherwise
    /// head + truncation marker (with the true byte count) + tail.
    pub fn into_bytes(self) -> Vec<u8> {
        let skipped = self.skipped();
        if skipped == 0 {
            return self.materialize();
        }
        let marker = crate::truncation_marker(skipped, self.total, Some(("cap", self.keep)));
        let mut out = Vec::with_capacity(self.head.len() + self.tail.len() + marker.len());
        out.extend_from_slice(&self.head);
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(&self.tail);
        out
    }
}

// ── Truncation notice marker ────────────────────────────────────────

/// Head+tail truncation notice: `... [N bytes truncated (T total, L V)] ...`,
/// with an optional `, {label} {value}` limit suffix. The live-stream marker
/// is deliberately separate (`streaming_truncation_marker`).
pub(crate) fn truncation_marker(
    skipped: usize,
    total: usize,
    limit: Option<(&str, usize)>,
) -> String {
    let limit = limit.map_or(String::new(), |(label, value)| format!(", {label} {value}"));
    format!("\n\n... [{skipped} bytes truncated ({total} total{limit})] ...\n\n")
}
