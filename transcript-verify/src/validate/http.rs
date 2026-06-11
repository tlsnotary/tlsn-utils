//! HTTP walkers: request/status lines, header lines, chunked bodies, and
//! trailers (rule groups B, C, D, E).
//!
//! Every walker advances a forward-only cursor and equality-checks the
//! table's records in lockstep — the table never steers the walk.
// TODO(P1/B1): remove this allow once the walkers are implemented.
#![allow(dead_code)]

use alloc::vec::Vec;

use crate::{
    error::Error,
    spans::{HeaderSpan, RequestSpans, ResponseSpans},
    transcript::ChunkMapEntry,
    validate::ParsedHeadInfo,
};

/// Verified facts produced by [`walk_request_head`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestHead {
    /// Cursor one past the CRLFCRLF terminating the head (== first body
    /// byte). Already equality-checked against `RequestSpans::head_end`.
    pub(crate) head_end: usize,
    /// Framing-relevant facts collected from the verified header lines,
    /// ready for [`crate::validate::derive_request_framing`].
    pub(crate) info: ParsedHeadInfo,
    /// Whether the verified method is exactly `HEAD` (drives response
    /// framing).
    pub(crate) method_is_head: bool,
}

/// Walks and verifies the request head of `buf` against `req` (rule
/// group B).
///
/// Verifies, with a cursor starting at 0: the method span (`tchar+` at byte
/// 0, then SP), the target span (printable ASCII, then the literal
/// `` HTTP/1.1\r\n``), every header line in lockstep with `req.headers`
/// (token name, no space before `:`, canonical OWS trim, value charset,
/// strict CRLF), the terminating CRLF, and `req.head_end`. Collects CL/TE
/// facts and duplicate flags into [`ParsedHeadInfo`] along the way.
pub(crate) fn walk_request_head(_buf: &[u8], _req: &RequestSpans) -> Result<RequestHead, Error> {
    todo!()
}

/// Verified facts produced by [`walk_response_head`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResponseHead {
    /// Cursor one past the CRLFCRLF terminating the head (== first body
    /// byte). Already equality-checked against `ResponseSpans::head_end`.
    pub(crate) head_end: usize,
    /// Framing-relevant facts collected from the verified header lines,
    /// ready for [`crate::validate::derive_response_framing`].
    pub(crate) info: ParsedHeadInfo,
    /// The status code parsed from the verified 3-digit span: guaranteed
    /// `100..=599`.
    pub(crate) status: u16,
}

/// Walks and verifies the response head of `buf` against `resp` (rules D1,
/// D2).
///
/// Verifies the status line `HTTP/1.1 NNN[ reason]\r\n` (code span pinned
/// to `[9, 12)`, first digit `1..=5`, reason untrimmed and charset-checked,
/// possibly empty), then header lines exactly as the request walk, the
/// terminating CRLF, and `resp.head_end`.
pub(crate) fn walk_response_head(
    _buf: &[u8],
    _resp: &ResponseSpans,
) -> Result<ResponseHead, Error> {
    todo!()
}

/// The result of a verified chunk walk (rules C6, E).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChunkWalkOutcome {
    /// The de-chunked body bytes — exactly the concatenated chunk data,
    /// `Σ chunk sizes` long.
    pub(crate) decoded: Vec<u8>,
    /// One entry per non-empty data chunk, in order, mapping decoded
    /// coordinates back to source coordinates. Derived, never advised.
    pub(crate) chunk_map: Vec<ChunkMapEntry>,
    /// Cursor one past the CRLF of the terminal (size-0) chunk line, i.e.
    /// the first byte of the trailer section. Pass to [`walk_trailers`] to
    /// consume the rest of the message.
    pub(crate) trailer_start: usize,
}

/// Walks the chunked body of `buf` starting at `start` (the first chunk-size
/// digit), decoding it.
///
/// Used verbatim by the guest validator AND the host converter's
/// de-chunking, so it takes no table input: chunk structure is fully derived
/// from the bytes. Grammar per chunk: 1..=16 HEXDIG (no leading OWS),
/// optional `;extension` (charset-checked, no CR/LF), CRLF, `size` data
/// bytes (bounds-checked), CRLF. A size of 0 terminates the walk;
/// accumulated sizes are checked against overflow and the 2^30 limit.
///
/// `capacity_hint` pre-sizes the decode buffer (the guest passes the
/// table's `content_len`, the host passes 0). It is purely an allocation
/// hint and MUST NOT affect acceptance.
pub(crate) fn walk_chunks(
    _buf: &[u8],
    _start: usize,
    _capacity_hint: usize,
) -> Result<ChunkWalkOutcome, Error> {
    todo!()
}

/// Walks the trailer section of a chunked body starting at `start` (the
/// first byte after the terminal chunk's CRLF), in lockstep with `expected`
/// (rule C7).
///
/// Each trailer line is scanned with the same header-line rules as the head
/// walks and equality-checked against `expected[i]`; trailers named
/// `Content-Length`, `Transfer-Encoding`, or `Host` are rejected, as are
/// more than 128 lines. Consumes the final CRLF terminating the trailer
/// section (which is present even when there are no trailers) and returns
/// the cursor one past it — the chunked message end.
pub(crate) fn walk_trailers(
    _buf: &[u8],
    _start: usize,
    _expected: &[HeaderSpan],
) -> Result<usize, Error> {
    todo!()
}
