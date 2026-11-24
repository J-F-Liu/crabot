//! Decoding for tool input and output.
//!
//! Command output uses a fixed encoding: [`PlainTextDecoder`] pins it and adds
//! the terminal filter of [`super::ansi`]. Arbitrary data (a fetched body) uses
//! [`decode_bytes`], which still detects legacy charsets.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8};

use super::ansi::PlainText;

/// Buffered bytes that force an encoding guess (a GBK/Shift_JIS pair is
/// almost never valid UTF-8, so in practice the guess fires on the first
/// non-ASCII byte).
const DETECT_THRESHOLD: usize = 1024;

/// Decode a whole buffer into text: [`StreamDecoder`] fed in one go. Escapes
/// and control characters survive — use [`decode_plain`] for shell output.
pub(crate) fn decode_bytes(bytes: &[u8]) -> String {
    decode_all(&mut StreamDecoder::new(), bytes)
}

/// Decode a whole buffer into plain text with `encoding`: [`PlainTextDecoder`]
/// fed in one go.
pub(crate) fn decode_plain(bytes: &[u8], encoding: &'static Encoding) -> String {
    decode_all(&mut PlainTextDecoder::new(encoding), bytes)
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
            detector: EncodingDetector::new(Iso2022JpDetection::Deny),
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
    pub(crate) fn new(encoding: &'static Encoding) -> Self {
        Self {
            decoder: encoding.new_decoder_without_bom_handling(),
            pending: Vec::new(),
            at_start: true,
        }
    }

    /// Decode `bytes`, returning the text ready so far (possibly empty).
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> String {
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
    pub(crate) fn finish(&mut self) -> String {
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

/// [`FixedDecoder`] plus the [`PlainText`] filter: what a terminal would show.
pub(crate) struct PlainTextDecoder {
    decoder: FixedDecoder,
    plain: PlainText,
}

impl PlainTextDecoder {
    pub(crate) fn new(encoding: &'static Encoding) -> Self {
        Self {
            decoder: FixedDecoder::new(encoding),
            plain: PlainText::new(),
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
