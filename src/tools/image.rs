//! Shared image policy: one [`ImageBudget`] governs both sides of a picture
//! attachment — `read` reports the dimensions it resolves, and the request
//! builder encodes to exactly those. The budget comes from [`ToolLimits`],
//! narrowed per route (see [`ImageBudget::for_model`]).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{LazyLock, RwLock};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader, Limits};

use crate::model::ModelInfo;

use super::limits::{ToolLimits, tool_limits};

// ── Budget ───────────────────────────────────────────────────────────

/// Ceilings one image must respect on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageBudget {
    /// Decoded pixels (width × height) above which the image is downscaled.
    pub max_pixels: u64,
    /// Longest edge above which the image is downscaled.
    pub max_side: u32,
    /// Encoded bytes above which the quality ladder is walked.
    pub max_bytes: usize,
}

impl ImageBudget {
    /// The deployment budget, as configured in the tool limits.
    pub fn from_limits(limits: &ToolLimits) -> Self {
        Self {
            max_pixels: limits.read_max_image_pixels,
            max_side: limits.read_max_image_side,
            max_bytes: limits.read_max_image_bytes,
        }
    }

    /// The deployment budget narrowed by what the route does with oversized
    /// images anyway. Providers rescale before the model sees them, so
    /// matching their target loses no detail and saves the tokens a larger
    /// raster would cost on every request of the session.
    pub fn for_model(model: &ModelInfo) -> Self {
        let mut budget = Self::from_limits(&tool_limits());
        if let Some(edge) = route_long_edge(&model.api_type) {
            budget.max_side = budget.max_side.min(edge);
        }
        budget
    }

    /// Whether a source of this size survives untouched.
    pub fn fits(&self, width: u32, height: u32, bytes: u64) -> bool {
        width as u64 * height as u64 <= self.max_pixels
            && width.max(height) <= self.max_side
            && bytes <= self.max_bytes as u64
    }
}

/// Long edge a provider's documented resize target imposes, by `api_type`.
/// Only adapters with a published target are listed: an unknown or custom
/// endpoint keeps the deployment budget rather than guessing.
fn route_long_edge(api_type: &str) -> Option<u32> {
    match api_type {
        "anthropic" => Some(1568),
        "openai" | "openai_resp" => Some(2048),
        _ => None,
    }
}

/// Aspect-preserving dimensions inside both budgets, rounded inward so the
/// result never exceeds either. Never enlarges.
pub fn target_dimensions(width: u32, height: u32, budget: &ImageBudget) -> (u32, u32) {
    let (w, h) = (width as f64, height as f64);
    let pixels = (budget.max_pixels as f64 / (w * h)).sqrt();
    let side = budget.max_side as f64 / w.max(h);
    let scale = pixels.min(side).min(1.0);
    if scale >= 1.0 {
        return (width, height);
    }
    let mut tw = ((w * scale).floor() as u32).max(1);
    let mut th = ((h * scale).floor() as u32).max(1);
    // Rounding can leave the pair a hair over the pixel budget.
    while tw as u64 * th as u64 > budget.max_pixels && (tw, th) != (1, 1) {
        if tw >= th {
            tw -= 1;
        } else {
            th -= 1;
        }
    }
    (tw, th)
}

// ── Per-turn route facts ─────────────────────────────────────────────

/// What the LLM loop publishes about the route driving the current turn, so
/// tools can refuse work the model could not possibly consume.
#[derive(Debug, Clone)]
pub struct RouteImages {
    /// Whether the model accepts image input.
    pub vision: bool,
    /// Model id, named in refusal messages.
    pub model_id: String,
    /// The budget every image of this turn is encoded against.
    pub budget: ImageBudget,
}

/// Route facts per session tab, refreshed at the start of every turn. Tools
/// run inside a tab scope, so the read tool resolves the same budget the
/// request builder will.
static ROUTE_IMAGES: LazyLock<RwLock<HashMap<usize, RouteImages>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Publish the route facts for one session tab.
pub fn set_route_images(tab: usize, route: RouteImages) {
    if let Ok(mut routes) = ROUTE_IMAGES.write() {
        // Tab numbers are never reused and a missing entry is harmless, so the
        // whole table is dropped once stale entries pile up.
        if routes.len() >= MAX_TRACKED_ROUTES {
            routes.clear();
        }
        routes.insert(tab, route);
    }
}

