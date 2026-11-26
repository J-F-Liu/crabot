//! Decoding for tool input and output.
//!
//! Shell output uses a fixed encoding or UTF-8 with a Windows console fallback;
//! [`PlainTextDecoder`] adds the terminal filter. [`decode_bytes`] detects
//! arbitrary data's charset.

use std::borrow::Cow;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8, X_USER_DEFINED};

use super::ansi::PlainText;

/// Buffered bytes that force an encoding guess (a GBK/Shift_JIS pair is
/// almost never valid UTF-8, so in practice the guess fires on the first
/// non-ASCII byte).
const DETECT_THRESHOLD: usize = 1024;

/// Detector shared by whole-buffer and streaming decode paths.
fn new_detector() -> EncodingDetector {
    EncodingDetector::new(Iso2022JpDetection::Deny)
}

/// Share of control bytes above which a buffer is treated as binary.
const MAX_CONTROL_PERCENT: usize = 10;

/// C0 control byte (except whitespace) or DEL.
const fn is_control(byte: u8) -> bool {
    matches!(byte, 0x00..=0x08 | 0x0e..=0x1a | 0x1c..=0x1f | 0x7f)
}

/// Binary heuristic: NUL bytes or too many control bytes.
fn looks_like_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0)
        && bytes.iter().filter(|&&byte| is_control(byte)).count() * 100
            <= bytes.len() * MAX_CONTROL_PERCENT
}

/// Decode a bounded buffer to UTF-8, or `None` when it looks binary. A BOM
/// overrides the heuristic (UTF-16 text is full of NULs); BOM-less UTF-16
/// stays binary, and legacy charsets are guessed and validated. Unlike
/// [`StreamDecoder`], this path rejects binary data and keeps the valid UTF-8
/// prefix when a byte limit cuts a character.
pub(crate) fn decode_text_attachment(bytes: &[u8]) -> Option<String> {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom..]);
        return clean_text(text);
    }
    if !looks_like_text(bytes) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_owned()),
        // A byte limit can cut a multi-byte character in half.
        Err(err) if err.error_len().is_none() => {
            Some(String::from_utf8_lossy(&bytes[..err.valid_up_to()]).into_owned())
        }
        Err(_) => legacy_text(bytes),
    }
}

/// Guess a legacy charset for bytes that are not valid UTF-8.
fn legacy_text(bytes: &[u8]) -> Option<String> {
    let mut detector = new_detector();
    detector.feed(bytes, true);
    // Deny UTF-8 here: unlike `StreamDecoder`, this buffer already failed
    // UTF-8 validation, so a UTF-8 guess would only produce replacements.
    let encoding = detector.guess(None, Utf8Detection::Deny);
    // x-user-defined maps every byte to a private-use glyph: a failed guess.
    if encoding == X_USER_DEFINED {
        return None;
    }
    let (text, _) = encoding.decode_without_bom_handling(bytes);
    clean_text(text)
}

/// Reject decodes full of controls or replacement/private-use glyphs: a wrong
/// guess or binary data. One is allowed at a truncation boundary.
fn clean_text(text: Cow<'_, str>) -> Option<String> {
    let mut chars = 0;
    let mut suspicious = 0;
    for c in text.chars() {
        chars += 1;
        if suspicious_char(c) {
            suspicious += 1;
        }
    }
    (suspicious <= 1 || suspicious * 100 <= chars).then(|| text.into_owned())
}

/// [`is_control`] plus replacement and private-use glyphs from a bad guess.
fn suspicious_char(c: char) -> bool {
    matches!(c, '\u{FFFD}' | '\u{007F}'..='\u{009F}' | '\u{F780}'..='\u{F7FF}')
        || u8::try_from(c).is_ok_and(is_control)
}

/// Decode a whole buffer into text: [`StreamDecoder`] fed in one go. Escapes
/// and control characters survive — use [`decode_plain`] for shell output.
pub(crate) fn decode_bytes(bytes: &[u8]) -> String {
    decode_all(&mut StreamDecoder::new(), bytes)
}

/// Decode a whole buffer as plain text with `encoding`.
pub(crate) fn decode_plain(bytes: &[u8], encoding: &'static Encoding) -> String {
    decode_plain_with_fallback(bytes, encoding, None)
}

