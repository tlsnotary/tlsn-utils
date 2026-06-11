//! The span-table wire format.
//!
//! Design invariant: **the table never steers parsing**. The validator's byte
//! walk is fully deterministic and every field here is *derived from the
//! transcript bytes, then equality-checked* against the table. Canonicality
//! falls out: for fixed bytes, any two tables that validate are
//! byte-identical (the only deliberate degree of freedom is the
//! JSON-vs-opaque body claim, which is consumer-visible). The table exists so
//! the guest does zero dynamic construction — it borrows the table as its
//! navigation index — and so the parse witness can cross the VM boundary or
//! be committed.
//!
//! Deliberately NOT stored (derived and checked instead): the request-line
//! extent, per-header line extents, chunk ranges, and object-member
//! (key+value) extents. A minimal table is a minimal soundness surface.

use alloc::vec::Vec;

/// The current span-table format version.
///
/// Version 2 is reserved for multi-exchange (keep-alive) transcripts, which
/// would carry a list of request/response pairs.
pub const FORMAT_VERSION: u16 = 1;

/// A byte range, end-exclusive.
///
/// Invariants enforced by validation: `start <= end`, and all coordinates are
/// `< 2^30` so that any two offsets can be added without `u32` overflow.
/// Depending on context a span is in *source* coordinates (offsets into
/// `sent` or `recv`) or *decoded-body* coordinates (offsets into a de-chunked
/// body); each field below documents which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Span {
    /// Offset of the first byte.
    pub start: u32,
    /// Offset one past the last byte.
    pub end: u32,
}

impl Span {
    /// Creates a new span.
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Returns the length in bytes (`end - start`).
    ///
    /// Saturates to 0 for an inverted span (`start > end`); validation
    /// rejects such spans, so this can only saturate on an unvalidated
    /// table.
    pub const fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Returns `true` if the span covers zero bytes.
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Returns the span as a `usize` range, suitable for slicing the buffer
    /// it indexes into.
    pub const fn as_range(&self) -> core::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// The parse witness for one transcript: a request in `sent`, a response in
/// `recv`.
///
/// Produced by [`parse_transcript`](crate::parse_transcript) on the host and
/// verified in full by [`validate`](crate::validate()) in the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SpanTable {
    /// Format version; must equal [`FORMAT_VERSION`].
    pub version: u16,
    /// Spans into the `sent` buffer.
    pub request: RequestSpans,
    /// Spans into the `recv` buffer.
    pub response: ResponseSpans,
}

/// Spans describing the request in the `sent` buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RequestSpans {
    /// The request method.
    ///
    /// Checked: starts at byte 0, consists of one or more `tchar` bytes
    /// (RFC 9110 token), and is followed by a single SP.
    pub method: Span,
    /// The request target.
    ///
    /// Checked: immediately follows the SP after the method, consists of one
    /// or more printable-ASCII bytes (0x21..=0x7E), and is followed by the
    /// literal `` HTTP/1.1\r\n``. The target is otherwise opaque — URI
    /// parsing is a non-goal.
    pub target: Span,
    /// Offset one past the CRLFCRLF terminating the head — equivalently, the
    /// offset of the first body byte (or the buffer length if there is no
    /// body).
    pub head_end: u32,
    /// Every header line, in order of appearance.
    ///
    /// Checked to be in bijection with the header lines in the bytes: no
    /// hidden, missing, or reordered records.
    pub headers: Vec<HeaderSpan>,
    /// The body record.
    ///
    /// Must be `Some` if and only if the framing derived from the verified
    /// head yields more than zero body bytes (e.g. `Content-Length: 0` means
    /// `None`).
    pub body: Option<BodySpans>,
}

/// Spans describing the response in the `recv` buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResponseSpans {
    /// The 3-digit status code.
    ///
    /// Checked: pinned to bytes `[9, 12)` (after the literal
    /// `HTTP/1.1 `), exactly 3 DIGITs with the first in `1..=5`.
    pub code: Span,
    /// The reason phrase.
    ///
    /// Possibly empty; untrimmed; charset-checked (printable ASCII plus SP
    /// and HTAB) but otherwise opaque.
    pub reason: Span,
    /// Offset one past the CRLFCRLF terminating the head — equivalently, the
    /// offset of the first body byte (or the buffer length if there is no
    /// body).
    pub head_end: u32,
    /// Every header line, in order of appearance (same checks as
    /// [`RequestSpans::headers`]).
    pub headers: Vec<HeaderSpan>,
    /// The body record (same presence rule as [`RequestSpans::body`]; for
    /// responses the derived framing also accounts for HEAD requests and
    /// 1xx/204/304 statuses, which never have a body).
    pub body: Option<BodySpans>,
}

