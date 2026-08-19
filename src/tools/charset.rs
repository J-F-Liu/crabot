//! Charset-aware decoding for shell-tool output, replacing the lossy UTF-8
//! assumption that garbles legacy encodings (GBK on Chinese Windows,
//! Shift_JIS, windows-1252, …).
//!
//! [`decode_bytes`] decodes a whole buffer; [`StreamDecoder`] is the
//! incremental variant for live streaming. Both honor a leading UTF-8/UTF-16
//! BOM, pass valid UTF-8 through losslessly, and fall back to a `chardetng`
//! guess decoded via `encoding_rs`.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8};

/// Buffered bytes that force an encoding guess (a GBK/Shift_JIS pair is
/// almost never valid UTF-8, so in practice the guess fires on the first
/// non-ASCII byte).
const DETECT_THRESHOLD: usize = 1024;

/// Decode a whole buffer: honor a leading BOM, pass valid UTF-8 through,
/// else detect the charset and decode with it.
pub(crate) fn decode_bytes(bytes: &[u8]) -> String {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom..]);
        return text.into_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Allow);
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

/// Incremental decoder for live streaming: ASCII passes through immediately,
/// the charset is pinned by a BOM, valid UTF-8, or a detector guess, and
/// `\r\n` is normalized to `\n`.
pub(crate) struct StreamDecoder {
    /// Pinned decoder; `None` while the encoding is still undecided.
    decoder: Option<Decoder>,
    /// Bytes buffered while undecided.
    pending: Vec<u8>,
    detector: EncodingDetector,
    /// BOM sniffing applies only before the first emitted byte.
    at_start: bool,
    pending_cr: bool,
}

impl StreamDecoder {
    pub(crate) fn new() -> Self {
        Self {
            decoder: None,
            pending: Vec::new(),
            detector: EncodingDetector::new(Iso2022JpDetection::Deny),
            at_start: true,
            pending_cr: false,
        }
    }

    /// Decode `bytes`, appending normalized text chunks to `out`.
    pub(crate) fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        if bytes.is_empty() {
            return;
        }
        if self.decoder.is_some() {
            self.decode(bytes, out);
        } else {
            self.undecided_feed(bytes, out);
        }
    }

    /// Flush a carried incomplete sequence and a pending trailing `\r`.
    pub(crate) fn flush(&mut self, out: &mut Vec<String>) {
        if !self.pending.is_empty() {
            let encoding = self.detector.guess(None, Utf8Detection::Allow);
            self.pin(encoding);
            self.decode_pending(out);
        }
        if let Some(decoder) = self.decoder.as_mut() {
            // Finalize an incomplete sequence held inside the decoder.
            let text = decode_with(decoder, b"", true);
            if !text.is_empty() {
                self.push_normalized(&text, out);
            }
        }
        if self.pending_cr {
            self.pending_cr = false;
            out.push("\r".into());
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
            self.push_normalized(unsafe { std::str::from_utf8_unchecked(prefix) }, out);
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
            let encoding = self.detector.guess(None, Utf8Detection::Allow);
            self.pin(encoding);
            self.decode_pending(out);
        }
    }

    fn pin(&mut self, encoding: &'static Encoding) {
        self.decoder = Some(encoding.new_decoder_without_bom_handling());
    }

    /// Decode the buffered bytes with the pinned encoding.
    fn decode_pending(&mut self, out: &mut Vec<String>) {
        let bytes = std::mem::take(&mut self.pending);
        self.decode(&bytes, out);
    }

    fn decode(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        let decoder = self.decoder.as_mut().expect("pinned before decode");
        let text = decode_with(decoder, bytes, false);
        if !text.is_empty() {
            self.push_normalized(&text, out);
        }
    }

    /// Normalize `\r\n` → `\n` (carrying a trailing `\r`), then emit the chunk.
    fn push_normalized(&mut self, text: &str, out: &mut Vec<String>) {
        let mut normalized = String::with_capacity(text.len() + 1);
        let mut chars = text.chars().peekable();
        if self.pending_cr {
            self.pending_cr = false;
            match chars.peek() {
                Some('\n') => {
                    chars.next(); // `\r\n` split across chunks
                    normalized.push('\n');
                }
                Some(_) => normalized.push('\r'),
                None => {
                    self.pending_cr = true;
                    return;
                }
            }
        }
        while let Some(c) = chars.next() {
            if c == '\r' {
                match chars.peek() {
                    Some('\n') => {
                        chars.next();
                        normalized.push('\n');
                    }
                    Some(_) => normalized.push('\r'),
                    None => self.pending_cr = true,
                }
            } else {
                normalized.push(c);
            }
        }
        if !normalized.is_empty() {
            out.push(normalized);
        }
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
