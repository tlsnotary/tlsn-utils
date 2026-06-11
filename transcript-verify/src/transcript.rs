//! Zero-copy accessors over a validated transcript.
//!
//! Everything in this module borrows from the validated buffers and the span
//! table; every answer is backed by a span the validator verified. Nothing
//! here re-parses or searches the transcript beyond following the table as a
//! navigation index.
// TODO(P1/B3): remove this allow once the accessor bodies are implemented.
#![allow(dead_code)]

use alloc::vec::Vec;
use core::ops::Range;

use crate::spans::{
    BodySpans, Framing, HeaderSpan, JsonKind, JsonNode, RequestSpans, ResponseSpans, Span,
    SpanTable,
};

// === pub(crate) construction seam (FROZEN: the validator builds these) ===

/// The decoded payload of a validated body.
#[derive(Debug, Clone)]
pub(crate) enum BodyData<'a> {
    /// Body bytes borrowed directly from the source buffer
    /// ([`Framing::ContentLength`] and [`Framing::Close`] bodies).
    Borrowed(&'a [u8]),
    /// Body bytes decoded out of chunked framing ([`Framing::Chunked`]
    /// bodies).
    Decoded(Vec<u8>),
}

/// Maps one contiguous run of decoded body bytes back to the source buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ChunkMapEntry {
    /// Start of the run's data bytes, in source-buffer coordinates
    /// (absolute offset into `sent`/`recv`).
    pub(crate) src_start: u32,
    /// Start of the same bytes, in decoded-body coordinates.
    pub(crate) body_offset: u32,
    /// Number of bytes in the run.
    pub(crate) len: u32,
}

/// A body that passed framing (and, if claimed, JSON) validation.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedBody<'a> {
    /// The decoded body bytes.
    pub(crate) data: BodyData<'a>,
    /// Decoded-to-source map: one entry per non-empty data chunk, in order,
    /// covering the decoded body exactly.
    ///
    /// An empty map means identity/contiguous: decoded coordinate `i` lives
    /// at source byte `raw.start + i`. This is always the case for
    /// [`BodyData::Borrowed`] and is derived (never advised) for chunked
    /// bodies.
    pub(crate) chunk_map: Vec<ChunkMapEntry>,
}

/// A validated transcript: one HTTP/1.1 request (`sent`) and one response
/// (`recv`).
///
/// Obtained exclusively from [`validate`](crate::validate()); its existence
/// proves the span table is THE parse of the buffers. All accessors are
/// zero-copy except chunked body content, which borrows the decode buffer
/// owned by this struct.
#[derive(Debug, Clone)]
pub struct Transcript<'a> {
    /// The `sent` buffer.
    sent: &'a [u8],
    /// The `recv` buffer.
    recv: &'a [u8],
    /// The verified span table.
    table: &'a SpanTable,
    /// The verified status code, parsed from the status line.
    status: u16,
    /// The validated request body; `Some` iff `table.request.body` is.
    req_body: Option<ValidatedBody<'a>>,
    /// The validated response body; `Some` iff `table.response.body` is.
    resp_body: Option<ValidatedBody<'a>>,
}

impl<'a> Transcript<'a> {
    /// Construction seam for the validator — the only way to build a
    /// [`Transcript`].
    ///
    /// Contract: every argument must already be fully verified. `status` is
    /// the value parsed from the verified status-code span; `req_body` /
    /// `resp_body` must be `Some` exactly when the corresponding
    /// `table` body record is `Some`, and must hold the decoded bytes and
    /// derived chunk map produced by the body walk.
    pub(crate) fn new(
        sent: &'a [u8],
        recv: &'a [u8],
        table: &'a SpanTable,
        status: u16,
        req_body: Option<ValidatedBody<'a>>,
        resp_body: Option<ValidatedBody<'a>>,
    ) -> Self {
        Self {
            sent,
            recv,
            table,
            status,
            req_body,
            resp_body,
        }
    }

    /// Returns the validated request.
    pub fn request(&self) -> Request<'_> {
        todo!()
    }

    /// Returns the validated response.
    pub fn response(&self) -> Response<'_> {
        todo!()
    }
}

/// Accessor over the validated request.
#[derive(Debug, Clone, Copy)]
pub struct Request<'t> {
    /// The `sent` buffer.
    buf: &'t [u8],
    /// The verified request spans.
    spans: &'t RequestSpans,
    /// The validated body, present iff `spans.body` is.
    body: Option<&'t ValidatedBody<'t>>,
}

