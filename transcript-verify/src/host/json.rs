//! Host-side span-emitting JSON pass over a decoded body.
// TODO(P2/C2): remove this allow once the emitter is implemented.
#![allow(dead_code)]

use alloc::vec::Vec;

use crate::{error::Error, spans::JsonNode};

/// Emits the pre-order JSON node table for `content` (a decoded body), in
/// decoded-body coordinates.
///
/// Explicit-stack recursive descent sharing the validator's
/// [`scan_string`](crate::validate::json::scan_string) /
/// [`scan_number`](crate::validate::json::scan_number) scanners — so host
/// and guest agree on the grammar by construction. Nodes are pushed
/// pre-order; container `end` and `size` are patched at container close.
///
/// An `Err` means `content` is not JSON the validator would accept (the
/// error is the validator's own [`Error::Json`]); callers fall back to an
/// opaque (no-JSON) body claim rather than failing the host parse.
pub(crate) fn emit(_content: &[u8]) -> Result<Vec<JsonNode>, Error> {
    todo!()
}
