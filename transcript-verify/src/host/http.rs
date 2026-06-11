//! Host-side HTTP flattening: spansy types → table spans, plus the fallback
//! head scanner for framings spansy cannot parse.
// TODO(P2/C1): remove this allow once the converter is implemented.
#![allow(dead_code)]

use crate::{
    error::HostError,
    spans::{RequestSpans, ResponseSpans},
};

/// Flattens the request head and body framing of `sent` into table spans.
///
/// Parses with `spansy`, flattens method/target/headers (re-deriving
/// empty-header-value positions locally, since spansy loses them), and
/// derives the body record via the validator's shared framing derivation.
/// The body's `json` claim is left `None`; the orchestrator
/// ([`super::parse_transcript`]) de-chunks and patches the claim in.
pub(crate) fn request_spans(_sent: &[u8]) -> Result<RequestSpans, HostError> {
    todo!()
}

/// Flattens the response head and body framing of `recv` into table spans.
///
/// Needs request context (`request_method_is_head`) for framing. Tries
/// `spansy` first; on its two known unsupported framings (close-delimited
/// responses, HEAD responses bearing `Content-Length`) falls back to a
/// small internal head scanner. As with [`request_spans`], the `json` claim
/// is left `None` for the orchestrator to patch.
pub(crate) fn response_spans(
    _recv: &[u8],
    _request_method_is_head: bool,
) -> Result<ResponseSpans, HostError> {
    todo!()
}