/// Like [`decode_plain`], with a legacy fallback for output that is not valid
/// UTF-8. Only the bytes after the valid UTF-8 prefix use the fallback.
pub(crate) fn decode_plain_with_fallback(
    bytes: &[u8],
    encoding: &'static Encoding,
    fallback: Option<&'static Encoding>,
) -> String {
    decode_all(
        &mut PlainTextDecoder::with_fallback(encoding, fallback),
        bytes,
    )
}

/// Feed a whole buffer through `decoder`, flush, and merge the chunks.
fn decode_all(decoder: &mut impl ChunkDecoder, bytes: &[u8]) -> String {
    let mut out = Vec::new();
    decoder.feed(bytes, &mut out);
    decoder.flush(&mut out);
    out.concat()
}

/// Incremental decoder that emits text chunks.
pub(crate) trait ChunkDecoder {
    /// Decode `bytes`, appending text chunks to `out`.
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>);
    /// Flush the end of a stream: buffered bytes and any carried partial state.
    fn flush(&mut self, out: &mut Vec<String>);
}

/// Incremental charset detector for arbitrary data: ASCII passes through
/// immediately, and the charset is pinned by a BOM, valid UTF-8, or a detector
/// guess.
struct StreamDecoder {
    /// Pinned decoder; `None` while the encoding is still undecided.
    decoder: Option<Decoder>,
    /// Bytes buffered while undecided.
    pending: Vec<u8>,
    detector: EncodingDetector,
    /// BOM sniffing applies only before the first emitted byte.
    at_start: bool,
}

impl StreamDecoder {
    fn new() -> Self {
        Self {
            decoder: None,
            pending: Vec::new(),
            detector: new_detector(),
            at_start: true,
        }
    }

    /// Feed while undecided: emit ASCII live (byte-identical in every legacy
    /// encoding), pin on a BOM or valid UTF-8, else buffer until the detector
    /// has enough evidence.
    fn undecided_feed(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        let ascii = Encoding::ascii_valid_up_to(bytes);
        let (prefix, rest) = bytes.split_at(ascii);
        if self.pending.is_empty() && !prefix.is_empty() {
            // SAFETY: prefix is pure ASCII by ascii_valid_up_to.
            let ascii = unsafe { std::str::from_utf8_unchecked(prefix) };
            emit(ascii.to_string(), out);
            self.at_start = false;
        } else {
            self.pending.extend_from_slice(prefix); // keep order behind buffered bytes
        }
        if !rest.is_empty() {
            self.pending.extend_from_slice(rest);
            // Checked before the UTF-8 fast path so a UTF-8 BOM (valid UTF-8)
            // isn't emitted as U+FEFF.
            if self.at_start
                && let Some((encoding, bom)) = Encoding::for_bom(&self.pending)
            {
                self.pending.drain(..bom);
                self.pin(encoding);
                return self.decode_pending(out);
            }
            // A GBK/Shift_JIS pair is almost never valid UTF-8, so this pins
            // UTF-8 correctly in practice.
            if std::str::from_utf8(&self.pending).is_ok() {
                self.pin(UTF_8);
                return self.decode_pending(out);
            }
            self.detector.feed(rest, false);
        }
        if self.pending.len() >= DETECT_THRESHOLD {
            self.pin(self.detector.guess(None, Utf8Detection::Allow));
            self.decode_pending(out);
        }
    }

    fn pin(&mut self, encoding: &'static Encoding) {
        self.decoder = Some(encoding.new_decoder_without_bom_handling());
    }

    /// Decode the bytes buffered while undecided.
    fn decode_pending(&mut self, out: &mut Vec<String>) {
        let bytes = std::mem::take(&mut self.pending);
        self.decode(&bytes, out);
    }

    fn decode(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        let decoder = self.decoder.as_mut().expect("pinned before decode");
        emit(decode_with(decoder, bytes, false), out);
    }
}

