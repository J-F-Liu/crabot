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
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ExtendedColorType, ImageDecoder, ImageReader, Limits};

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

/// Largest encoded source file accepted for one image, before downscaling.
const MAX_SOURCE_BYTES: u64 = 20 * 1024 * 1024;

/// Largest decoded raster accepted for one image, before downscaling.
const MAX_SOURCE_PIXELS: u64 = 64_000_000;

/// Largest stored width or height accepted for one image, before downscaling.
const MAX_SOURCE_SIDE: u32 = 8192;

/// Decoder-side bound above the admission side, so oversized headers reach the
/// friendly admission error before a full decode allocates pixels.
const MAX_DECODE_SIDE: u32 = MAX_SOURCE_SIDE * 2;

/// Longest edge the byte-budget downscale may reach before giving up.
const MIN_ENCODED_SIDE: u32 = 16;

/// JPEG quality ladder, walked until the encoded bytes fit the budget.
const JPEG_QUALITIES: [u8; 3] = [85, 75, 60];

/// Session tabs whose route facts are kept before the table is dropped.
const MAX_TRACKED_ROUTES: usize = 64;

/// The image formats the attachment picker offers; kept in sync with
/// [`EXTENSION_TYPES`].
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

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

/// The MIME type proven by a file's leading bytes.
fn resolve_media_type(head: &[u8], display: &str) -> Result<&'static str, String> {
    sniff_media_type(head).ok_or_else(|| {
        format!(
            "Failed to decode image {display}: the bytes are not a PNG/JPEG/WebP/GIF/BMP image; \
             the file may be truncated or corrupt"
        )
    })
}

/// Facts a full decode proves about an image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProbe {
    /// Media type proven by the file's leading bytes.
    pub media_type: &'static str,
    /// Intrinsic width after EXIF orientation — the width a viewer perceives.
    pub width: u32,
    /// Intrinsic height after EXIF orientation — the height a viewer perceives.
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

/// A fully decoded source image plus the facts to decide whether its bytes may
/// go to a provider unchanged.
struct Source {
    /// Decoded pixels with EXIF orientation applied.
    image: DynamicImage,
    /// Exact source bytes, kept for a metadata-free pass-through.
    data: Vec<u8>,
    media_type: &'static str,
    bytes: u64,
    /// Whether the source is already a wire-safe, metadata-free 8-bit raster.
    clean: bool,
}

impl Source {
    /// Read, validate, and decode a file once; the returned image is oriented
    /// and the source bytes are kept for the pass-through decision.
    fn load(path: &Path, display: &str) -> Result<Self, String> {
        let metadata =
            std::fs::metadata(path).map_err(|e| format!("Failed to read image {display}: {e}"))?;
        if metadata.len() > MAX_SOURCE_BYTES {
            return Err(format!(
                "Image {display} is {} bytes; the limit is {MAX_SOURCE_BYTES} bytes",
                metadata.len()
            ));
        }
        let data =
            std::fs::read(path).map_err(|e| format!("Failed to read image {display}: {e}"))?;
        let bytes = data.len() as u64;
        let media_type = resolve_media_type(&data[..data.len().min(SIGNATURE_BYTES)], display)?;

        let mut reader = ImageReader::new(Cursor::new(data.as_slice()))
            .with_guessed_format()
            .map_err(|e| format!("Failed to decode image {display}: {e}"))?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_DECODE_SIDE);
        limits.max_image_height = Some(MAX_DECODE_SIDE);
        reader.limits(limits);

        let mut decoder = reader
            .into_decoder()
            .map_err(|e| format!("Failed to decode image {display}: {e}"))?;
        let (stored_width, stored_height) = decoder.dimensions();
        let exif = decoder.exif_metadata();
        let exif_present = matches!(exif, Ok(Some(_)));
        let exif_ok = exif.is_ok();
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        let (width, height) = oriented_dimensions(orientation, stored_width, stored_height);
        if width == 0 || height == 0 {
            return Err(format!(
                "Failed to decode image {display}: it has zero dimensions"
            ));
        }
        let pixels = u64::from(width) * u64::from(height);
        if pixels > MAX_SOURCE_PIXELS {
            return Err(format!(
                "Image {display} is {width}x{height} ({pixels} pixels); the limit is \
                 {MAX_SOURCE_PIXELS} pixels"
            ));
        }
        if width.max(height) > MAX_SOURCE_SIDE {
            return Err(format!(
                "Image {display} is {width}x{height}; each side must be at most \
                 {MAX_SOURCE_SIDE} pixels"
            ));
        }

