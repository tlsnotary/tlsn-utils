//! Lockstep JSON validation and RFC 8259 lexical scanners (rule group F).
//!
//! The scanners are shared with the host's span-emitting JSON pass
//! (`host/json.rs`), so guest and host agree on the lexical grammar by
//! construction.
// TODO(P1/B2): remove this allow once the checker is implemented.
#![allow(dead_code)]

use crate::{error::Error, spans::JsonNode};

/// Verifies that `nodes` is THE pre-order parse of `content` (rule
/// group F).
///
/// `content` is a decoded body; all node spans are in decoded-body
/// coordinates. Single forward pass with an explicit stack (depth ≤ 128)
/// and a lockstep node cursor:
///
/// - one linear UTF-8 check of the whole body (F0);
/// - the root span must equal the whitespace-trimmed extent — no hidden
///   prefix/suffix or second value (F1, F10);
/// - the byte at the cursor determines the only admissible node kind (F2);
/// - container interiors are exactly tiled by their children and separators,
///   with whitespace-only gaps (F3/F4);
/// - strings/keys/numbers/literals are checked lexically via the scanners below
///   (F6/F7/F8);
/// - duplicate object keys are rejected, comparing DECODED keys (F9).
///
/// Returns `Ok(())` iff every node verifies and the body is fully covered.
pub(crate) fn validate_json(_content: &[u8], _nodes: &[JsonNode]) -> Result<(), Error> {
    todo!()
}

/// Scans one RFC 8259 string starting at `start` and returns the index one
/// past the closing quote.
///
/// Contract:
///
/// - Requires `src[start] == b'"'` (the opening quote); errors if `start` is
///   out of bounds or the byte is not a quote.
/// - Content bytes may be anything except `"`, `\`, and raw control bytes (`<
///   0x20`, rejected — stricter than spansy's grammar). Escapes are `\"` `\\`
///   `\/` `\b` `\f` `\n` `\r` `\t` and `\u` followed by exactly 4 HEXDIG
///   (case-insensitive). Lone surrogates are accepted: this is a grammar-level
///   check and strings are never decoded here. Bytes `>= 0x80` pass through —
///   UTF-8 validity is checked once for the whole body by [`validate_json`].
/// - The string ends at the first unescaped `"`, which forces the boundary: no
///   other end position can validate.
///
/// On success the content span (quotes excluded) is `start + 1..ret - 1`.
/// Errors with [`Error::Json`] on a bad escape, a raw control byte, or an
/// unterminated string.
pub(crate) fn scan_string(_src: &[u8], _start: usize) -> Result<usize, Error> {
    todo!()
}

/// Scans one RFC 8259 number starting at `start` and returns the index one
/// past the end of the lexeme.
///
/// Contract:
///
/// - Requires `src[start]` to be `-` or a DIGIT.
/// - Matches the longest prefix of `src[start..]` conforming to the full RFC
///   8259 grammar `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` — no leading
///   `+`, no leading zeros, and the fraction/exponent parts require at least
///   one digit (so for `1.x` or `1e+` the lexeme is just `1`).
/// - Errors with [`Error::Json`] iff no nonempty prefix matches (e.g. a lone
///   `-`).
///
/// Maximality is NOT enforced here: callers ([`validate_json`]'s tiling
/// rule and the host emitter) must require the byte at the returned index
/// to be JSON whitespace, `,`, `}`, `]`, or end of input, which pins the
/// lexeme end (rule F7).
pub(crate) fn scan_number(_src: &[u8], _start: usize) -> Result<usize, Error> {
    todo!()
}