impl<'t> Request<'t> {
    /// Returns the request method.
    ///
    /// Guaranteed to be a nonempty RFC 9110 token (`tchar+`) starting at
    /// byte 0 of `sent` and followed by a single SP (rule B1) — always valid
    /// ASCII.
    pub fn method(&self) -> &'t str {
        todo!()
    }

    /// Returns the request target.
    ///
    /// Guaranteed nonempty printable ASCII (0x21..=0x7E) followed by the
    /// literal `` HTTP/1.1\r\n`` (rules B2/B3); otherwise opaque — this
    /// crate does not parse URIs.
    pub fn target(&self) -> &'t str {
        todo!()
    }

    /// Returns an iterator over all headers, in order of appearance.
    ///
    /// The header records are guaranteed to be in bijection with the header
    /// lines in the bytes (rule B7).
    pub fn headers(&self) -> Headers<'t> {
        todo!()
    }

    /// Returns the first header whose name matches `name`
    /// case-insensitively (ASCII), or `None`.
    ///
    /// Duplicate names are allowed in general (first match wins), but
    /// duplicate `Content-Length`, `Transfer-Encoding`, and `Host` were
    /// rejected during validation (rule B8). Chunked trailers are a separate
    /// namespace and are never returned here (rule G4).
    pub fn header(&self, _name: &str) -> Option<Header<'t>> {
        todo!()
    }

    /// Returns an iterator over every header whose name matches `name`
    /// case-insensitively (ASCII), in order of appearance.
    pub fn headers_with_name<'n>(&self, _name: &'n str) -> HeadersWithName<'t, 'n> {
        todo!()
    }

    /// Returns the request body, or `None` if the message has none.
    ///
    /// Present iff the framing derived from the verified head yields body
    /// bytes (e.g. `Content-Length: 0` and absent framing both yield
    /// `None`).
    pub fn body(&self) -> Option<Body<'t>> {
        todo!()
    }
}

/// Accessor over the validated response.
#[derive(Debug, Clone, Copy)]
pub struct Response<'t> {
    /// The `recv` buffer.
    buf: &'t [u8],
    /// The verified response spans.
    spans: &'t ResponseSpans,
    /// The verified status code.
    status: u16,
    /// The validated body, present iff `spans.body` is.
    body: Option<&'t ValidatedBody<'t>>,
}

impl<'t> Response<'t> {
    /// Returns the status code.
    ///
    /// Guaranteed parsed from exactly 3 DIGITs pinned at bytes `[9, 12)` of
    /// `recv`, with the first digit in `1..=5`, i.e. in `100..=599`
    /// (rule D1).
    pub fn status(&self) -> u16 {
        todo!()
    }

    /// Returns the reason phrase.
    ///
    /// Possibly empty; untrimmed; guaranteed printable ASCII plus SP/HTAB
    /// (rule D1).
    pub fn reason(&self) -> &'t str {
        todo!()
    }

    /// Returns an iterator over all headers, in order of appearance (same
    /// guarantees as [`Request::headers`]).
    pub fn headers(&self) -> Headers<'t> {
        todo!()
    }

    /// Returns the first header whose name matches `name`
    /// case-insensitively (ASCII), or `None` (same guarantees as
    /// [`Request::header`]).
    pub fn header(&self, _name: &str) -> Option<Header<'t>> {
        todo!()
    }

    /// Returns an iterator over every header whose name matches `name`
    /// case-insensitively (ASCII), in order of appearance.
    pub fn headers_with_name<'n>(&self, _name: &'n str) -> HeadersWithName<'t, 'n> {
        todo!()
    }

    /// Returns the response body, or `None` if the message has none.
    ///
    /// Present iff the derived framing yields body bytes; responses to HEAD
    /// requests and 1xx/204/304 statuses never have one (rule D3).
    pub fn body(&self) -> Option<Body<'t>> {
        todo!()
    }
}

/// One verified header (or chunked-trailer) line.
#[derive(Debug, Clone, Copy)]
pub struct Header<'t> {
    /// The source buffer the spans index into.
    buf: &'t [u8],
    /// The verified name/value spans.
    span: HeaderSpan,
}