impl ChunkDecoder for StreamDecoder {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        if bytes.is_empty() {
            return;
        }
        if self.decoder.is_some() {
            self.decode(bytes, out);
        } else {
            self.undecided_feed(bytes, out);
        }
    }

    /// Flush buffered bytes and the decoder's carried partial sequence.
    fn flush(&mut self, out: &mut Vec<String>) {
        if self.decoder.is_none() {
            self.pin(self.detector.guess(None, Utf8Detection::Allow));
        }
        self.decode_pending(out);
        // Finalize an incomplete sequence held inside the decoder.
        let decoder = self.decoder.as_mut().expect("pinned above");
        emit(decode_with(decoder, b"", true), out);
    }
}

/// Incremental fixed-encoding decoder; a leading BOM may override the encoding.
pub(crate) struct FixedDecoder {
    decoder: Decoder,
    /// Leading bytes held while a BOM is still possible.
    pending: Vec<u8>,
    at_start: bool,
}

impl FixedDecoder {
    fn new(encoding: &'static Encoding) -> Self {
        Self {
            decoder: encoding.new_decoder_without_bom_handling(),
            pending: Vec::new(),
            at_start: true,
        }
    }

    /// Decode `bytes`, returning the text ready so far (possibly empty).
    fn feed(&mut self, bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        if !self.at_start {
            return decode_with(&mut self.decoder, bytes, false);
        }
        self.pending.extend_from_slice(bytes);
        if let Some((encoding, bom)) = Encoding::for_bom(&self.pending) {
            self.decoder = encoding.new_decoder_without_bom_handling();
            self.at_start = false;
            let pending = std::mem::take(&mut self.pending);
            return decode_with(&mut self.decoder, &pending[bom..], false);
        }
        if self.pending.len() >= 3 {
            self.at_start = false;
            let pending = std::mem::take(&mut self.pending);
            return decode_with(&mut self.decoder, &pending, false);
        }
        String::new()
    }

    /// Flush the bytes held for BOM sniffing and any partial sequence.
    fn finish(&mut self) -> String {
        let mut text = String::new();
        if self.at_start {
            self.at_start = false;
            let pending = std::mem::take(&mut self.pending);
            text.push_str(&decode_with(&mut self.decoder, &pending, false));
        }
        text.push_str(&decode_with(&mut self.decoder, &[], true));
        text
    }
}

/// UTF-8 output that switches once to `fallback` (the Windows console OEM
/// code page) when output is not valid UTF-8. The valid UTF-8 prefix of a
/// stream is kept, so a whole-buffer decode matches a chunked one.
pub(crate) struct AutoDecoder {
    fallback: &'static Encoding,
    /// Bytes not yet emitted, including an incomplete UTF-8 sequence.
    pending: Vec<u8>,
    legacy: Option<Decoder>,
    at_start: bool,
}

impl AutoDecoder {
    fn new(fallback: &'static Encoding) -> Self {
        Self {
            fallback,
            pending: Vec::new(),
            legacy: None,
            at_start: true,
        }
    }

    /// Decode `bytes`, returning the text ready so far (possibly empty).
    fn feed(&mut self, bytes: &[u8]) -> String {
        if let Some(decoder) = self.legacy.as_mut() {
            return decode_with(decoder, bytes, false);
        }
        if bytes.is_empty() {
            return String::new();
        }
        self.pending.extend_from_slice(bytes);
        if self.at_start {
            if let Some((encoding, bom)) = Encoding::for_bom(&self.pending) {
                self.pending.drain(..bom);
                self.at_start = false;
                if encoding != UTF_8 {
                    return self.switch(encoding);
                }
            } else if is_bom_prefix(&self.pending) {
                return String::new(); // a BOM may still arrive
            } else {
                self.at_start = false;
            }
        }
        match std::str::from_utf8(&self.pending) {
            Ok(text) => {
                let text = text.to_owned();
                self.pending.clear();
                text
            }
            Err(error) => {
                // Emit the valid prefix; only the bytes from the first error on
                // are undecided, so no valid UTF-8 is re-decoded as legacy.
                let valid = error.valid_up_to();
                let mut text = std::str::from_utf8(&self.pending[..valid])
                    .expect("valid_up_to ends on a UTF-8 boundary")
                    .to_owned();
                self.pending.drain(..valid);
                if error.error_len().is_none() {
                    return text; // an incomplete sequence may still complete
                }
                let fallback = self.fallback;
                text.push_str(&self.switch(fallback));
                text
            }
        }
    }