/// The route facts of the tab running the current tool call; `None` outside
/// the LLM loop, where no route has been resolved yet.
pub fn route_images(tab: usize) -> Option<RouteImages> {
    ROUTE_IMAGES
        .read()
        .ok()
        .and_then(|routes| routes.get(&tab).cloned())
}

// ── Media types ──────────────────────────────────────────────────────

/// Bytes read from the head of a file for signature sniffing.
const SIGNATURE_BYTES: usize = 16;

/// Longest edge the decoder accepts outright; bounds decompression bombs
/// without refusing images we could still downscale.
const MAX_DECODE_SIDE: u32 = 32_000;

/// JPEG quality ladder, walked until the encoded bytes fit the budget.
const JPEG_QUALITIES: [u8; 3] = [85, 75, 60];

/// Session tabs whose route facts are kept before the table is dropped.
const MAX_TRACKED_ROUTES: usize = 64;

/// The image formats `read` attaches, by file extension.
const EXTENSION_TYPES: [(&str, &str); 6] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
];

/// Media type declared by a path's extension, or `None` when it claims no
/// supported image format.
pub fn media_type_for_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    EXTENSION_TYPES
        .iter()
        .find(|(candidate, _)| ext.eq_ignore_ascii_case(candidate))
        .map(|(_, media_type)| *media_type)
}

/// Media type proven by a file's leading bytes.
pub fn sniff_media_type(head: &[u8]) -> Option<&'static str> {
    let starts = |prefix: &[u8]| head.starts_with(prefix);
    let ascii = |offset: usize, word: &[u8]| {
        head.len() >= offset + word.len() && &head[offset..offset + word.len()] == word
    };
    if starts(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if starts(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if ascii(0, b"GIF87a") || ascii(0, b"GIF89a") {
        return Some("image/gif");
    }
    if ascii(0, b"RIFF") && ascii(8, b"WEBP") {
        return Some("image/webp");
    }
    if starts(b"BM") {
        return Some("image/bmp");
    }
    None
}

/// Read the leading bytes used for signature sniffing.
fn read_signature(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut head = vec![0u8; SIGNATURE_BYTES];
    let mut file = File::open(path)?;
    let read = file.read(&mut head)?;
    head.truncate(read);
    Ok(head)
}

/// Media type proven by a file's own leading bytes, without reading it whole.
pub fn sniff_file(path: &Path) -> Option<&'static str> {
    sniff_media_type(&read_signature(path).ok()?)
}

/// The media type to attach `path` as: its extension when it names an image,
/// otherwise whatever the bytes prove. A declared type the bytes contradict is
/// an error — the provider would be handed the wrong MIME.
fn resolve_media_type(path: &Path, display: &str) -> Result<&'static str, String> {
    let head = read_signature(path).map_err(|e| format!("Failed to read image {display}: {e}"))?;
    let declared = media_type_for_extension(path);
    let sniffed = sniff_media_type(&head);
    match (declared, sniffed) {
        (Some(declared), Some(sniffed)) if declared != sniffed => {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            Err(format!(
                "Failed to decode image {display}: the .{ext} extension declares {declared}, \
                 but the bytes are {sniffed}; rename the file to match its actual format, \
                 or convert it to PNG/JPEG/WebP"
            ))
        }
        (_, None) => Err(format!(
            "Failed to decode image {display}: the bytes are not a PNG/JPEG/WebP/GIF/BMP image; \
             the file may be truncated or corrupt"
        )),
        (declared, sniffed) => Ok(declared.or(sniffed).expect("one arm is Some")),
    }
}

/// Facts a full decode proves about an image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProbe {
    /// Media type agreed on by the extension and the bytes.
    pub media_type: &'static str,
    /// Intrinsic width as stored, EXIF orientation not applied.
    pub width: u32,
    /// Intrinsic height as stored, EXIF orientation not applied.
    pub height: u32,
    /// Encoded size of the file on disk.
    pub bytes: u64,
}

/// Decode `path` completely and report what it holds. Full decode (not a
/// header probe) is what keeps a truncated file from reaching a provider.
pub fn probe(path: &Path, display: &str) -> Result<ImageProbe, String> {
    let source = Source::load(path, display)?;
    Ok(ImageProbe {
        media_type: source.media_type,
        width: source.image.width(),
        height: source.image.height(),
        bytes: source.bytes,
    })
}

/// A fully decoded source image plus the facts of the file it came from.
struct Source {
    image: DynamicImage,
    media_type: &'static str,
    bytes: u64,
}

