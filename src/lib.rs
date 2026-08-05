pub mod chat;
pub mod model;
pub mod model_database;
pub mod session;
pub mod settings;
pub mod setup;
pub mod tools;
pub mod user;
pub mod workspace;

use std::collections::HashSet;

pub fn app_title() -> &'static str {
    concat!("Crabot v", env!("CARGO_PKG_VERSION"))
}
use std::hash::Hash;

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
        let marker = format!(
            "\n\n... [{skipped} bytes truncated ({total} total, cap {keep})] ...\n\n",
            total = self.total,
            keep = self.keep,
        );
        let mut out = Vec::with_capacity(self.head.len() + self.tail.len() + marker.len());
        out.extend_from_slice(&self.head);
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(&self.tail);
        out
    }
}