impl<'t> Header<'t> {
    /// Returns the header name.
    ///
    /// Guaranteed a nonempty RFC 9110 token immediately followed by `:` in
    /// the source (rule B4) — always valid ASCII.
    pub fn name(&self) -> &'t str {
        todo!()
    }

    /// Returns the header value, OWS-trimmed on both sides (rule B5).
    ///
    /// May be empty. Returned as bytes because values may legally contain
    /// obs-text (0x80..=0xFF); CR/LF/NUL/DEL can never appear.
    pub fn value(&self) -> &'t [u8] {
        todo!()
    }

    /// Returns the header value as a string, or `None` if it is not valid
    /// UTF-8.
    pub fn value_str(&self) -> Option<&'t str> {
        todo!()
    }

    /// Returns the name span, in source coordinates.
    pub fn name_span(&self) -> Span {
        todo!()
    }

    /// Returns the value span, in source coordinates (an empty value is
    /// pinned at the terminating CR).
    pub fn value_span(&self) -> Span {
        todo!()
    }
}

/// Accessor over one validated message body.
#[derive(Debug, Clone, Copy)]
pub struct Body<'t> {
    /// The source buffer the body came from.
    buf: &'t [u8],
    /// The verified body spans.
    spans: &'t BodySpans,
    /// The validated (decoded) body data.
    data: &'t ValidatedBody<'t>,
}

impl<'t> Body<'t> {
    /// Returns the decoded body content.
    ///
    /// Zero-copy (borrowed from the source buffer) unless the body was
    /// chunked, in which case it borrows the de-chunk buffer owned by the
    /// [`Transcript`]. Guaranteed exactly `content_len` bytes, equal to the
    /// verified decode of the raw body section (rule group E).
    pub fn content(&self) -> &'t [u8] {
        todo!()
    }

    /// Returns the verified framing of this body.
    pub fn framing(&self) -> Framing {
        todo!()
    }

    /// Returns the full source extent of the body section
    /// (`head_end..message_end`), in source coordinates. For chunked bodies
    /// this includes size lines, CRLFs, the terminal chunk, and any
    /// trailers.
    pub fn raw_span(&self) -> Span {
        todo!()
    }

    /// Returns an iterator over the chunked trailers, in order of
    /// appearance. Empty for non-chunked bodies.
    ///
    /// Trailers are a separate namespace from headers (rule G4); names
    /// `Content-Length`, `Transfer-Encoding`, and `Host` were rejected
    /// during validation (rule C7).
    pub fn trailers(&self) -> Headers<'t> {
        todo!()
    }

    /// Returns the first trailer whose name matches `name`
    /// case-insensitively (ASCII), or `None`.
    pub fn trailer(&self, _name: &str) -> Option<Header<'t>> {
        todo!()
    }

    /// Returns the verified JSON view of the decoded body.
    ///
    /// `Some` iff the table claimed JSON AND validation verified the node
    /// tree (a claimed-but-invalid tree fails [`validate`] as a whole). The
    /// claim is deliberately independent of `Content-Type` (rule G1).
    ///
    /// [`validate`]: crate::validate()
    pub fn json(&self) -> Option<JsonValue<'t>> {
        todo!()
    }

    /// Maps a range of DECODED-body coordinates to the source spans backing
    /// it, for selective disclosure.
    ///
    /// Returns one span for contiguous (`ContentLength`/`Close`) bodies and
    /// one span per straddled chunk for chunked bodies, in order; an empty
    /// range yields an empty `Vec`.
    ///
    /// # Panics
    ///
    /// Panics if `range.start > range.end` or `range.end` exceeds the
    /// decoded body length.
    pub fn content_to_source(&self, _range: Range<u32>) -> Vec<Span> {
        todo!()
    }
}

/// A verified JSON value inside a decoded body.
///
/// All spans and offsets are in DECODED-body coordinates; use
/// [`Body::content_to_source`] to map back to wire bytes.
#[derive(Debug, Clone, Copy)]
pub struct JsonValue<'t> {
    /// The decoded body content.
    content: &'t [u8],
    /// The full pre-order node table.
    nodes: &'t [JsonNode],
    /// Index of this value's node in `nodes`.
    idx: usize,
}

impl<'t> JsonValue<'t> {
    /// Returns the value's kind. Never [`JsonKind::Key`] — keys are only
    /// yielded by [`iter_object`](Self::iter_object).
    ///
    /// The kind was verified against the value's first byte, which admits
    /// exactly one kind (rule F2).
    pub fn kind(&self) -> JsonKind {
        todo!()
    }

