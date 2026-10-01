//! Image attachments for tool results.
//!
//! History keeps only path markers (see [`chat::image_marker_message`]), so a
//! session file never carries base64; the bytes are encoded here every time a
//! request is built, reusing the session's [`ImageCache`] for files that have
//! not changed. A request spends a bounded allowance on pictures, newest
//! first: whatever it cannot afford, and whatever a text-only model can never
//! consume, degrades to a plain-text notice in place of the marker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatRole, ContentPart, MessageContent};

use crabot::chat::{image_marker_paths, image_marker_text};
use crabot::tools::{EncodedImage, ImageBudget, encode_for_request, resolve_path};

/// Base64 bytes one request may spend on pictures. Well under every provider's
/// per-request image ceiling, and small enough that a long session of large
/// screenshots cannot silently triple the bill.
const REQUEST_IMAGE_BUDGET_BYTES: usize = 24 * 1024 * 1024;

/// Pictures one request may carry, independent of the byte budget.
const REQUEST_IMAGE_MAX_IMAGES: usize = 20;

/// Images kept encoded at once. Must exceed `REQUEST_IMAGE_MAX_IMAGES`,
/// otherwise an image-heavy request would evict everything every round.
const MAX_CACHED_IMAGES: usize = REQUEST_IMAGE_MAX_IMAGES + 4;

/// Why a picture is not on the wire, phrased for the model to read.
const BUDGET_FULL_REASON: &str =
    "this request's image budget is full; read the file again to see it";
const ENCODE_FAILED_REASON: &str =
    "it could not be encoded within the image limits; read it again, or downscale it first";
const TEXT_ONLY_REASON: &str = "the current model does not accept image input";

/// One encoded image plus the file state it was built from. Fingerprinting by
/// size + mtime misses a same-size rewrite within the mtime resolution — the
/// usual trade-off of a stat-based cache. The budget is part of the key, so
/// tightening a limit never serves the older, larger encoding. A failed
/// encoding is cached as `image: None` under the same fingerprint, so later
/// requests of the session never re-encode a file that cannot fit.
#[derive(Clone)]
struct CachedImage {
    len: u64,
    mtime: SystemTime,
    budget: ImageBudget,
    /// `None` when the file could not be encoded within the budget.
    image: Option<WireImage>,
}

/// One picture ready for the wire: the content part plus its cost in the
/// request's byte allowance.
#[derive(Clone)]
struct WireImage {
    part: ContentPart,
    /// base64 length of `part`, i.e. what it costs in a request.
    wire_bytes: usize,
}

/// Encoded image parts for one session, keyed by resolved path.
///
/// Every request of an agent loop re-attaches the session's whole image
/// history, which would otherwise re-decode and re-encode each file on every
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
    /// The binary part for `path`, encoding it on a cache miss and caching a
    /// failed encoding so it is never retried. `None` only when the file is
    /// gone or unreadable.
    ///
    /// Encoding is CPU-bound, so it runs on the blocking pool rather than on
    /// the async thread that is about to await the request.
    async fn image_for(&mut self, path: &Path, budget: ImageBudget) -> Option<CachedImage> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        if let Some(cached) = self.entries.get(path)
            && cached.len == meta.len()
            && cached.mtime == mtime
            && cached.budget == budget
        {
            #[cfg(test)]
            {
                self.hits += 1;
            }
            return Some(cached.clone());
        }
        let source = path.to_path_buf();
        let image = tokio::task::spawn_blocking(move || encode_for_request(&source, &budget))
            .await
            .ok()
            .and_then(Result::ok)
            .filter(|encoded| encoded.data.len() <= budget.max_bytes)
            .map(|encoded| WireImage {
                part: binary_part(path, &encoded),
                wire_bytes: encoded.base64_len(),
            });
        let entry = CachedImage {
            len: meta.len(),
            mtime,
            budget,
            image,
        };
        if self.entries.len() >= MAX_CACHED_IMAGES {
            self.entries.clear();
        }
        self.entries.insert(path.to_path_buf(), entry.clone());
        Some(entry)
    }
}

/// The image as one binary content part, so the model receives the picture
/// itself rather than its path.
fn binary_part(path: &Path, image: &EncodedImage) -> ContentPart {
    let name = path.file_name().and_then(|name| name.to_str());
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image.data);
    ContentPart::from_binary_base64(image.media_type.clone(), encoded, name.map(String::from))
}

/// What one request may still spend on pictures.
#[derive(Clone, Copy)]
struct Allowance {
    bytes: usize,
    images: usize,
}

