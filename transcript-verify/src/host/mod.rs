//! Host-side span-table production (feature `parse`).
//!
//! Uses `spansy` for HTTP message structure, a small fallback head scanner
//! for the framings spansy cannot parse (close-delimited responses, HEAD
//! responses bearing `Content-Length`), and this crate's own span-emitting
//! JSON pass over the decoded body. Framing decisions reuse the validator's
//! shared derivation functions, and every emitted table is self-checked
//! with [`validate`](crate::validate()) before being returned.
// TODO(P2/C1): remove this allow once the converter is implemented.
#![allow(dead_code)]

pub(crate) mod http;
pub(crate) mod json;

use crate::{error::HostError, spans::SpanTable};

/// Parses a transcript into a [`SpanTable`] ready for
/// [`validate`](crate::validate()).
///
/// `sent` must contain exactly one HTTP/1.1 request and `recv` exactly one
/// HTTP/1.1 response. JSON node spans are emitted for a body iff its
/// `Content-Type` media type is `application/json` AND the decoded body
/// passes this crate's JSON rules; otherwise the body is claimed opaque
/// (JSON parse failure alone never fails the host parse).
///
/// Guarantee: the returned table has already passed a self-check run of the
/// validator, so host-accepted implies validator-accepted; a self-check
/// failure surfaces as [`HostError::Internal`]. Transcripts that `spansy`
/// accepts but the (stricter) validator rejects — e.g. a `+5`
/// `Content-Length`, `Content-Length` together with `Transfer-Encoding` —
/// are reported as [`HostError::Unsupported`] rather than silently emitting
/// a doomed table.
pub fn parse_transcript(_sent: &[u8], _recv: &[u8]) -> Result<SpanTable, HostError> {
    todo!()
}