/// Spans for one header (or chunked-trailer) line, in source coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HeaderSpan {
    /// The field name: the token before the `:`.
    ///
    /// Checked: one or more `tchar` bytes immediately followed by `:` — no
    /// whitespace is permitted before the colon.
    pub name: Span,
    /// The field value, OWS-trimmed on both sides (httparse semantics).
    ///
    /// Checked canonically: the first and last covered bytes are non-OWS,
    /// every byte between the `:` and the value (and between the value and
    /// the CR) is OWS (SP/HTAB), and value bytes are in
    /// {0x21..=0x7E, 0x80..=0xFF, SP, HTAB} — no CR/LF/NUL/DEL injection. An
    /// empty value is a zero-length span pinned at the position of the
    /// terminating CR.
    pub value: Span,
}

/// How a message body is delimited.
///
/// Never trusted from the table: [`BodySpans::framing`] must equal the
/// framing derived from the verified method/status/headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum Framing {
    /// Body length is given by a verified `Content-Length` header; the body
    /// is the final `N` bytes of the buffer, starting at `head_end`.
    ContentLength = 0,
    /// `Transfer-Encoding: chunked`; the body is decoded by the verified
    /// chunk walk.
    Chunked = 1,
    /// Close-delimited: the body extends from `head_end` to the end of the
    /// buffer. Responses only — a request claiming `Close` is rejected — and
    /// legal only when neither `Content-Length` nor `Transfer-Encoding` is
    /// present.
    Close = 2,
}

/// Spans describing one message body.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BodySpans {
    /// The claimed framing; must equal the framing derived from the verified
    /// method/status/headers.
    pub framing: Framing,
    /// The full source extent of the body section: `head_end..message_end`.
    ///
    /// For chunked bodies this includes the chunk-size lines, their CRLFs,
    /// the terminal (size-0) chunk, and any trailer section — i.e. the raw
    /// wire bytes, not the decoded content.
    pub raw: Span,
    /// The DECODED body length.
    ///
    /// Equals `raw.len()` for `ContentLength`/`Close` framing and the sum of
    /// the chunk sizes for `Chunked` (letting the guest pre-allocate the
    /// de-chunk buffer exactly once). Verified against the actual decode.
    pub content_len: u32,
    /// Chunked-trailer lines, in order of appearance; spans are source
    /// coordinates within `raw`.
    ///
    /// Chunked framing only — must be empty otherwise. Trailers named
    /// `Content-Length`, `Transfer-Encoding`, or `Host` are rejected.
    pub trailers: Vec<HeaderSpan>,
    /// The prover's claim that the decoded body is JSON.
    ///
    /// `None` is an *opaque* claim: the body bytes are still verified
    /// against the framing, but no JSON structure is checked or exposed.
    /// The claim is part of the public statement — it is deliberately NOT
    /// validated against `Content-Type`, so consumers requiring JSON must
    /// check `json.is_some()` themselves. Spans inside are in DECODED-BODY
    /// coordinates (identical to source-relative offsets only for
    /// non-chunked bodies).
    pub json: Option<JsonSpans>,
}

/// The verified JSON parse tree of a decoded body, flattened pre-order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct JsonSpans {
    /// Nodes in pre-order; `nodes[0]` is the root value.
    ///
    /// Object members are encoded as a [`JsonKind::Key`] node immediately
    /// followed by the value's subtree, per member in document order; array
    /// elements are the element subtrees in order. The guest walks this with
    /// an explicit stack — no recursion anywhere.
    pub nodes: Vec<JsonNode>,
}

/// One node of the pre-order JSON tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct JsonNode {
    /// The node's kind. The first byte of the node determines the only
    /// admissible kind, so this is verified, never trusted.
    pub kind: JsonKind,
    /// Span start, in DECODED-BODY coordinates.
    ///
    /// For `String` and `Key` nodes this is the *content* start — the quotes
    /// are excluded (spansy-compatible). For `Object`/`Array` it is the
    /// position of the `{`/`[`. For other kinds it is the start of the full
    /// lexeme.
    pub start: u32,
    /// Span end (exclusive), in DECODED-BODY coordinates.
    ///
    /// For `String`/`Key`: the content end (closing quote excluded). For
    /// `Object`/`Array`: one past the `}`/`]`. For other kinds: one past the
    /// lexeme.
    pub end: u32,
    /// The number of nodes in this node's subtree, including itself; leaves
    /// and keys have `size == 1`.
    ///
    /// Gives O(1) next-sibling navigation: the subtree after node `i` starts
    /// at `i + nodes[i].size`. Verified against the actual tree shape.
    pub size: u32,
}

/// The kind of a JSON node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum JsonKind {
    /// The literal `null`.
    Null = 0,
    /// The literal `true` or `false`.
    Bool = 1,
    /// A number (exact RFC 8259 lexeme).
    Number = 2,
    /// A string value; the span covers the content between the quotes,
    /// escapes left raw.
    String = 3,
    /// An object-member key; same span semantics as `String`. Appears
    /// immediately before the member value's subtree and is never a value
    /// kind.
    Key = 4,
    /// An object; the span covers `{` through `}`.
    Object = 5,
    /// An array; the span covers `[` through `]`.
    Array = 6,
}
