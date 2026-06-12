//! Zero-copy accessors over a validated transcript.
//!
//! Everything in this module borrows from the validated buffers and the span
//! table; every answer is backed by a span the validator verified. Nothing
//! here re-parses or searches the transcript beyond following the table as a
//! navigation index.
//!
//! All accessors are panic-free by construction: every span is applied with
//! guarded slicing, and the failure paths — unreachable for a table that
//! passed validation — degrade to empty slices, `""`, or `None` instead of
//! panicking.

use alloc::vec::Vec;
use core::ops::Range;

use crate::spans::{
    BodySpans, Framing, HeaderSpan, JsonKind, JsonNode, RequestSpans, ResponseSpans, Span,
    SpanTable,
};

/// Returns the bytes `span` covers in `buf`.
///
/// Falls back to the empty slice if the span is inverted or out of bounds —
/// impossible for a validated table (rule group A); the guard keeps every
/// accessor panic-free by construction.
fn slice_span(buf: &[u8], span: Span) -> &[u8] {
    buf.get(span.as_range()).unwrap_or(&[])
}

/// Returns the bytes `span` covers in `buf` as UTF-8.
///
/// Falls back to `""` if the span is out of bounds or the bytes are not
/// valid UTF-8. Used only for spans whose validated charset implies UTF-8
/// (ASCII tokens and targets), plus the reason phrase, whose documented
/// fallback this implements.
fn str_span(buf: &[u8], span: Span) -> &str {
    core::str::from_utf8(slice_span(buf, span)).unwrap_or("")
}

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
        Request {
            buf: self.sent,
            spans: &self.table.request,
            body: self.req_body.as_ref(),
        }
    }

    /// Returns the validated response.
    pub fn response(&self) -> Response<'_> {
        Response {
            buf: self.recv,
            spans: &self.table.response,
            status: self.status,
            body: self.resp_body.as_ref(),
        }
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
        str_span(self.buf, self.spans.method)
    }

    /// Returns the request target.
    ///
    /// Guaranteed nonempty printable ASCII (0x21..=0x7E) followed by the
    /// literal `` HTTP/1.1\r\n`` (rules B2/B3); otherwise opaque — this
    /// crate does not parse URIs.
    pub fn target(&self) -> &'t str {
        str_span(self.buf, self.spans.target)
    }

    /// Returns an iterator over all headers, in order of appearance.
    ///
    /// The header records are guaranteed to be in bijection with the header
    /// lines in the bytes (rule B7).
    pub fn headers(&self) -> Headers<'t> {
        Headers {
            buf: self.buf,
            spans: self.spans.headers.iter(),
        }
    }

    /// Returns the first header whose name matches `name`
    /// case-insensitively (ASCII), or `None`.
    ///
    /// Duplicate names are allowed in general (first match wins), but
    /// duplicate `Content-Length`, `Transfer-Encoding`, and `Host` were
    /// rejected during validation (rule B8). Chunked trailers are a separate
    /// namespace and are never returned here (rule G4).
    pub fn header(&self, name: &str) -> Option<Header<'t>> {
        self.headers_with_name(name).next()
    }

    /// Returns an iterator over every header whose name matches `name`
    /// case-insensitively (ASCII), in order of appearance.
    pub fn headers_with_name<'n>(&self, name: &'n str) -> HeadersWithName<'t, 'n> {
        HeadersWithName {
            inner: self.headers(),
            name,
        }
    }

    /// Returns the request body, or `None` if the message has none.
    ///
    /// Present iff the framing derived from the verified head yields body
    /// bytes (e.g. `Content-Length: 0` and absent framing both yield
    /// `None`).
    pub fn body(&self) -> Option<Body<'t>> {
        Some(Body {
            buf: self.buf,
            spans: self.spans.body.as_ref()?,
            data: self.body?,
        })
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
        self.status
    }

    /// Returns the reason phrase.
    ///
    /// Possibly empty; untrimmed; charset-checked during validation (rule
    /// D1). The reason-phrase charset may admit obs-text bytes
    /// (0x80..=0xFF), which need not form valid UTF-8 — if the verified
    /// span is not valid UTF-8 this returns `""` rather than panicking. For
    /// a possibly-non-UTF-8 reason, use [`reason_bytes`](Self::reason_bytes).
    pub fn reason(&self) -> &'t str {
        str_span(self.buf, self.spans.reason)
    }

    /// Returns the raw reason-phrase bytes.
    ///
    /// The verified span exactly (rule D1), returned as bytes because the
    /// reason-phrase charset may contain obs-text (0x80..=0xFF) that need
    /// not form valid UTF-8 — analogous to [`Header::value`]. Possibly
    /// empty; untrimmed. Lossless companion to [`reason`](Self::reason).
    pub fn reason_bytes(&self) -> &'t [u8] {
        slice_span(self.buf, self.spans.reason)
    }

    /// Returns an iterator over all headers, in order of appearance (same
    /// guarantees as [`Request::headers`]).
    pub fn headers(&self) -> Headers<'t> {
        Headers {
            buf: self.buf,
            spans: self.spans.headers.iter(),
        }
    }

    /// Returns the first header whose name matches `name`
    /// case-insensitively (ASCII), or `None` (same guarantees as
    /// [`Request::header`]).
    pub fn header(&self, name: &str) -> Option<Header<'t>> {
        self.headers_with_name(name).next()
    }

    /// Returns an iterator over every header whose name matches `name`
    /// case-insensitively (ASCII), in order of appearance.
    pub fn headers_with_name<'n>(&self, name: &'n str) -> HeadersWithName<'t, 'n> {
        HeadersWithName {
            inner: self.headers(),
            name,
        }
    }

    /// Returns the response body, or `None` if the message has none.
    ///
    /// Present iff the derived framing yields body bytes; responses to HEAD
    /// requests and 1xx/204/304 statuses never have one (rule D3).
    pub fn body(&self) -> Option<Body<'t>> {
        Some(Body {
            buf: self.buf,
            spans: self.spans.body.as_ref()?,
            data: self.body?,
        })
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
        str_span(self.buf, self.span.name)
    }

    /// Returns the header value, OWS-trimmed on both sides (rule B5).
    ///
    /// May be empty. Returned as bytes because values may legally contain
    /// obs-text (0x80..=0xFF); CR/LF/NUL/DEL can never appear.
    pub fn value(&self) -> &'t [u8] {
        slice_span(self.buf, self.span.value)
    }

    /// Returns the header value as a string, or `None` if it is not valid
    /// UTF-8.
    pub fn value_str(&self) -> Option<&'t str> {
        core::str::from_utf8(self.value()).ok()
    }

    /// Returns the name span, in source coordinates.
    pub fn name_span(&self) -> Span {
        self.span.name
    }

    /// Returns the value span, in source coordinates (an empty value is
    /// pinned at the terminating CR).
    pub fn value_span(&self) -> Span {
        self.span.value
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
        match &self.data.data {
            BodyData::Borrowed(bytes) => bytes,
            BodyData::Decoded(bytes) => bytes.as_slice(),
        }
    }

    /// Returns the verified framing of this body.
    pub fn framing(&self) -> Framing {
        self.spans.framing
    }

    /// Returns the full source extent of the body section
    /// (`head_end..message_end`), in source coordinates. For chunked bodies
    /// this includes size lines, CRLFs, the terminal chunk, and any
    /// trailers.
    pub fn raw_span(&self) -> Span {
        self.spans.raw
    }

    /// Returns an iterator over the chunked trailers, in order of
    /// appearance. Empty for non-chunked bodies.
    ///
    /// Trailers are a separate namespace from headers (rule G4); names
    /// `Content-Length`, `Transfer-Encoding`, and `Host` were rejected
    /// during validation (rule C7).
    pub fn trailers(&self) -> Headers<'t> {
        Headers {
            buf: self.buf,
            spans: self.spans.trailers.iter(),
        }
    }

    /// Returns the first trailer whose name matches `name`
    /// case-insensitively (ASCII), or `None`.
    pub fn trailer(&self, name: &str) -> Option<Header<'t>> {
        self.trailers()
            .find(|trailer| trailer.name().eq_ignore_ascii_case(name))
    }

    /// Returns the verified JSON view of the decoded body.
    ///
    /// `Some` iff the table claimed JSON AND validation verified the node
    /// tree (a claimed-but-invalid tree fails [`validate`] as a whole). The
    /// claim is deliberately independent of `Content-Type` (rule G1).
    ///
    /// [`validate`]: crate::validate()
    pub fn json(&self) -> Option<JsonValue<'t>> {
        let nodes = self.spans.json.as_ref()?.nodes.as_slice();
        if nodes.is_empty() {
            // A validated claim always has a root node; guard regardless.
            return None;
        }
        Some(JsonValue {
            content: self.content(),
            nodes,
            idx: 0,
        })
    }

    /// Maps a range of DECODED-body coordinates to the source spans backing
    /// it, for selective disclosure.
    ///
    /// Returns one span for contiguous (`ContentLength`/`Close`) bodies and
    /// one span per straddled chunk for chunked bodies, in order; together
    /// the returned spans cover exactly the requested bytes as they appear
    /// on the wire.
    ///
    /// An empty range, an inverted range (`start > end`), or a range
    /// reaching past the decoded body length yields an empty `Vec` — this
    /// accessor never panics.
    pub fn content_to_source(&self, range: Range<u32>) -> Vec<Span> {
        // The decoded length fits u32: validation caps it at 2^30.
        let content_len = u32::try_from(self.content().len()).unwrap_or(u32::MAX);
        if range.start >= range.end || range.end > content_len {
            return Vec::new();
        }
        if self.data.chunk_map.is_empty() {
            // Identity mapping: decoded coordinate `i` lives at source byte
            // `raw.start + i`.
            let base = self.spans.raw.start;
            return alloc::vec![Span::new(
                base.saturating_add(range.start),
                base.saturating_add(range.end),
            )];
        }
        let mut spans = Vec::new();
        for entry in &self.data.chunk_map {
            // Entries are sorted by `body_offset`.
            if entry.body_offset >= range.end {
                break;
            }
            let chunk_end = entry.body_offset.saturating_add(entry.len);
            let overlap_start = range.start.max(entry.body_offset);
            let overlap_end = range.end.min(chunk_end);
            if overlap_start < overlap_end {
                spans.push(Span::new(
                    entry
                        .src_start
                        .saturating_add(overlap_start - entry.body_offset),
                    entry
                        .src_start
                        .saturating_add(overlap_end - entry.body_offset),
                ));
            }
        }
        spans
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
    /// Returns this value's node.
    ///
    /// `None` only if `idx` is out of bounds, which cannot happen for
    /// values produced by this module (the root is checked at creation and
    /// every navigation step is bounds-checked).
    fn node(&self) -> Option<JsonNode> {
        self.nodes.get(self.idx).copied()
    }

    /// Returns the bytes of this value's span in the decoded body.
    fn content_bytes(&self) -> &'t [u8] {
        slice_span(self.content, self.span())
    }

    /// Resolves one path segment against this value (object key by raw
    /// bytes, or decimal array index).
    fn step(&self, segment: &str) -> Option<JsonValue<'t>> {
        match self.kind() {
            JsonKind::Object => self
                .iter_object()?
                .find(|(key, _)| key.raw() == segment.as_bytes())
                .map(|(_, value)| value),
            JsonKind::Array => self.iter_array()?.nth(segment.parse::<usize>().ok()?),
            _ => None,
        }
    }

    /// Returns the value's kind. Never [`JsonKind::Key`] — keys are only
    /// yielded by [`iter_object`](Self::iter_object).
    ///
    /// The kind was verified against the value's first byte, which admits
    /// exactly one kind (rule F2).
    pub fn kind(&self) -> JsonKind {
        self.node().map_or(JsonKind::Null, |node| node.kind)
    }

    /// Looks up a descendant value by spansy-style dot path, e.g.
    /// `"a.b.1"`.
    ///
    /// Path segments name object keys (matched against the RAW key bytes —
    /// escaped keys and keys containing `.` are unaddressable; use
    /// [`iter_object`](Self::iter_object) for those) or decimal array
    /// indices. Returns `None` for any missing segment or kind mismatch;
    /// never panics.
    pub fn get(&self, path: &str) -> Option<JsonValue<'t>> {
        let mut value = *self;
        for segment in path.split('.') {
            value = value.step(segment)?;
        }
        Some(value)
    }

    /// Returns the string content between the quotes, or `None` if this is
    /// not a string.
    ///
    /// Escapes are left raw (not decoded); guaranteed valid UTF-8 and
    /// RFC 8259 string grammar (rules F0/F6).
    pub fn as_str(&self) -> Option<&'t str> {
        if self.kind() != JsonKind::String {
            return None;
        }
        core::str::from_utf8(self.content_bytes()).ok()
    }

    /// Returns the exact number lexeme, or `None` if this is not a number.
    ///
    /// Guaranteed to match the RFC 8259 number grammar exactly (rule F7);
    /// numeric conversion is left to the caller.
    pub fn as_number_str(&self) -> Option<&'t str> {
        if self.kind() != JsonKind::Number {
            return None;
        }
        core::str::from_utf8(self.content_bytes()).ok()
    }

    /// Returns the boolean value, or `None` if this is not a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        if self.kind() != JsonKind::Bool {
            return None;
        }
        // The literal is verified to be exactly `true` or `false` (rule
        // F8), so the first byte decides.
        self.content_bytes().first().map(|&byte| byte == b't')
    }

    /// Returns `true` iff this value is `null`.
    pub fn is_null(&self) -> bool {
        self.node().is_some_and(|node| node.kind == JsonKind::Null)
    }

    /// Returns this value's span, in DECODED-body coordinates (string
    /// content excludes the quotes; containers span `{`..`}` / `[`..`]`
    /// inclusive).
    pub fn span(&self) -> Span {
        self.node()
            .map_or(Span::new(0, 0), |node| Span::new(node.start, node.end))
    }

    /// Returns the number of elements, or `None` if this is not an array.
    ///
    /// O(number of elements), via size-jumps.
    pub fn array_len(&self) -> Option<u32> {
        let count = self.iter_array()?.count();
        // A validated tree has at most 2^30 nodes, so this cannot truncate;
        // saturate defensively anyway.
        Some(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Returns an iterator over the array's elements, or `None` if this is
    /// not an array. Iteration is O(1) per element via size-jumps.
    pub fn iter_array(&self) -> Option<JsonArrayIter<'t>> {
        let node = self.node()?;
        if node.kind != JsonKind::Array {
            return None;
        }
        Some(JsonArrayIter {
            content: self.content,
            nodes: self.nodes,
            next: self.idx.saturating_add(1),
            end: self
                .idx
                .saturating_add(node.size as usize)
                .min(self.nodes.len()),
        })
    }

    /// Returns an iterator over the object's members in document order, or
    /// `None` if this is not an object.
    ///
    /// Duplicate keys were rejected during validation (rule F9), so each
    /// key is unique under decoded comparison.
    pub fn iter_object(&self) -> Option<JsonObjectIter<'t>> {
        let node = self.node()?;
        if node.kind != JsonKind::Object {
            return None;
        }
        Some(JsonObjectIter {
            content: self.content,
            nodes: self.nodes,
            next: self.idx.saturating_add(1),
            end: self
                .idx
                .saturating_add(node.size as usize)
                .min(self.nodes.len()),
        })
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
    /// Returns the raw content bytes between the quotes.
    fn raw(&self) -> &'t [u8] {
        slice_span(self.content, Span::new(self.node.start, self.node.end))
    }

    /// Returns the key content between the quotes.
    ///
    /// Escapes are left raw (not decoded); guaranteed valid UTF-8 and
    /// RFC 8259 string grammar.
    pub fn as_str(&self) -> &'t str {
        core::str::from_utf8(self.raw()).unwrap_or("")
    }

    /// Returns the key's content span (quotes excluded), in DECODED-body
    /// coordinates.
    pub fn span(&self) -> Span {
        Span::new(self.node.start, self.node.end)
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
        let span = *self.spans.next()?;
        Some(Header {
            buf: self.buf,
            span,
        })
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
        let name = self.name;
        self.inner
            .find(|header| header.name().eq_ignore_ascii_case(name))
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
        if self.next >= self.end {
            return None;
        }
        let idx = self.next;
        // A validated tree always has `size >= 1`; the clamp ensures even a
        // malformed table cannot stall the iterator.
        let size = self
            .nodes
            .get(idx)
            .map_or(1, |node| node.size.max(1) as usize);
        self.next = idx.saturating_add(size);
        Some(JsonValue {
            content: self.content,
            nodes: self.nodes,
            idx,
        })
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
        if self.next >= self.end {
            return None;
        }
        let key_idx = self.next;
        let value_idx = key_idx.saturating_add(1);
        let key_node = self.nodes.get(key_idx).copied();
        let value_node = if value_idx < self.end {
            self.nodes.get(value_idx).copied()
        } else {
            None
        };
        let (Some(key_node), Some(value_node)) = (key_node, value_node) else {
            // A key without a value subtree cannot occur in a validated
            // tree; stop rather than yield a torn pair.
            self.next = self.end;
            return None;
        };
        // Jump over the value's whole subtree to the next member's key
        // (clamped: a validated tree always has `size >= 1`).
        self.next = value_idx.saturating_add(value_node.size.max(1) as usize);
        Some((
            JsonKey {
                content: self.content,
                node: key_node,
            },
            JsonValue {
                content: self.content,
                nodes: self.nodes,
                idx: value_idx,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use alloc::{format, vec, vec::Vec};

    use super::*;
    use crate::spans::{FORMAT_VERSION, JsonSpans};

    // === builders: hand-build the tables validation WOULD have produced ===

    /// Incrementally builds a message buffer, handing back the span of every
    /// appended piece so tables stay consistent with the documented
    /// invariants.
    struct HeadBuilder {
        buf: Vec<u8>,
    }

    impl HeadBuilder {
        fn new() -> Self {
            Self { buf: Vec::new() }
        }

        /// Appends raw bytes, returning their span.
        fn raw(&mut self, bytes: &[u8]) -> Span {
            let start = self.buf.len() as u32;
            self.buf.extend_from_slice(bytes);
            Span::new(start, self.buf.len() as u32)
        }

        /// Appends `name: value\r\n`, returning the header record. An empty
        /// value yields a zero-length span pinned at the CR (rule B5).
        fn header_line(&mut self, name: &str, value: &[u8]) -> HeaderSpan {
            let name_span = self.raw(name.as_bytes());
            self.raw(b":");
            if !value.is_empty() {
                self.raw(b" ");
            }
            let value_span = self.raw(value);
            self.raw(b"\r\n");
            HeaderSpan {
                name: name_span,
                value: value_span,
            }
        }

        /// Appends a status line, returning the (code, reason) spans. An
        /// empty reason yields a zero-length span after the SP.
        fn status_line(&mut self, code: &str, reason: &[u8]) -> (Span, Span) {
            self.raw(b"HTTP/1.1 ");
            let code_span = self.raw(code.as_bytes());
            self.raw(b" ");
            let reason_span = self.raw(reason);
            self.raw(b"\r\n");
            (code_span, reason_span)
        }
    }

    /// `GET / HTTP/1.1` + Host, no body.
    fn minimal_request() -> (Vec<u8>, RequestSpans) {
        let mut b = HeadBuilder::new();
        let method = b.raw(b"GET");
        b.raw(b" ");
        let target = b.raw(b"/");
        b.raw(b" HTTP/1.1\r\n");
        let headers = vec![b.header_line("Host", b"example.com")];
        b.raw(b"\r\n");
        let head_end = b.buf.len() as u32;
        (
            b.buf,
            RequestSpans {
                method,
                target,
                head_end,
                headers,
                body: None,
            },
        )
    }

    /// Request head exercising header lookups: case-varied duplicates, an
    /// obs-text (non-UTF-8) value, and an empty value. No body.
    fn rich_request() -> (Vec<u8>, RequestSpans) {
        let mut b = HeadBuilder::new();
        let method = b.raw(b"GET");
        b.raw(b" ");
        let target = b.raw(b"/api/v2/pokemon");
        b.raw(b" HTTP/1.1\r\n");
        let headers = vec![
            b.header_line("Host", b"example.com"),
            b.header_line("Content-Type", b"application/json"),
            b.header_line("Accept", b"text/html"),
            b.header_line("accept", b"application/xml"),
            b.header_line("X-Bin", &[0x80, 0xC3, 0x28]),
            b.header_line("X-Empty", b""),
        ];
        b.raw(b"\r\n");
        let head_end = b.buf.len() as u32;
        (
            b.buf,
            RequestSpans {
                method,
                target,
                head_end,
                headers,
                body: None,
            },
        )
    }

    /// POST request with a Content-Length body (no JSON claim).
    fn request_with_body(content: &[u8]) -> (Vec<u8>, RequestSpans) {
        let mut b = HeadBuilder::new();
        let method = b.raw(b"POST");
        b.raw(b" ");
        let target = b.raw(b"/upload");
        b.raw(b" HTTP/1.1\r\n");
        let content_length = format!("{}", content.len());
        let headers = vec![
            b.header_line("Host", b"example.com"),
            b.header_line("Content-Length", content_length.as_bytes()),
        ];
        b.raw(b"\r\n");
        let head_end = b.buf.len() as u32;
        let raw = b.raw(content);
        let body = BodySpans {
            framing: Framing::ContentLength,
            raw,
            content_len: content.len() as u32,
            trailers: vec![],
            json: None,
        };
        (
            b.buf,
            RequestSpans {
                method,
                target,
                head_end,
                headers,
                body: Some(body),
            },
        )
    }

    /// `200 OK` response with one Server header, no body.
    fn plain_response() -> (Vec<u8>, ResponseSpans) {
        let mut b = HeadBuilder::new();
        let (code, reason) = b.status_line("200", b"OK");
        let headers = vec![b.header_line("Server", b"test")];
        b.raw(b"\r\n");
        let head_end = b.buf.len() as u32;
        (
            b.buf,
            ResponseSpans {
                code,
                reason,
                head_end,
                headers,
                body: None,
            },
        )
    }

    /// `200 OK` response with a Content-Length body claimed as JSON.
    fn response_with_json(content: &[u8], nodes: Vec<JsonNode>) -> (Vec<u8>, ResponseSpans) {
        let mut b = HeadBuilder::new();
        let (code, reason) = b.status_line("200", b"OK");
        let content_length = format!("{}", content.len());
        let headers = vec![b.header_line("Content-Length", content_length.as_bytes())];
        b.raw(b"\r\n");
        let head_end = b.buf.len() as u32;
        let raw = b.raw(content);
        let body = BodySpans {
            framing: Framing::ContentLength,
            raw,
            content_len: content.len() as u32,
            trailers: vec![],
            json: Some(JsonSpans { nodes }),
        };
        (
            b.buf,
            ResponseSpans {
                code,
                reason,
                head_end,
                headers,
                body: Some(body),
            },
        )
    }

    /// `200 OK` chunked response: one chunk per `chunks` entry, then the
    /// terminal chunk and the given trailer lines. Returns the buffer, the
    /// spans, and the (decoded bytes, chunk map) the chunk walk would have
    /// produced.
    fn chunked_response(
        chunks: &[&[u8]],
        trailers: &[(&str, &[u8])],
        json: Option<JsonSpans>,
    ) -> (Vec<u8>, ResponseSpans, Vec<u8>, Vec<ChunkMapEntry>) {
        let mut b = HeadBuilder::new();
        let (code, reason) = b.status_line("200", b"OK");
        let headers = vec![b.header_line("Transfer-Encoding", b"chunked")];
        b.raw(b"\r\n");
        let head_end = b.buf.len() as u32;
        let mut decoded = Vec::new();
        let mut chunk_map = Vec::new();
        for data in chunks {
            let size_line = format!("{:x}\r\n", data.len());
            b.raw(size_line.as_bytes());
            let span = b.raw(data);
            chunk_map.push(ChunkMapEntry {
                src_start: span.start,
                body_offset: decoded.len() as u32,
                len: data.len() as u32,
            });
            decoded.extend_from_slice(data);
            b.raw(b"\r\n");
        }
        b.raw(b"0\r\n");
        let trailer_spans: Vec<HeaderSpan> = trailers
            .iter()
            .map(|(name, value)| b.header_line(name, value))
            .collect();
        b.raw(b"\r\n");
        let raw = Span::new(head_end, b.buf.len() as u32);
        let body = BodySpans {
            framing: Framing::Chunked,
            raw,
            content_len: decoded.len() as u32,
            trailers: trailer_spans,
            json,
        };
        (
            b.buf,
            ResponseSpans {
                code,
                reason,
                head_end,
                headers,
                body: Some(body),
            },
            decoded,
            chunk_map,
        )
    }

    /// Owns everything a [`Transcript`] borrows.
    struct Fixture {
        sent: Vec<u8>,
        recv: Vec<u8>,
        table: SpanTable,
        status: u16,
        /// Decoded bytes + chunk map for a chunked response body; `None`
        /// means the response body (if any) is borrowed.
        resp_decoded: Option<(Vec<u8>, Vec<ChunkMapEntry>)>,
    }

    impl Fixture {
        fn assemble(
            sent: Vec<u8>,
            recv: Vec<u8>,
            request: RequestSpans,
            response: ResponseSpans,
            status: u16,
            resp_decoded: Option<(Vec<u8>, Vec<ChunkMapEntry>)>,
        ) -> Self {
            Self {
                sent,
                recv,
                table: SpanTable {
                    version: FORMAT_VERSION,
                    request,
                    response,
                },
                status,
                resp_decoded,
            }
        }

        /// Builds the transcript exactly as a successful `validate()` run
        /// would.
        fn transcript(&self) -> Transcript<'_> {
            let req_body = self.table.request.body.as_ref().map(|spans| ValidatedBody {
                data: BodyData::Borrowed(&self.sent[spans.raw.as_range()]),
                chunk_map: Vec::new(),
            });
            let resp_body =
                self.table
                    .response
                    .body
                    .as_ref()
                    .map(|spans| match &self.resp_decoded {
                        Some((decoded, chunk_map)) => ValidatedBody {
                            data: BodyData::Decoded(decoded.clone()),
                            chunk_map: chunk_map.clone(),
                        },
                        None => ValidatedBody {
                            data: BodyData::Borrowed(&self.recv[spans.raw.as_range()]),
                            chunk_map: Vec::new(),
                        },
                    });
            Transcript::new(
                &self.sent,
                &self.recv,
                &self.table,
                self.status,
                req_body,
                resp_body,
            )
        }
    }

    fn plain_fixture() -> Fixture {
        let (sent, request) = rich_request();
        let (recv, response) = plain_response();
        Fixture::assemble(sent, recv, request, response, 200, None)
    }

    fn reason_fixture(code: &str, reason: &[u8], status: u16) -> Fixture {
        let (sent, request) = minimal_request();
        let mut b = HeadBuilder::new();
        let (code_span, reason_span) = b.status_line(code, reason);
        b.raw(b"\r\n");
        let head_end = b.buf.len() as u32;
        let response = ResponseSpans {
            code: code_span,
            reason: reason_span,
            head_end,
            headers: vec![],
            body: None,
        };
        Fixture::assemble(sent, b.buf, request, response, status, None)
    }

    /// Request with a Content-Length body `hello`, response with DOC1 as a
    /// Content-Length JSON body.
    fn cl_fixture() -> Fixture {
        let (sent, request) = request_with_body(b"hello");
        let (recv, response) = response_with_json(DOC1, doc1_nodes());
        Fixture::assemble(sent, recv, request, response, 200, None)
    }

    /// Chunked response `hello` + ` world` with one trailer.
    fn chunked_fixture() -> Fixture {
        let (sent, request) = minimal_request();
        let chunks: &[&[u8]] = &[b"hello", b" world"];
        let trailers: &[(&str, &[u8])] = &[("X-Trailer", b"yes")];
        let (recv, response, decoded, chunk_map) = chunked_response(chunks, trailers, None);
        Fixture::assemble(
            sent,
            recv,
            request,
            response,
            200,
            Some((decoded, chunk_map)),
        )
    }

    /// Chunked response whose decoded body is `{"x": 12}` with the number
    /// split across a chunk boundary.
    fn chunked_json_fixture() -> Fixture {
        let nodes = vec![
            node(JsonKind::Object, 0, 9, 3),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 6, 8, 1),
        ];
        let (sent, request) = minimal_request();
        let chunks: &[&[u8]] = &[b"{\"x\"", b": 1", b"2}"];
        let (recv, response, decoded, chunk_map) =
            chunked_response(chunks, &[], Some(JsonSpans { nodes }));
        assert_eq!(decoded.as_slice(), b"{\"x\": 12}");
        Fixture::assemble(
            sent,
            recv,
            request,
            response,
            200,
            Some((decoded, chunk_map)),
        )
    }

    /// Minimal transcript whose response body is `content` claimed as JSON
    /// with `nodes`.
    fn json_fixture(content: &[u8], nodes: Vec<JsonNode>) -> Fixture {
        let (sent, request) = minimal_request();
        let (recv, response) = response_with_json(content, nodes);
        Fixture::assemble(sent, recv, request, response, 200, None)
    }

    fn node(kind: JsonKind, start: u32, end: u32, size: u32) -> JsonNode {
        JsonNode {
            kind,
            start,
            end,
            size,
        }
    }

    /// Nested object document; see `doc1_nodes` for the hand-built tree.
    const DOC1: &[u8] = br#"{"a": [1, {"b": "x"}, true], "c": null, "d": 1.5e3}"#;

    /// Pre-order nodes for [`DOC1`] (string/key spans exclude quotes).
    fn doc1_nodes() -> Vec<JsonNode> {
        vec![
            node(JsonKind::Object, 0, 51, 12),
            node(JsonKind::Key, 2, 3, 1),      // "a"
            node(JsonKind::Array, 6, 27, 6),   // [1, {"b": "x"}, true]
            node(JsonKind::Number, 7, 8, 1),   // 1
            node(JsonKind::Object, 10, 20, 3), // {"b": "x"}
            node(JsonKind::Key, 12, 13, 1),    // "b"
            node(JsonKind::String, 17, 18, 1), // "x"
            node(JsonKind::Bool, 22, 26, 1),   // true
            node(JsonKind::Key, 30, 31, 1),    // "c"
            node(JsonKind::Null, 34, 38, 1),   // null
            node(JsonKind::Key, 41, 42, 1),    // "d"
            node(JsonKind::Number, 45, 50, 1), // 1.5e3
        ]
    }

    /// Array document with multi-node subtrees on BOTH sides of sibling
    /// boundaries.
    const DOC2: &[u8] = br#"[[1,2],{"k":[3]},"s"]"#;

    /// Pre-order nodes for [`DOC2`].
    fn doc2_nodes() -> Vec<JsonNode> {
        vec![
            node(JsonKind::Array, 0, 21, 9),  // root
            node(JsonKind::Array, 1, 6, 3),   // [1,2]
            node(JsonKind::Number, 2, 3, 1),  // 1
            node(JsonKind::Number, 4, 5, 1),  // 2
            node(JsonKind::Object, 7, 16, 4), // {"k":[3]}
            node(JsonKind::Key, 9, 10, 1),    // "k"
            node(JsonKind::Array, 12, 15, 2), // [3]
            node(JsonKind::Number, 13, 14, 1),
            node(JsonKind::String, 18, 19, 1), // "s"
        ]
    }

    /// Document with empty containers.
    const EMPTY_DOC: &[u8] = br#"{"o": {}, "a": []}"#;

    fn empty_doc_nodes() -> Vec<JsonNode> {
        vec![
            node(JsonKind::Object, 0, 18, 5),
            node(JsonKind::Key, 2, 3, 1), // "o"
            node(JsonKind::Object, 6, 8, 1),
            node(JsonKind::Key, 11, 12, 1), // "a"
            node(JsonKind::Array, 15, 17, 1),
        ]
    }

    /// Pulls the response JSON root out of a transcript.
    fn json_root<'x>(t: &'x Transcript<'_>) -> JsonValue<'x> {
        t.response()
            .body()
            .and_then(|body| body.json())
            .expect("fixture should expose a JSON body")
    }

    /// Asserts `content_to_source(range)` round-trips: concatenating the
    /// source bytes of the returned spans yields exactly `content[range]`.
    fn assert_maps_back(buf: &[u8], body: &Body<'_>, range: Range<u32>) {
        let mut collected = Vec::new();
        for span in body.content_to_source(range.clone()) {
            collected.extend_from_slice(&buf[span.as_range()]);
        }
        assert_eq!(
            collected.as_slice(),
            &body.content()[range.start as usize..range.end as usize],
        );
    }

    // === request/response line ===

    #[test]
    fn method_and_target() {
        let f = plain_fixture();
        let t = f.transcript();
        assert_eq!(t.request().method(), "GET");
        assert_eq!(t.request().target(), "/api/v2/pokemon");
    }

    #[test]
    fn status_and_reason() {
        let f = plain_fixture();
        let t = f.transcript();
        assert_eq!(t.response().status(), 200);
        assert_eq!(t.response().reason(), "OK");
    }

    #[test]
    fn empty_reason() {
        let f = reason_fixture("204", b"", 204);
        let t = f.transcript();
        assert_eq!(t.response().status(), 204);
        assert_eq!(t.response().reason(), "");
    }

    #[test]
    fn obs_text_reason_falls_back_to_empty() {
        // 0x80 is legal obs-text on the wire but not valid UTF-8: the
        // documented fallback returns "".
        let f = reason_fixture("200", b"\x80K", 200);
        let t = f.transcript();
        assert_eq!(t.response().reason(), "");
        // ...but the raw verified span is recoverable losslessly.
        assert_eq!(t.response().reason_bytes(), b"\x80K");
    }

    #[test]
    fn reason_bytes_matches_reason_for_valid_utf8() {
        // For a UTF-8 reason the bytes equal the string's bytes; an empty
        // reason yields an empty slice.
        let f = plain_fixture();
        let t = f.transcript();
        assert_eq!(t.response().reason(), "OK");
        assert_eq!(t.response().reason_bytes(), b"OK");
        let f = reason_fixture("204", b"", 204);
        let t = f.transcript();
        assert_eq!(t.response().reason_bytes(), b"");
    }

    // === headers ===

    #[test]
    fn headers_in_order() {
        let f = plain_fixture();
        let t = f.transcript();
        let req = t.request();
        assert_eq!(req.headers().count(), 6);
        let names: Vec<&str> = req.headers().map(|h| h.name()).collect();
        assert_eq!(
            names,
            [
                "Host",
                "Content-Type",
                "Accept",
                "accept",
                "X-Bin",
                "X-Empty"
            ]
        );
        let resp = t.response();
        let names: Vec<&str> = resp.headers().map(|h| h.name()).collect();
        assert_eq!(names, ["Server"]);
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let f = plain_fixture();
        let t = f.transcript();
        let req = t.request();
        let h = req.header("content-TYPE").expect("should match");
        assert_eq!(h.name(), "Content-Type");
        assert_eq!(h.value(), b"application/json");
        assert_eq!(h.value_str(), Some("application/json"));
        assert!(req.header("HOST").is_some());
        assert!(t.response().header("sErVeR").is_some());
    }

    #[test]
    fn header_returns_first_of_duplicates() {
        let f = plain_fixture();
        let t = f.transcript();
        let h = t.request().header("ACCEPT").expect("should match");
        assert_eq!(h.name(), "Accept");
        assert_eq!(h.value(), b"text/html");
    }

    #[test]
    fn header_miss_returns_none() {
        let f = plain_fixture();
        let t = f.transcript();
        assert!(t.request().header("X-Missing").is_none());
        assert!(t.response().header("Accept").is_none());
    }

    #[test]
    fn headers_with_name_yields_duplicates_in_order() {
        let f = plain_fixture();
        let t = f.transcript();
        let req = t.request();
        let values: Vec<&[u8]> = req.headers_with_name("aCCePt").map(|h| h.value()).collect();
        assert_eq!(values, [b"text/html".as_slice(), b"application/xml"]);
        let names: Vec<&str> = req.headers_with_name("accept").map(|h| h.name()).collect();
        assert_eq!(names, ["Accept", "accept"]);
        assert_eq!(req.headers_with_name("X-Missing").count(), 0);
    }

    #[test]
    fn empty_header_value() {
        let f = plain_fixture();
        let t = f.transcript();
        let h = t.request().header("x-empty").expect("should match");
        assert_eq!(h.value(), b"");
        assert_eq!(h.value_str(), Some(""));
        let span = h.value_span();
        assert!(span.is_empty());
        // The empty value span is pinned at the terminating CR.
        let at = span.start as usize;
        assert_eq!(&f.sent[at..at + 2], b"\r\n");
    }

    #[test]
    fn non_utf8_header_value() {
        let f = plain_fixture();
        let t = f.transcript();
        let h = t.request().header("X-Bin").expect("should match");
        assert_eq!(h.value(), [0x80, 0xC3, 0x28]);
        assert!(h.value_str().is_none());
    }

    #[test]
    fn header_spans_are_source_coordinates() {
        let f = plain_fixture();
        let t = f.transcript();
        let h = t.request().header("Host").expect("should match");
        assert_eq!(&f.sent[h.name_span().as_range()], b"Host");
        assert_eq!(&f.sent[h.value_span().as_range()], b"example.com");
    }

    // === bodies ===

    #[test]
    fn body_none_when_absent() {
        let f = plain_fixture();
        let t = f.transcript();
        assert!(t.request().body().is_none());
        assert!(t.response().body().is_none());
    }

    #[test]
    fn borrowed_body_content() {
        let f = cl_fixture();
        let t = f.transcript();
        let body = t.request().body().expect("request body");
        assert_eq!(body.content(), b"hello");
        assert_eq!(body.framing(), Framing::ContentLength);
        let raw = body.raw_span();
        assert_eq!(raw.start, f.table.request.head_end);
        assert_eq!(raw.end as usize, f.sent.len());
        assert_eq!(&f.sent[raw.as_range()], b"hello");
        // Non-chunked bodies have no trailers and this one claims no JSON.
        assert_eq!(body.trailers().count(), 0);
        assert!(body.trailer("X-Trailer").is_none());
        assert!(body.json().is_none());
    }

    #[test]
    fn decoded_body_content() {
        let f = chunked_fixture();
        let t = f.transcript();
        let body = t.response().body().expect("response body");
        assert_eq!(body.content(), b"hello world");
        assert_eq!(body.framing(), Framing::Chunked);
        let raw = body.raw_span();
        assert_eq!(raw.start, f.table.response.head_end);
        assert_eq!(raw.end as usize, f.recv.len());
    }

    #[test]
    fn trailers_iterate_in_order() {
        let f = chunked_fixture();
        let t = f.transcript();
        let body = t.response().body().expect("response body");
        let trailers: Vec<Header<'_>> = body.trailers().collect();
        assert_eq!(trailers.len(), 1);
        assert_eq!(trailers[0].name(), "X-Trailer");
        assert_eq!(trailers[0].value(), b"yes");
        // Trailer spans index the SOURCE buffer.
        assert_eq!(&f.recv[trailers[0].name_span().as_range()], b"X-Trailer");
        assert_eq!(&f.recv[trailers[0].value_span().as_range()], b"yes");
    }

    #[test]
    fn trailers_and_headers_are_separate_namespaces() {
        let f = chunked_fixture();
        let t = f.transcript();
        let resp = t.response();
        let body = resp.body().expect("response body");
        // Found as a trailer (case-insensitively)...
        let trailer = body.trailer("x-TRAILER").expect("trailer should match");
        assert_eq!(trailer.value(), b"yes");
        // ...but never via header() (rule G4).
        assert!(resp.header("X-Trailer").is_none());
        // And head headers never leak into the trailer namespace.
        assert!(resp.header("Transfer-Encoding").is_some());
        assert!(body.trailer("Transfer-Encoding").is_none());
    }

    // === content_to_source ===

    #[test]
    fn content_to_source_identity() {
        let f = cl_fixture();
        let t = f.transcript();
        let body = t.request().body().expect("request body");
        let base = body.raw_span().start;
        // Middle range.
        assert_eq!(
            body.content_to_source(1..4),
            vec![Span::new(base + 1, base + 4)]
        );
        assert_maps_back(&f.sent, &body, 1..4);
        // Full body.
        assert_eq!(body.content_to_source(0..5), vec![body.raw_span()]);
        // Empty range.
        assert_eq!(body.content_to_source(2..2), Vec::new());
        // Past the end / fully out of bounds / inverted: empty.
        assert_eq!(body.content_to_source(0..6), Vec::new());
        assert_eq!(body.content_to_source(6..7), Vec::new());
        assert_eq!(
            body.content_to_source(Range { start: 4, end: 1 }),
            Vec::new()
        );
    }

    #[test]
    fn content_to_source_chunked() {
        let f = chunked_fixture();
        let t = f.transcript();
        let body = t.response().body().expect("response body");
        let map = &f.resp_decoded.as_ref().expect("chunked fixture").1;
        let (c1, c2) = (map[0], map[1]);
        assert_eq!((c1.body_offset, c1.len), (0, 5));
        assert_eq!((c2.body_offset, c2.len), (5, 6));

        // Inside one chunk.
        assert_eq!(
            body.content_to_source(1..4),
            vec![Span::new(c1.src_start + 1, c1.src_start + 4)]
        );
        // Straddling two chunks.
        assert_eq!(
            body.content_to_source(3..8),
            vec![
                Span::new(c1.src_start + 3, c1.src_start + 5),
                Span::new(c2.src_start, c2.src_start + 3),
            ]
        );
        // Exactly chunk-aligned.
        assert_eq!(
            body.content_to_source(0..5),
            vec![Span::new(c1.src_start, c1.src_start + 5)]
        );
        assert_eq!(
            body.content_to_source(5..11),
            vec![Span::new(c2.src_start, c2.src_start + 6)]
        );
        // Full body.
        assert_eq!(
            body.content_to_source(0..11),
            vec![
                Span::new(c1.src_start, c1.src_start + 5),
                Span::new(c2.src_start, c2.src_start + 6),
            ]
        );
        // Empty ranges, including one pinned at a chunk boundary.
        assert_eq!(body.content_to_source(4..4), Vec::new());
        assert_eq!(body.content_to_source(5..5), Vec::new());
        // Out of bounds.
        assert_eq!(body.content_to_source(0..12), Vec::new());
        assert_eq!(body.content_to_source(11..12), Vec::new());
        // Every in-bounds mapping round-trips to the exact content bytes.
        for range in [0..11, 1..4, 3..8, 0..5, 5..11, 4..6] {
            assert_maps_back(&f.recv, &body, range);
        }
    }

    // === JSON ===

    #[test]
    fn json_none_when_unclaimed() {
        let f = cl_fixture();
        let t = f.transcript();
        // The request body exists but carries no JSON claim.
        assert!(t.request().body().expect("request body").json().is_none());
    }

    #[test]
    fn json_root_kind_and_span() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let root = json_root(&t);
        assert_eq!(root.kind(), JsonKind::Object);
        assert_eq!(root.span(), Span::new(0, 51));
        assert!(!root.is_null());
    }

    #[test]
    fn json_get_nested_string() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let v = json_root(&t).get("a.1.b").expect("path should resolve");
        assert_eq!(v.kind(), JsonKind::String);
        assert_eq!(v.as_str(), Some("x"));
        assert_eq!(v.span(), Span::new(17, 18));
    }

    #[test]
    fn json_get_bool_null_number() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let root = json_root(&t);

        let b = root.get("a.2").expect("a.2");
        assert_eq!(b.kind(), JsonKind::Bool);
        assert_eq!(b.as_bool(), Some(true));
        assert_eq!(b.span(), Span::new(22, 26));

        let n = root.get("c").expect("c");
        assert_eq!(n.kind(), JsonKind::Null);
        assert!(n.is_null());

        let d = root.get("d").expect("d");
        assert_eq!(d.kind(), JsonKind::Number);
        assert_eq!(d.as_number_str(), Some("1.5e3"));
        assert_eq!(d.span(), Span::new(45, 50));
    }

    #[test]
    fn json_get_misses() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let root = json_root(&t);
        // Wrong key.
        assert!(root.get("z").is_none());
        // Array index out of bounds.
        assert!(root.get("a.3").is_none());
        // Path into a scalar.
        assert!(root.get("a.1.b.0").is_none());
        assert!(root.get("c.x").is_none());
        // Numeric segment on an object.
        assert!(root.get("0").is_none());
        // Non-numeric segment on an array.
        assert!(root.get("a.b").is_none());
        // Empty segments never match.
        assert!(root.get("").is_none());
        assert!(root.get("a.").is_none());
    }

    #[test]
    fn json_bool_false() {
        let f = json_fixture(
            b"[true, false]",
            vec![
                node(JsonKind::Array, 0, 13, 3),
                node(JsonKind::Bool, 1, 5, 1),
                node(JsonKind::Bool, 7, 12, 1),
            ],
        );
        let t = f.transcript();
        let root = json_root(&t);
        assert_eq!(root.get("0").expect("0").as_bool(), Some(true));
        assert_eq!(root.get("1").expect("1").as_bool(), Some(false));
    }

    #[test]
    fn json_array_len() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let root = json_root(&t);
        assert_eq!(root.get("a").expect("a").array_len(), Some(3));
        // Not arrays.
        assert_eq!(root.array_len(), None);
        assert_eq!(root.get("c").expect("c").array_len(), None);
    }

    #[test]
    fn json_iter_array() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let arr = json_root(&t).get("a").expect("a");
        let items: Vec<JsonValue<'_>> = arr.iter_array().expect("array").collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind(), JsonKind::Number);
        assert_eq!(items[0].span(), Span::new(7, 8));
        assert_eq!(items[1].kind(), JsonKind::Object);
        assert_eq!(items[1].span(), Span::new(10, 20));
        assert_eq!(items[2].kind(), JsonKind::Bool);
        assert_eq!(items[2].span(), Span::new(22, 26));
    }

    #[test]
    fn json_iter_object() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let pairs: Vec<(JsonKey<'_>, JsonValue<'_>)> =
            json_root(&t).iter_object().expect("object").collect();
        assert_eq!(pairs.len(), 3);
        let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["a", "c", "d"]);
        assert_eq!(pairs[0].1.kind(), JsonKind::Array);
        assert_eq!(pairs[1].1.kind(), JsonKind::Null);
        assert_eq!(pairs[2].1.kind(), JsonKind::Number);
        // Key spans are content spans (quotes excluded).
        assert_eq!(pairs[0].0.span(), Span::new(2, 3));
        assert_eq!(pairs[2].0.span(), Span::new(41, 42));
        // Nested object iterates too.
        let inner: Vec<(JsonKey<'_>, JsonValue<'_>)> = json_root(&t)
            .get("a.1")
            .expect("a.1")
            .iter_object()
            .expect("object")
            .collect();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0].0.as_str(), "b");
        assert_eq!(inner[0].1.as_str(), Some("x"));
    }

    #[test]
    fn json_sibling_jumps_across_multinode_subtrees() {
        let f = json_fixture(DOC2, doc2_nodes());
        let t = f.transcript();
        let root = json_root(&t);
        // Sibling boundaries [1,2] -> {"k":[3]} -> "s" have multi-node
        // subtrees on both sides; size-jumps must land exactly on each
        // element root.
        let items: Vec<JsonValue<'_>> = root.iter_array().expect("array").collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind(), JsonKind::Array);
        assert_eq!(items[0].span(), Span::new(1, 6));
        assert_eq!(items[1].kind(), JsonKind::Object);
        assert_eq!(items[1].span(), Span::new(7, 16));
        assert_eq!(items[2].kind(), JsonKind::String);
        assert_eq!(items[2].as_str(), Some("s"));
        assert_eq!(root.array_len(), Some(3));
        // Deep paths across those boundaries.
        assert_eq!(root.get("0.1").expect("0.1").as_number_str(), Some("2"));
        assert_eq!(root.get("1.k.0").expect("1.k.0").as_number_str(), Some("3"));
        assert_eq!(root.get("1.k").expect("1.k").array_len(), Some(1));
        assert_eq!(root.get("2").expect("2").as_str(), Some("s"));
    }

    #[test]
    fn json_scalar_root() {
        let f = json_fixture(b"true", vec![node(JsonKind::Bool, 0, 4, 1)]);
        let t = f.transcript();
        let root = json_root(&t);
        assert_eq!(root.kind(), JsonKind::Bool);
        assert_eq!(root.as_bool(), Some(true));
        assert_eq!(root.span(), Span::new(0, 4));
        // Scalars have no children: iterators absent, paths unresolvable.
        assert!(root.iter_array().is_none());
        assert!(root.iter_object().is_none());
        assert!(root.array_len().is_none());
        assert!(root.get("0").is_none());
        assert!(root.get("a").is_none());
    }

    #[test]
    fn json_empty_containers() {
        let f = json_fixture(EMPTY_DOC, empty_doc_nodes());
        let t = f.transcript();
        let root = json_root(&t);
        let o = root.get("o").expect("o");
        assert_eq!(o.kind(), JsonKind::Object);
        assert_eq!(o.iter_object().expect("object").count(), 0);
        assert!(o.get("x").is_none());
        let a = root.get("a").expect("a");
        assert_eq!(a.kind(), JsonKind::Array);
        assert_eq!(a.array_len(), Some(0));
        assert_eq!(a.iter_array().expect("array").count(), 0);
        assert!(a.get("0").is_none());
    }

    #[test]
    fn json_kind_gating_matrix() {
        let f = json_fixture(DOC1, doc1_nodes());
        let t = f.transcript();
        let root = json_root(&t); // Object
        let arr = root.get("a").expect("a"); // Array
        let num = root.get("d").expect("d"); // Number
        let string = root.get("a.1.b").expect("a.1.b"); // String
        let boolean = root.get("a.2").expect("a.2"); // Bool
        let null = root.get("c").expect("c"); // Null

        // as_str: String only.
        assert_eq!(string.as_str(), Some("x"));
        for v in [root, arr, num, boolean, null] {
            assert!(v.as_str().is_none());
        }
        // as_number_str: Number only.
        assert_eq!(num.as_number_str(), Some("1.5e3"));
        for v in [root, arr, string, boolean, null] {
            assert!(v.as_number_str().is_none());
        }
        // as_bool: Bool only.
        assert_eq!(boolean.as_bool(), Some(true));
        for v in [root, arr, num, string, null] {
            assert!(v.as_bool().is_none());
        }
        // is_null: Null only.
        assert!(null.is_null());
        for v in [root, arr, num, string, boolean] {
            assert!(!v.is_null());
        }
        // Array accessors: Array only.
        assert!(arr.iter_array().is_some());
        assert!(arr.array_len().is_some());
        for v in [root, num, string, boolean, null] {
            assert!(v.iter_array().is_none());
            assert!(v.array_len().is_none());
        }
        // Object accessor: Object only.
        assert!(root.iter_object().is_some());
        for v in [arr, num, string, boolean, null] {
            assert!(v.iter_object().is_none());
        }
    }

    #[test]
    fn json_over_chunked_decoded_body() {
        let f = chunked_json_fixture();
        let t = f.transcript();
        let body = t.response().body().expect("response body");
        assert_eq!(body.content(), b"{\"x\": 12}");
        let root = body.json().expect("json claim");
        let x = root.get("x").expect("x");
        assert_eq!(x.as_number_str(), Some("12"));
        // The decoded-coordinate span maps back across the chunk boundary.
        let span = x.span();
        let spans = body.content_to_source(span.start..span.end);
        assert_eq!(spans.len(), 2);
        let mut wire = Vec::new();
        for s in &spans {
            wire.extend_from_slice(&f.recv[s.as_range()]);
        }
        assert_eq!(wire.as_slice(), b"12");
    }
}
