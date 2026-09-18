//! Error types.

/// The reason validation of a span table failed.
///
/// Designed for `no_std` guests: every variant carries only `Copy` data
/// (offsets and `&'static str` reasons), so the whole enum is `Copy`. The
/// `reason` strings are stable, human-readable rule descriptions intended for
/// debugging and test assertions, not for programmatic dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum Error {
    /// The table's format version is not supported by this build of the
    /// validator.
    #[error("unsupported span-table format version {found}, expected {expected}")]
    Version {
        /// The version this validator supports ([`crate::FORMAT_VERSION`]).
        expected: u16,
        /// The version found in the table.
        found: u16,
    },

    /// An input buffer exceeds the 2^30-byte coordinate limit.
    ///
    /// The limit guarantees that any two in-bounds `u32` offsets can be added
    /// without overflow.
    #[error("input buffer of {len} bytes exceeds the 2^30-byte limit")]
    TooLarge {
        /// The offending buffer's length.
        len: usize,
    },

    /// The table is structurally malformed (rule group A) in a way not tied
    /// to a byte position: header/trailer/node caps exceeded, trailers on a
    /// non-chunked body, nesting depth over the limit, etc.
    #[error("malformed span table: {reason}")]
    Table {
        /// Which well-formedness rule was violated.
        reason: &'static str,
    },

    /// A span or offset in the table is outside its coordinate space or
    /// inverted (`start > end`).
    #[error("table span `{what}` out of bounds: [{start}, {end}) in space of {len} bytes")]
    SpanOutOfBounds {
        /// The span's start offset.
        start: u32,
        /// The span's end offset.
        end: u32,
        /// The length of the coordinate space (buffer or decoded body).
        len: u32,
        /// Which table field held the offending span.
        what: &'static str,
    },

    /// An HTTP grammar rule was violated during the byte walk (rule groups
    /// B, C, D, E): request/status line, header lines, chunk framing,
    /// trailers, or a table span disagreeing with the derived one.
    #[error("HTTP rule violated at byte {at}: {reason}")]
    Http {
        /// Cursor position in the source buffer where the rule failed.
        at: u32,
        /// Which rule was violated.
        reason: &'static str,
    },

    /// The body record disagrees with the framing derived from the verified
    /// head (rule groups C, D), or the head facts themselves are
    /// framing-toxic: duplicate `Content-Length`/`Transfer-Encoding`/`Host`,
    /// `Content-Length` together with `Transfer-Encoding`, an unsupported
    /// transfer coding, a body record present when none is allowed (or vice
    /// versa), or a wrong [`crate::Framing`] tag.
    #[error("framing rule violated: {reason}")]
    Framing {
        /// Which framing rule was violated.
        reason: &'static str,
    },

    /// A JSON rule was violated (rule group F) while checking a claimed JSON
    /// body against its node table.
    #[error("JSON rule violated at byte {at}: {reason}")]
    Json {
        /// Position in DECODED-body coordinates where the rule failed.
        at: u32,
        /// Which rule was violated.
        reason: &'static str,
    },
}

/// The reason host-side span-table production failed (feature `parse`).
///
/// Returned by [`parse_transcript`](crate::parse_transcript). Host code runs
/// with `std`, so variants may carry owned strings.
#[cfg(feature = "parse")]
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// `spansy` failed to parse the transcript's HTTP structure.
    #[error("spansy parse error: {0}")]
    Spansy(#[from] spansy::ParseError),

    /// The transcript is parseable HTTP but uses a feature outside this
    /// crate's v1 scope (see the README non-goals), or violates a rule the
    /// validator enforces more strictly than `spansy` (e.g. a non-canonical
    /// `Content-Length` value, `Content-Length` + `Transfer-Encoding`
    /// together).
    #[error("unsupported transcript feature ({feature}): {detail}")]
    Unsupported {
        /// Which feature or limitation was hit.
        feature: &'static str,
        /// Details, e.g. the offending header value.
        detail: String,
    },

    /// An internal invariant failed — most importantly, the emitted table
    /// failed the host's own self-check run of
    /// [`validate`](crate::validate()). Always a bug in this crate, never
    /// caller error: host-accepted transcripts are validator-accepted by
    /// construction.
    #[error("internal error: {reason}")]
    Internal {
        /// Which invariant failed.
        reason: &'static str,
        /// The validator error from the self-check, if that is what failed.
        #[source]
        source: Option<Error>,
    },
}