    /// Looks up a descendant value by spansy-style dot path, e.g.
    /// `"a.b.1"`.
    ///
    /// Path segments name object keys (matched against the RAW key bytes —
    /// escaped keys and keys containing `.` are unaddressable; use
    /// [`iter_object`](Self::iter_object) for those) or decimal array
    /// indices. Returns `None` for any missing segment or kind mismatch;
    /// never panics.
    pub fn get(&self, _path: &str) -> Option<JsonValue<'t>> {
        todo!()
    }

    /// Returns the string content between the quotes, or `None` if this is
    /// not a string.
    ///
    /// Escapes are left raw (not decoded); guaranteed valid UTF-8 and
    /// RFC 8259 string grammar (rules F0/F6).
    pub fn as_str(&self) -> Option<&'t str> {
        todo!()
    }

    /// Returns the exact number lexeme, or `None` if this is not a number.
    ///
    /// Guaranteed to match the RFC 8259 number grammar exactly (rule F7);
    /// numeric conversion is left to the caller.
    pub fn as_number_str(&self) -> Option<&'t str> {
        todo!()
    }

    /// Returns the boolean value, or `None` if this is not a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        todo!()
    }

    /// Returns `true` iff this value is `null`.
    pub fn is_null(&self) -> bool {
        todo!()
    }

    /// Returns this value's span, in DECODED-body coordinates (string
    /// content excludes the quotes; containers span `{`..`}` / `[`..`]`
    /// inclusive).
    pub fn span(&self) -> Span {
        todo!()
    }

    /// Returns the number of elements, or `None` if this is not an array.
    ///
    /// O(number of elements), via size-jumps.
    pub fn array_len(&self) -> Option<u32> {
        todo!()
    }

    /// Returns an iterator over the array's elements, or `None` if this is
    /// not an array. Iteration is O(1) per element via size-jumps.
    pub fn iter_array(&self) -> Option<JsonArrayIter<'t>> {
        todo!()
    }

    /// Returns an iterator over the object's members in document order, or
    /// `None` if this is not an object.
    ///
    /// Duplicate keys were rejected during validation (rule F9), so each
    /// key is unique under decoded comparison.
    pub fn iter_object(&self) -> Option<JsonObjectIter<'t>> {
        todo!()
    }
}

/// A verified object-member key.
#[derive(Debug, Clone, Copy)]
pub struct JsonKey<'t> {
    /// The decoded body content.
    content: &'t [u8],
    /// The key's node.
    node: JsonNode,
}

impl<'t> JsonKey<'t> {
    /// Returns the key content between the quotes.
    ///
    /// Escapes are left raw (not decoded); guaranteed valid UTF-8 and
    /// RFC 8259 string grammar.
    pub fn as_str(&self) -> &'t str {
        todo!()
    }

    /// Returns the key's content span (quotes excluded), in DECODED-body
    /// coordinates.
    pub fn span(&self) -> Span {
        todo!()
    }
}

/// Iterator over a message's headers (or a body's trailers), in order of
/// appearance. Created by [`Request::headers`], [`Response::headers`], and
/// [`Body::trailers`].
#[derive(Debug, Clone)]
pub struct Headers<'t> {
    /// The source buffer the spans index into.
    buf: &'t [u8],
    /// Remaining header records.
    spans: core::slice::Iter<'t, HeaderSpan>,
}

impl<'t> Iterator for Headers<'t> {
    type Item = Header<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

/// Iterator over the headers matching one name case-insensitively (ASCII),
/// in order of appearance. Created by [`Request::headers_with_name`] and
/// [`Response::headers_with_name`].
#[derive(Debug, Clone)]
pub struct HeadersWithName<'t, 'n> {
    /// The underlying header iterator.
    inner: Headers<'t>,
    /// The name to match (ASCII case-insensitive).
    name: &'n str,
}

impl<'t, 'n> Iterator for HeadersWithName<'t, 'n> {
    type Item = Header<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

/// Iterator over a JSON array's elements. Created by
/// [`JsonValue::iter_array`].
#[derive(Debug, Clone)]
pub struct JsonArrayIter<'t> {
    /// The decoded body content.
    content: &'t [u8],
    /// The full pre-order node table.
    nodes: &'t [JsonNode],
    /// Node index of the next element's subtree root.
    next: usize,
    /// One past the last node index of the array's subtree.
    end: usize,
}

impl<'t> Iterator for JsonArrayIter<'t> {
    type Item = JsonValue<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

/// Iterator over a JSON object's members in document order. Created by
/// [`JsonValue::iter_object`].
#[derive(Debug, Clone)]
pub struct JsonObjectIter<'t> {
    /// The decoded body content.
    content: &'t [u8],
    /// The full pre-order node table.
    nodes: &'t [JsonNode],
    /// Node index of the next member's key node.
    next: usize,
    /// One past the last node index of the object's subtree.
    end: usize,
}

impl<'t> Iterator for JsonObjectIter<'t> {
    type Item = (JsonKey<'t>, JsonValue<'t>);

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}
