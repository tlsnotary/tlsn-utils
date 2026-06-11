//! Transcript validation — the in-VM entry point.
//!
//! [`validate`] performs a single forward pass over each buffer: table
//! well-formedness checks (rule group A), the request head walk (B), framing
//! derivation (C/D), the body walk with de-chunking (E), and lockstep JSON
//! checking (F). No rule may search — every record is pinned by cursor
//! equality.
// TODO(P1/B1): remove this allow once the validator is implemented.
#![allow(dead_code)]

pub(crate) mod http;
pub(crate) mod json;

use crate::{error::Error, spans::SpanTable, transcript::Transcript};

/// Validates `table` against the transcript bytes and returns a zero-copy
/// accessor over them.
///
/// Succeeds iff `table` is THE parse of `sent` (one HTTP/1.1 request) and
/// `recv` (one HTTP/1.1 response): every span is re-derived from the bytes
/// in a single linear walk and equality-checked, so for fixed buffers at
/// most one semantically distinct table is accepted (the only degree of
/// freedom being the consumer-visible JSON-vs-opaque body claim).
///
/// Complexity is O(`sent.len()` + `recv.len()` + number of JSON nodes); the
/// only allocations are the chunked-body decode buffer(s), pre-sized from
/// the table, and the JSON walk stack.
pub fn validate<'a>(
    _sent: &'a [u8],
    _recv: &'a [u8],
    _table: &'a SpanTable,
) -> Result<Transcript<'a>, Error> {
    todo!()
}

/// Message framing derived from verified head facts.
///
/// Unlike [`crate::Framing`] (the table's claim), this is the ground truth
/// computed by [`derive_request_framing`] / [`derive_response_framing`]; the
/// claim must match it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DerivedFraming {
    /// The message has no body section.
    None,
    /// The body is exactly this many bytes starting at `head_end`. The
    /// value is already checked to be `<= 2^30` (it fits the coordinate
    /// space).
    ContentLength(u32),
    /// The body is chunked, starting at `head_end`.
    Chunked,
    /// The body extends from `head_end` to the end of the buffer
    /// (responses only).
    Close,
}

/// Framing-relevant facts collected from a verified head (guest) or a
/// host-parsed head (feature `parse`).
///
/// Both producers funnel into the same [`derive_request_framing`] /
/// [`derive_response_framing`] functions, eliminating guest/host divergence.
/// Construct with `ParsedHeadInfo::default()` plus field updates so that
/// adding fields stays non-breaking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ParsedHeadInfo {
    /// The `Content-Length` value, strictly parsed.
    ///
    /// `Some` only if a `Content-Length` header was present AND its value
    /// matched the strict grammar `1..=19 DIGIT` (no sign, no leading OWS
    /// inside the value, no comma list) and fits a `u64` (rule C3).
    /// Producers must reject the header otherwise — never coerce.
    pub(crate) content_length: Option<u64>,
    /// A `Transfer-Encoding` header was present (whatever its value).
    pub(crate) te_present: bool,
    /// A `Transfer-Encoding` header was present with the value exactly
    /// `chunked`, ASCII case-insensitive (rule C2). If `te_present` is set
    /// without this, derivation rejects (unsupported transfer coding).
    pub(crate) te_chunked: bool,
    /// More than one `Content-Length` header line was present (rule B8).
    pub(crate) dup_content_length: bool,
    /// More than one `Transfer-Encoding` header line was present (rule B8).
    pub(crate) dup_transfer_encoding: bool,
    /// More than one `Host` header line was present (rule B8).
    pub(crate) dup_host: bool,
}

/// Derives the request body framing from verified head facts.
///
/// Shared by the guest validator and the host converter. Decision order
/// (normative):
///
/// 1. any duplicate flag set → [`Error::Framing`] (rule B8);
/// 2. `Content-Length` and `Transfer-Encoding` both present →
///    [`Error::Framing`] (rule C1, request smuggling root cause);
/// 3. `te_present && !te_chunked` → [`Error::Framing`] (rule C2);
/// 4. `te_chunked` → [`DerivedFraming::Chunked`];
/// 5. `content_length: Some(n)` → [`DerivedFraming::ContentLength`] (`n > 2^30`
///    → [`Error::Framing`]);
/// 6. otherwise → [`DerivedFraming::None`] — requests are never close-delimited
///    (rule C5); the caller's coverage check rejects trailing bytes.
pub(crate) fn derive_request_framing(_headers: &ParsedHeadInfo) -> Result<DerivedFraming, Error> {
    todo!()
}

/// Derives the response body framing from verified head facts plus request
/// context.
///
/// Shared by the guest validator and the host converter. Decision order
/// (normative):
///
/// 1. duplicate / CL+TE / TE-value checks exactly as [`derive_request_framing`]
///    steps 1-3;
/// 2. `request_method_is_head`, or `status` is 1xx, 204, or 304 →
///    [`DerivedFraming::None`] regardless of CL/TE (rule D3);
/// 3. `te_chunked` → [`DerivedFraming::Chunked`];
/// 4. `content_length: Some(n)` → [`DerivedFraming::ContentLength`] (`n > 2^30`
///    → [`Error::Framing`]);
/// 5. `bytes_remain` (i.e. `recv.len() > head_end`) →
///    [`DerivedFraming::Close`], legal exactly because neither CL nor TE was
///    present (rule D4); otherwise → [`DerivedFraming::None`].
pub(crate) fn derive_response_framing(
    _request_method_is_head: bool,
    _status: u16,
    _headers: &ParsedHeadInfo,
    _bytes_remain: bool,
) -> Result<DerivedFraming, Error> {
    todo!()
}
