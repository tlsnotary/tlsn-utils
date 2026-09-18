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
//! ```
//! use transcript_verify::{parse_transcript, validate};
//!
//! // One HTTP/1.1 request in `sent`, one response in `recv` — exact wire
//! // bytes (CRLF line endings, Content-Length framing).
//! let sent: &[u8] = b"GET /pets/132 HTTP/1.1\r\n\
//!     Host: api.example\r\n\
//!     \r\n";
//! let recv: &[u8] = b"HTTP/1.1 200 OK\r\n\
//!     Content-Type: application/json\r\n\
//!     Content-Length: 25\r\n\
//!     \r\n\
//!     {\"name\":\"ditto\",\"id\":132}";
//!
//! // Host (untrusted): produce the span table once.
//! let table = parse_transcript(sent, recv)?;
//!
//! // Guest (inside the zkVM): verify the table against the raw bytes.
//! let transcript = validate(sent, recv, &table)?;
//!
//! assert_eq!(transcript.request().method(), "GET");
//! assert_eq!(transcript.request().target(), "/pets/132");
//! assert_eq!(transcript.response().status(), 200);
//!
//! // Header lookup is ASCII case-insensitive.
//! let content_type = transcript.response().header("content-type").unwrap();
//! assert_eq!(content_type.value(), b"application/json");
//!
//! // The body carries a verified JSON view (the host claimed JSON and the
//! // guest verified the node tree against the bytes).
//! let body = transcript.response().body().expect("response has a body");
//! let json = body.json().expect("body is claimed and verified as JSON");
//! assert_eq!(json.get("name").and_then(|v| v.as_str()), Some("ditto"));
//! assert_eq!(json.get("id").and_then(|v| v.as_number_str()), Some("132"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
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
