//! zkVM-friendly validation of pre-parsed HTTP/JSON transcript span tables.
//!
//! A transcript is a `sent` buffer holding exactly one HTTP/1.1 request and a
//! `recv` buffer holding exactly one response. Inspecting it inside a zkVM
//! guest with a full parser is prohibitively expensive, so this crate
//! implements the *non-deterministic advice* pattern:
//!
//! 1. **Outside the VM** (feature `parse`): [`parse_transcript`] parses the
//!    transcript into a flat, serializable [`SpanTable`] — byte ranges and type
//!    tags acting as a parse witness (request-line parts, header spans, body
//!    framing, and a pre-order tree of typed JSON nodes for JSON bodies).
//! 2. **Inside the VM**: [`validate()`] checks in a single linear cursor walk —
//!    no searching, no backtracking, no dynamic tree construction — that the
//!    table is THE parse of those bytes, and returns a zero-copy [`Transcript`]
//!    accessor. Every table field is derived from the bytes and
//!    equality-checked, never trusted, so for fixed bytes at most one
//!    semantically distinct table validates.
//!
//! The validator core is `no_std` + `alloc` with no required dependencies
//! beyond `thiserror`, and is zkVM-agnostic (no SP1/RISC Zero specifics). The
//! only guest allocations are the chunked-body decode buffer (pre-sized
//! exactly once from the table) and the JSON walk stack.
//!
//! # Example
//!
//! ```ignore
//! use transcript_verify::{parse_transcript, validate};
//!
//! // Host (untrusted): produce the span table once.
//! let table = parse_transcript(&sent, &recv)?;
//!
//! // Guest (inside the zkVM): verify the table against the raw bytes.
//! let transcript = validate(&sent, &recv, &table)?;
//!
//! assert_eq!(transcript.request().method(), "GET");
//! let status = transcript.response().status();
//! let name = transcript
//!     .response()
//!     .body()
//!     .and_then(|body| body.json())
//!     .and_then(|json| json.get("name"))
//!     .and_then(|value| value.as_str());
//! ```
//!
//! # Features
//!
//! - `std` (default): standard library support.
//! - `parse` (default, implies `std`): host-side span-table production via
//!   [`parse_transcript`], backed by `spansy`.
//! - `serde`: `Serialize`/`Deserialize` derives on the wire-format types so the
//!   table can cross the VM boundary.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs, unreachable_pub, unused_must_use)]
#![deny(clippy::all)]
#![deny(unsafe_code)]

extern crate alloc;

mod error;
#[cfg(feature = "parse")]
mod host;
mod spans;
mod transcript;
mod validate;

pub use error::Error;
#[cfg(feature = "parse")]
pub use error::HostError;
#[cfg(feature = "parse")]
pub use host::parse_transcript;
pub use spans::{
    BodySpans, FORMAT_VERSION, Framing, HeaderSpan, JsonKind, JsonNode, JsonSpans, RequestSpans,
    ResponseSpans, Span, SpanTable,
};
pub use transcript::{
    Body, Header, Headers, HeadersWithName, JsonArrayIter, JsonKey, JsonObjectIter, JsonValue,
    Request, Response, Transcript,
};
pub use validate::validate;