        let color_ok =
            is_8bit_color(decoder.color_type()) && is_8bit_extended(decoder.original_color_type());
        let metadata_free = exif_ok
            && !exif_present
            && metadata_absent(decoder.icc_profile())
            && metadata_absent(decoder.xmp_metadata())
            && metadata_absent(decoder.iptc_metadata());
        let animated = is_animated(&data, media_type);
        let adobe_cmyk = media_type == "image/jpeg" && jpeg_uses_cmyk(&data);
        let wire_safe = matches!(media_type, "image/png" | "image/jpeg" | "image/webp");

        let mut image = DynamicImage::from_decoder(decoder)
            .map_err(|e| format!("Failed to decode image {display}: {e}"))?;
        image.apply_orientation(orientation);

        let clean = orientation == Orientation::NoTransforms
            && color_ok
            && metadata_free
            && !animated
            && !adobe_cmyk
            && wire_safe;
        Ok(Self {
            image,
            data,
            media_type,
            bytes,
            clean,
        })
    }
}

/// Dimensions after EXIF orientation transposes the stored raster.
fn oriented_dimensions(orientation: Orientation, width: u32, height: u32) -> (u32, u32) {
    match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (height, width),
        _ => (width, height),
    }
}

/// Whether decoded pixels and their source color type are 8-bit.
fn is_8bit_color(color: ColorType) -> bool {
    matches!(
        color,
        ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8
    )
}

fn is_8bit_extended(color: ExtendedColorType) -> bool {
    matches!(
        color,
        ExtendedColorType::L8
            | ExtendedColorType::La8
            | ExtendedColorType::Rgb8
            | ExtendedColorType::Rgba8
    )
}

fn metadata_absent(result: image::ImageResult<Option<Vec<u8>>>) -> bool {
    matches!(result, Ok(None))
}

/// Whether a container declares animation, so it must be re-encoded static.
fn is_animated(data: &[u8], media_type: &str) -> bool {
    match media_type {
        "image/gif" => true,
        "image/png" => contains_subslice(data, b"acTL"),
        "image/webp" => contains_subslice(data, b"ANIM") || contains_subslice(data, b"ANMF"),
        _ => false,
    }
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Adobe APP14 transform 0/2 marks CMYK/YCCK bytes, which providers may reject.
fn jpeg_uses_cmyk(data: &[u8]) -> bool {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return false;
    }
    let mut i = 2usize;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xD8 || marker == 0xD9 {
            i += 2;
            continue;
        }
        if marker == 0xDA {
            break;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        let end = i + 2 + len;
        if len < 2 || end > data.len() {
            break;
        }
        let payload = &data[i + 4..end];
        if marker == 0xEE && payload.len() >= 12 && payload.starts_with(b"Adobe") {
            let transform = payload[11];
            if transform == 0 || transform == 2 {
                return true;
            }
        }
        i = end;
    }
    false
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

/// A source image plus the exact bytes the request will carry for it.
pub struct PreparedImage {
    /// Source facts: orientation applied, before any downscale.
    pub probe: ImageProbe,
    /// The bytes and dimensions the model will receive.
    pub encoded: EncodedImage,
}

/// Decode and prepare `path` for a request. The source is passed through only
/// when it is already a clean 8-bit sRGB wire format; otherwise it is
/// re-encoded and downscaled until it fits every budget.
pub fn prepare_for_request(
    path: &Path,
    display: &str,
    budget: &ImageBudget,
) -> Result<PreparedImage, String> {
    let source = Source::load(path, display)?;
    let probe = ImageProbe {
        media_type: source.media_type,
        width: source.image.width(),
        height: source.image.height(),
        bytes: source.bytes,
    };
    let encoded = encode_source(source, budget, display)?;
    Ok(PreparedImage { probe, encoded })
}

/// The bytes to send for `path`; see [`prepare_for_request`].
pub fn encode_for_request(path: &Path, budget: &ImageBudget) -> Result<EncodedImage, String> {
    prepare_for_request(path, &path.display().to_string(), budget).map(|prepared| prepared.encoded)
}

fn encode_source(
    source: Source,
    budget: &ImageBudget,
    display: &str,
) -> Result<EncodedImage, String> {
    let Source {
        image,
        data,
        media_type,
        bytes,
        clean,
    } = source;
    let (source_width, source_height) = (image.width(), image.height());
    if clean && budget.fits(source_width, source_height, bytes) {
        return Ok(EncodedImage {
            media_type: media_type.to_string(),
            width: source_width,
            height: source_height,
            data,
        });
    }

    // Re-encoding always writes 8-bit sRGB-like bytes; flatten an opaque alpha
    // plane so JPEG can carry it.
    let image = to_wire_image(image);
    let transparent = matches!(image.color(), ColorType::Rgba8);
    let max_bytes = budget.max_bytes.max(1);
    let (mut width, mut height) = target_dimensions(source_width, source_height, budget);
    let mut scaled = image.resize_exact(width, height, FilterType::Lanczos3);

    loop {
        let (data, out_type) = encode_within_budget(&scaled, max_bytes, transparent);
        if !data.is_empty() && data.len() <= max_bytes {
            verify_encoded(&data, out_type, width, height, transparent)
                .map_err(|e| format!("Failed to encode image {display}: {e}"))?;
            return Ok(EncodedImage {
                media_type: out_type.to_string(),
                width,
                height,
                data,
            });
        }
        if width.max(height) <= MIN_ENCODED_SIDE {
            return Err(format!(
                "Failed to encode image {display}: a {width}x{height} copy is still larger \
                 than {max_bytes} bytes"
            ));
        }
        (width, height) = shrink_dimensions(width, height);
        scaled = scaled.resize_exact(width, height, FilterType::Lanczos3);
    }
}

/// Normalize decoded pixels to an 8-bit RGB/RGBA image so encoders cannot emit
/// 16-bit or alpha-carrying output the route does not expect.
fn to_wire_image(image: DynamicImage) -> DynamicImage {
    if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        if rgba.pixels().any(|pixel| pixel.0[3] != u8::MAX) {
            return DynamicImage::ImageRgba8(rgba);
        }
    }
    if matches!(image.color(), ColorType::L8 | ColorType::Rgb8) {
        image
    } else {
        DynamicImage::ImageRgb8(image.to_rgb8())
    }
}