impl Allowance {
    const fn new() -> Self {
        Self {
            bytes: REQUEST_IMAGE_BUDGET_BYTES,
            images: REQUEST_IMAGE_MAX_IMAGES,
        }
    }

    /// Reserve room for one picture; `false` once the request is full.
    fn take(&mut self, wire_bytes: usize) -> bool {
        if self.images == 0 || wire_bytes > self.bytes {
            return false;
        }
        self.images -= 1;
        self.bytes -= wire_bytes;
        true
    }
}

/// Deliver the images referenced by marker messages as binary content parts,
/// so the model receives the pictures themselves rather than their paths.
/// Idempotent: materialized messages no longer look like markers.
pub(super) async fn attach_images(
    messages: &mut Vec<ChatMessage>,
    workspace: &Path,
    vision: bool,
    adapter: AdapterKind,
    cache: &mut ImageCache,
    budget: ImageBudget,
) {
    if !vision {
        text_only_notice(messages);
    } else {
        // Newest first: the allowance goes to the pictures this turn is about,
        // and the ones it cannot afford are the oldest.
        let mut allowance = Allowance::new();
        for msg in messages.iter_mut().rev() {
            materialize(msg, workspace, cache, budget, &mut allowance).await;
        }
    }
    if strict_alternation(adapter) {
        merge_into_tool_turn(messages);
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

/// Fold every user message into the preceding turn, keeping a single user
/// turn per model turn: markers merge into the tool results they follow, and
/// a user prompt injected during tool execution merges into that same turn.
/// Genai maps both `Tool` and a promoted `User` message to a `user` turn for
/// these adapters, so the fold preserves strict alternation.
fn merge_into_tool_turn(messages: &mut Vec<ChatMessage>) {
    let mut merged: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for msg in messages.drain(..) {
        match merged.last_mut() {
            // Fold into a tool turn (promoted to user) or a turn left user by
            // a previous fold — never into the system or assistant turns.
            Some(last)
                if msg.role == ChatRole::User
                    && matches!(last.role, ChatRole::Tool | ChatRole::User) =>
            {
                last.role = ChatRole::User;
                last.content.extend(msg.content.into_parts());
            }
            _ => merged.push(msg),
        }
    }
    *messages = merged;
}

/// Replace every marker with a notice, for a model that cannot look at
/// pictures — silence would read as "the image is right there".
fn text_only_notice(messages: &mut [ChatMessage]) {
    for msg in messages.iter_mut() {
        let Some(paths) = image_marker_paths(msg) else {
            continue;
        };
        let omitted: Vec<(String, String)> = paths
            .into_iter()
            .map(|path| (path.to_string(), TEXT_ONLY_REASON.to_string()))
            .collect();
        msg.content =
            MessageContent::from_parts(vec![ContentPart::Text(image_marker_text(&[], &omitted))]);
    }
}

/// Turn one marker message into the pictures the request can still afford.
async fn materialize(
    msg: &mut ChatMessage,
    workspace: &Path,
    cache: &mut ImageCache,
    budget: ImageBudget,
    allowance: &mut Allowance,
) {
    let Some(paths) = image_marker_paths(msg) else {
        return;
    };
    let mut parts: Vec<ContentPart> = Vec::new();
    let mut attached: Vec<String> = Vec::new();
    let mut omitted: Vec<(String, String)> = Vec::new();
    for path in paths {
        let omit = |reason: &str| (path.to_string(), reason.to_string());
        let file = match resolve_path(path, workspace) {
            Ok(file) => file,
            Err(reason) => {
                omitted.push(omit(&format!("{reason}; read it again to see it")));
                continue;
            }
        };
        if allowance.images == 0 {
            omitted.push(omit(BUDGET_FULL_REASON));
            continue;
        }
        // Unreadable file or failed/cached-failed encoding share one notice.
        let Some(image) = cache
            .image_for(&file, budget)
            .await
            .and_then(|entry| entry.image)
        else {
            omitted.push(omit(ENCODE_FAILED_REASON));
            continue;
        };
        if allowance.take(image.wire_bytes) {
            parts.push(image.part);
            attached.push(path.to_string());
        } else {
            omitted.push(omit(BUDGET_FULL_REASON));
        }
    }
    if attached.is_empty() && omitted.is_empty() {
        return;
    }
    let mut text = vec![ContentPart::Text(image_marker_text(&attached, &omitted))];
    text.append(&mut parts);
    msg.content = MessageContent::from_parts(text);
}
