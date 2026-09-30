//! Image attachments for tool results.
//!
//! History keeps only path markers (see [`chat::image_marker_message`]), so a
//! session file never carries base64; the bytes are materialized here every
//! time a request is built, reusing the session's [`ImageCache`] for files that
//! have not changed. Unmaterialized markers stay plain text for text-only
//! models (and the UI).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatRole, ContentPart};

use crabot::chat::image_marker_paths;
use crabot::tools::{resolve_path, tool_limits};

/// Images kept encoded at once; the whole set is dropped when the cap is hit,
/// since a session attaches a handful of pictures.
const MAX_CACHED_IMAGES: usize = 16;

/// One encoded image plus the file state it was built from. Fingerprinting by
/// size + mtime misses a same-size rewrite within the mtime resolution — the
/// usual trade-off of a stat-based cache.
struct CachedImage {
    len: u64,
    mtime: SystemTime,
    part: ContentPart,
}

/// Encoded image parts for one session, keyed by resolved path.
///
/// Every request of an agent loop re-attaches the session's whole image
/// history, which would otherwise re-read and re-encode each file on every
/// round. genai's `Binary` holds its base64 behind an `Arc`, so a cached part
/// clones cheaply and only changed files pay for encoding.
#[derive(Default)]
pub(crate) struct ImageCache {
    entries: HashMap<PathBuf, CachedImage>,
    /// Test-only counter of served-from-cache attachments.
    #[cfg(test)]
    hits: usize,
}

impl std::fmt::Debug for ImageCache {
    /// Entry count only — a `Debug` dump would print megabytes of base64.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageCache")
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl ImageCache {
    /// The binary part for `path`, reading and encoding it on a cache miss.
    /// `None` when the file is gone, unreadable or larger than `max_bytes` —
    /// the limit is re-checked here so a file that grew since `read` is not
    /// sent.
    fn part_for(&mut self, path: &Path, max_bytes: u64) -> Option<ContentPart> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        if meta.len() > max_bytes {
            self.entries.remove(path);
            return None;
        }
        if let Some(cached) = self.entries.get(path)
            && cached.len == meta.len()
            && cached.mtime == mtime
        {
            #[cfg(test)]
            {
                self.hits += 1;
            }
            return Some(cached.part.clone());
        }
        let part = ContentPart::from_binary_file(path).ok()?;
        if self.entries.len() >= MAX_CACHED_IMAGES {
            self.entries.clear();
        }
        self.entries.insert(
            path.to_path_buf(),
            CachedImage {
                len: meta.len(),
                mtime,
                part: part.clone(),
            },
        );
        Some(part)
    }
}

/// Deliver the images referenced by marker messages as binary content parts,
/// so the model receives the pictures themselves rather than their paths.
/// Idempotent: materialized messages no longer look like markers.
pub(super) fn attach_images(
    messages: &mut Vec<ChatMessage>,
    workspace: &Path,
    vision: bool,
    adapter: AdapterKind,
    cache: &mut ImageCache,
) {
    if !vision {
        return;
    }
    if strict_alternation(adapter) {
        merge_into_tool_turn(messages, workspace, cache);
        return;
    }
    for msg in messages.iter_mut() {
        let images = marker_images(msg, workspace, cache);
        msg.content.extend(images);
    }
}

/// Adapters that reject two `user` turns in a row (strict user/model
/// alternation), so tool results and their images must share one turn.
fn strict_alternation(adapter: AdapterKind) -> bool {
    matches!(
        adapter,
        AdapterKind::Gemini
            | AdapterKind::Vertex
            | AdapterKind::BedrockApi
            | AdapterKind::Anthropic
    )
}

/// Move each marker's images into the preceding tool message, keeping one user
/// turn per model turn. Genai maps `Tool` to a `user` turn for these adapters,
/// and a `User` message maps tool responses *and* binaries alike, so promoting
/// the tool message keeps both.
fn merge_into_tool_turn(messages: &mut Vec<ChatMessage>, workspace: &Path, cache: &mut ImageCache) {
    let mut merged: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for msg in messages.drain(..) {
        let images = marker_images(&msg, workspace, cache);
        match merged.last_mut().filter(|last| last.role == ChatRole::Tool) {
            Some(tool_msg) if !images.is_empty() => {
                tool_msg.role = ChatRole::User;
                tool_msg.content.extend(images);
            }
            _ => merged.push(msg),
        }
    }
    *messages = merged;
}

/// Binary parts for a marker message; empty for any other message. Missing,
/// unreadable or oversized files are skipped, leaving the marker text as a note.
fn marker_images(msg: &ChatMessage, workspace: &Path, cache: &mut ImageCache) -> Vec<ContentPart> {
    let Some(paths) = image_marker_paths(msg) else {
        return Vec::new();
    };
    let max_bytes = tool_limits().read_max_image_bytes as u64;
    paths
        .iter()
        .filter_map(|path| {
            let file = resolve_path(path, workspace).ok()?;
            cache.part_for(&file, max_bytes)
        })
        .collect()
}