impl Source {
    /// Resolve the media type, stat, and decode the file in one pass.
    fn load(path: &Path, display: &str) -> Result<Self, String> {
        let media_type = resolve_media_type(path, display)?;
        let bytes = std::fs::metadata(path)
            .map_err(|e| format!("Failed to read image {display}: {e}"))?
            .len();
        let image = decode(path).map_err(|e| format!("Failed to decode image {display}: {e}"))?;
        Ok(Self {
            image,
            media_type,
            bytes,
        })
    }
}

/// Decode a file with the side limit applied, so a decompression bomb is
/// refused before it is materialized.
fn decode(path: &Path) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::open(path).map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    reader.limits(limits);
    reader
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())
}

// ── Wire encoding ────────────────────────────────────────────────────

/// One image ready to be put on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedImage {
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl EncodedImage {
    /// base64 length of `data`, the size it occupies in the request.
    pub fn base64_len(&self) -> usize {
        self.data.len().div_ceil(3) * 4
    }
}

/// The bytes to send for `path`: the file verbatim when it already fits every
/// budget, otherwise a downscaled copy re-encoded to drop what the provider
/// cannot use (metadata, extra frames, more than 8 bits per channel).
///
/// EXIF orientation is deliberately not applied: the dimensions reported to
/// the model and the pixels it sees stay the stored ones, which keeps its
/// coordinate advice exact.
pub fn encode_for_request(path: &Path, budget: &ImageBudget) -> Result<EncodedImage, String> {
    let display = path.display().to_string();
    let source = Source::load(path, &display)?;
    let (width, height) = (source.image.width(), source.image.height());

    // An animated source never passes through: providers only ever show the
    // first frame anyway, and a static copy is what they receive.
    if budget.fits(width, height, source.bytes) && source.media_type != "image/gif" {
        let data =
            std::fs::read(path).map_err(|e| format!("Failed to read image {display}: {e}"))?;
        return Ok(EncodedImage {
            media_type: source.media_type.to_string(),
            width,
            height,
            data,
        });
    }

    let (target_w, target_h) = target_dimensions(width, height, budget);
    let scaled = if (target_w, target_h) == (width, height) {
        source.image
    } else {
        source
            .image
            .resize_exact(target_w, target_h, FilterType::Lanczos3)
    };
    let (data, media_type) = encode_within_budget(&scaled, budget.max_bytes);
    // The envelope promises these dimensions; confirm the encoder agreed
    // before the model is told anything about them.
    let encoded = dimensions_in_memory(&data)
        .map_err(|e| format!("Failed to encode image {display}: {e}"))?;
    if encoded != (target_w, target_h) {
        return Err(format!(
            "Failed to encode image {display}: encoder produced {encoded:?} instead of {target_w}x{target_h}"
        ));
    }
    Ok(EncodedImage {
        media_type: media_type.to_string(),
        width: target_w,
        height: target_h,
        data,
    })
}

/// Encode `image` into the smallest representation the encoder offers that
/// fits `max_bytes`: transparency keeps WebP, everything else walks a JPEG
/// quality ladder. When nothing fits, the smallest result is returned and the
/// caller drops the image.
fn encode_within_budget(image: &DynamicImage, max_bytes: usize) -> (Vec<u8>, &'static str) {
    if has_transparency(image) {
        let mut buf = Vec::new();
        if image
            .write_with_encoder(WebPEncoder::new_lossless(&mut buf))
            .is_ok()
        {
            return (buf, "image/webp");
        }
    }
    let mut smallest = Vec::new();
    for quality in JPEG_QUALITIES {
        let mut buf = Vec::new();
        if image
            .write_with_encoder(JpegEncoder::new_with_quality(&mut buf, quality))
            .is_err()
        {
            continue;
        }
        if buf.len() <= max_bytes {
            return (buf, "image/jpeg");
        }
        if smallest.is_empty() || buf.len() < smallest.len() {
            smallest = buf;
        }
    }
    (smallest, "image/jpeg")
}

/// Whether any pixel is not fully opaque; a fully opaque alpha plane is not
/// worth the lossless codec.
fn has_transparency(image: &DynamicImage) -> bool {
    match image.as_rgba8() {
        Some(rgba) => rgba.pixels().any(|pixel| pixel.0[3] != u8::MAX),
        None => image.has_alpha(),
    }
}

/// Intrinsic dimensions of an encoded image, from its header alone.
fn dimensions_in_memory(data: &[u8]) -> Result<(u32, u32), String> {
    ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|e| e.to_string())
}