    /// Switch to `encoding`, decoding the bytes buffered so far.
    fn switch(&mut self, encoding: &'static Encoding) -> String {
        let mut decoder = encoding.new_decoder_without_bom_handling();
        let pending = std::mem::take(&mut self.pending);
        let text = decode_with(&mut decoder, &pending, false);
        self.legacy = Some(decoder);
        text
    }

    /// Flush buffered bytes and the decoder's carried partial sequence.
    fn finish(&mut self) -> String {
        if let Some(decoder) = self.legacy.as_mut() {
            return decode_with(decoder, &[], true);
        }
        let pending = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&pending).into_owned()
    }
}

/// Whether `bytes` is still a prefix of a BOM recognized by encoding_rs.
fn is_bom_prefix(bytes: &[u8]) -> bool {
    matches!(bytes, [0xEF] | [0xEF, 0xBB] | [0xFF] | [0xFE])
}

/// Incremental byte decoder shared by shell output and host-command pumps.
pub(crate) enum ByteDecoder {
    Fixed(FixedDecoder),
    Auto(AutoDecoder),
}

impl ByteDecoder {
    pub(crate) fn fixed(encoding: &'static Encoding) -> Self {
        Self::Fixed(FixedDecoder::new(encoding))
    }

    pub(crate) fn auto(fallback: &'static Encoding) -> Self {
        Self::Auto(AutoDecoder::new(fallback))
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> String {
        match self {
            Self::Fixed(decoder) => decoder.feed(bytes),
            Self::Auto(decoder) => decoder.feed(bytes),
        }
    }

    pub(crate) fn finish(&mut self) -> String {
        match self {
            Self::Fixed(decoder) => decoder.finish(),
            Self::Auto(decoder) => decoder.finish(),
        }
    }
}

/// A byte decoder plus the [`PlainText`] filter: what a terminal would show.
pub(crate) struct PlainTextDecoder {
    decoder: ByteDecoder,
    plain: PlainText,
}

impl PlainTextDecoder {
    pub(crate) fn new(encoding: &'static Encoding) -> Self {
        Self {
            decoder: ByteDecoder::fixed(encoding),
            plain: PlainText::new(),
        }
    }

    /// UTF-8 command output with a legacy fallback for native console tools.
    pub(crate) fn with_fallback(
        encoding: &'static Encoding,
        fallback: Option<&'static Encoding>,
    ) -> Self {
        match fallback {
            Some(fallback) if encoding == UTF_8 && fallback != UTF_8 => Self {
                decoder: ByteDecoder::auto(fallback),
                plain: PlainText::new(),
            },
            _ => Self::new(encoding),
        }
    }
}

impl ChunkDecoder for PlainTextDecoder {
    /// Decode `bytes`, appending plain-text chunks to `out`.
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        if let Some(clean) = self.plain.push(self.decoder.feed(bytes)) {
            out.push(clean);
        }
    }

    /// Flush buffered bytes, the decoder's carried partial sequence, and the
    /// line the filter holds.
    fn flush(&mut self, out: &mut Vec<String>) {
        if let Some(clean) = self.plain.push(self.decoder.finish()) {
            out.push(clean);
        }
        if let Some(held) = self.plain.finish() {
            out.push(held);
        }
    }
}

/// Append a decoded chunk unless it is empty.
fn emit(text: String, out: &mut Vec<String>) {
    if !text.is_empty() {
        out.push(text);
    }
}

/// Decode through the incremental decoder, growing the output as needed
/// (`decode_to_string` treats the `String`'s capacity as the output limit).
fn decode_with(decoder: &mut Decoder, bytes: &[u8], last: bool) -> String {
    let mut text = String::with_capacity(3 * bytes.len() + 16);
    let mut total = 0;
    loop {
        let (result, read, _) = decoder.decode_to_string(&bytes[total..], &mut text, last);
        total += read;
        match result {
            CoderResult::InputEmpty => break,
            CoderResult::OutputFull => text.reserve(3 * bytes.len() + 16),
        }
    }
    text
}