/// Encode `image` at its current size, preferring the smallest candidate that
/// fits `max_bytes`. The caller shrinks further when every candidate is over.
fn encode_within_budget(
    image: &DynamicImage,
    max_bytes: usize,
    transparent: bool,
) -> (Vec<u8>, &'static str) {
    if transparent {
        let mut best: Option<(Vec<u8>, &'static str)> = None;
        let mut buf = Vec::new();
        if image
            .write_with_encoder(WebPEncoder::new_lossless(&mut buf))
            .is_ok()
            && !buf.is_empty()
        {
            if buf.len() <= max_bytes {
                return (buf, "image/webp");
            }
            best = Some((buf, "image/webp"));
        }
        let mut buf = Vec::new();
        if image.write_with_encoder(PngEncoder::new(&mut buf)).is_ok() && !buf.is_empty() {
            if buf.len() <= max_bytes {
                return (buf, "image/png");
            }
            if best
                .as_ref()
                .is_none_or(|(smallest, _)| buf.len() < smallest.len())
            {
                best = Some((buf, "image/png"));
            }
        }
        if let Some(best) = best {
            return best;
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

/// Confirm an encoded candidate really is the promised 8-bit wire image.
fn verify_encoded(
    data: &[u8],
    media_type: &str,
    width: u32,
    height: u32,
    expect_alpha: bool,
) -> Result<(), String> {
    let sniffed = sniff_media_type(data)
        .ok_or_else(|| "the encoder produced bytes with no recognizable signature".to_string())?;
    if sniffed != media_type {
        return Err(format!(
            "the encoder declared {media_type}, but the bytes are {sniffed}"
        ));
    }
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let encoded = decoder.dimensions();
    if encoded != (width, height) {
        return Err(format!(
            "the encoder produced {encoded:?} instead of {width}x{height}"
        ));
    }
    let color = decoder.color_type();
    if !is_8bit_color(color) {
        return Err(format!(
            "the encoder produced {color:?} instead of an 8-bit raster"
        ));
    }
    if expect_alpha && !matches!(color, ColorType::La8 | ColorType::Rgba8) {
        return Err("the encoder dropped the alpha channel".to_string());
    }
    Ok(())
}

/// Shrink one step toward the byte-budget floor without losing aspect ratio.
fn shrink_dimensions(width: u32, height: u32) -> (u32, u32) {
    let scale = 0.75f64;
    let next = |value: u32| ((f64::from(value) * scale).floor() as u32).max(1);
    (next(width), next(height))
}

// ── Preview decoding ─────────────────────────────────────────────────

/// Decoded RGBA pixels for an in-app image preview.
#[derive(Debug)]
pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decode and orient `path` for display within `max_side`/`max_pixels`.
pub fn decode_for_preview(
    path: &Path,
    max_side: u32,
    max_pixels: u64,
) -> Result<PreviewImage, String> {
    let display = path.display().to_string();
    let source = Source::load(path, &display)?;
    let (width, height) = target_dimensions(
        source.image.width(),
        source.image.height(),
        &ImageBudget {
            max_pixels: max_pixels.max(1),
            max_side: max_side.max(1),
            max_bytes: usize::MAX,
        },
    );
    let image = if (width, height) == (source.image.width(), source.image.height()) {
        source.image
    } else {
        source
            .image
            .resize_exact(width, height, FilterType::Triangle)
    };
    Ok(PreviewImage {
        width,
        height,
        rgba: image.to_rgba8().into_raw(),
    })
}
