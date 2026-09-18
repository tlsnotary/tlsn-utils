//! HTTP walkers: request/status lines, header lines, chunked bodies, and
//! trailers (rule groups B, C, D, E).
//!
//! Every walker advances a forward-only cursor and equality-checks the
//! table's records in lockstep — the table never steers the walk.

use alloc::vec::Vec;

use crate::{
    error::Error,
    spans::{HeaderSpan, RequestSpans, ResponseSpans, Span},
    transcript::ChunkMapEntry,
    validate::ParsedHeadInfo,
};

/// Maximum number of header (or trailer) lines per section (rule A).
pub(crate) const MAX_HEADER_LINES: usize = 128;

/// Maximum decoded body length: 2^30 bytes (rule A).
const MAX_BODY_LEN: u64 = 1 << 30;

/// Shorthand for an [`Error::Http`] at a cursor position.
const fn http_err(at: usize, reason: &'static str) -> Error {
    Error::Http {
        at: at as u32,
        reason,
    }
}

// === charset predicates ===

/// RFC 9110 `tchar`: any VCHAR except delimiters.
fn is_tchar(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    ) || b.is_ascii_alphanumeric()
}

/// Request-target byte: printable ASCII (0x21..=0x7E).
fn is_target_byte(b: u8) -> bool {
    (0x21..=0x7E).contains(&b)
}

/// Optional whitespace: SP or HTAB.
pub(crate) fn is_ows(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Header-value (and reason / chunk-extension) byte:
/// {0x21..=0x7E, 0x80..=0xFF, SP, HTAB} — no CR/LF/NUL/DEL injection
/// (rule B5).
pub(crate) fn is_value_byte(b: u8) -> bool {
    b == b'\t' || (b >= 0x20 && b != 0x7F)
}

// === cursor primitives ===

/// Requires the literal `lit` at `buf[p..]` and returns the cursor one past
/// it. Truncation and mismatch both yield [`Error::Http`] at `p` with
/// `reason`.
pub(crate) fn expect_lit(
    buf: &[u8],
    p: usize,
    lit: &[u8],
    reason: &'static str,
) -> Result<usize, Error> {
    let end = p.checked_add(lit.len()).ok_or(http_err(p, reason))?;
    if end > buf.len() || &buf[p..end] != lit {
        return Err(http_err(p, reason));
    }
    Ok(end)
}

/// Advances the cursor over any run of OWS (SP/HTAB) bytes.
pub(crate) fn skip_ows(buf: &[u8], mut p: usize) -> usize {
    while p < buf.len() && is_ows(buf[p]) {
        p += 1;
    }
    p
}

/// Advances the cursor over any run of `tchar` bytes and returns the end
/// (== `p` for an empty run).
fn scan_token(buf: &[u8], mut p: usize) -> usize {
    while p < buf.len() && is_tchar(buf[p]) {
        p += 1;
    }
    p
}

/// Strictly parses `1..=19` ASCII DIGITs (rule C3).
///
/// Returns `None` for an empty slice, more than 19 digits, or any non-digit
/// byte (so `+5`, `0x5`, `5,5`, and OWS are all rejected; leading zeros are
/// valid digit-grammar and accepted). 19 digits cannot overflow a `u64`, but
/// the arithmetic is checked anyway.
pub(crate) fn parse_dec_u64(s: &[u8]) -> Option<u64> {
    if s.is_empty() || s.len() > 19 {
        return None;
    }
    let mut n: u64 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    Some(n)
}

/// Strictly parses `1..=16` HEXDIGs, case-insensitive (rule C6).
///
/// Returns `None` for an empty slice, more than 16 digits, or any non-HEXDIG
/// byte. 16 HEXDIGs cannot overflow a `u64`, but the arithmetic is checked
/// anyway.
fn parse_hex_u64(s: &[u8]) -> Option<u64> {
    if s.is_empty() || s.len() > 16 {
        return None;
    }
    let mut n: u64 = 0;
    for &b in s {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        n = n.checked_mul(16)?.checked_add(u64::from(d))?;
    }
    Some(n)
}

/// Checks that a table span is non-inverted and within a coordinate space of
/// `len` bytes (rule A). Every table span must pass here before any use.
pub(crate) fn check_span(span: Span, len: usize, what: &'static str) -> Result<(), Error> {
    if span.start > span.end || span.end as usize > len {
        return Err(Error::SpanOutOfBounds {
            start: span.start,
            end: span.end,
            len: u32::try_from(len).unwrap_or(u32::MAX),
            what,
        });
    }
    Ok(())
}

// === header-line scanning (shared by heads and trailers) ===

/// Byte facts of one scanned header line.
pub(crate) struct ScannedHeader {
    /// Name token start (the line start).
    pub(crate) name_start: usize,
    /// One past the name token (the `:` position).
    pub(crate) name_end: usize,
    /// OWS-trimmed value start (== the CR position for an empty value).
    pub(crate) value_start: usize,
    /// OWS-trimmed value end (== `value_start` for an empty value).
    pub(crate) value_end: usize,
    /// One past the terminating LF.
    pub(crate) line_end: usize,
}

/// Scans one header (or trailer) line at `p` (rules B4/B5/B6).
///
/// The caller must have established that `buf[p]` exists and is not CR (the
/// section terminator). Verifies: no leading SP/HTAB (obs-fold), `tchar+`
/// name immediately followed by `:`, value bytes in the value charset, and a
/// strict CRLF terminator. Derives the canonical OWS-trimmed value span,
/// pinned at the CR when empty.
pub(crate) fn scan_header_line(buf: &[u8], p: usize) -> Result<ScannedHeader, Error> {
    if is_ows(buf[p]) {
        return Err(http_err(p, "obs-fold or whitespace before header name"));
    }
    let name_start = p;
    let name_end = scan_token(buf, p);
    if name_end == name_start {
        return Err(http_err(p, "invalid header name"));
    }
    if name_end >= buf.len() {
        return Err(http_err(name_end, "truncated header line"));
    }
    if buf[name_end] != b':' {
        return Err(http_err(name_end, "expected colon after header name"));
    }
    let value_scan_start = skip_ows(buf, name_end + 1);
    let mut q = value_scan_start;
    while q < buf.len() && is_value_byte(buf[q]) {
        q += 1;
    }
    if q >= buf.len() {
        return Err(http_err(q, "truncated header line"));
    }
    if buf[q] != b'\r' {
        return Err(http_err(q, "invalid byte in header value"));
    }
    let line_end = expect_lit(buf, q, b"\r\n", "expected LF after CR")?;
    // Canonical trim: leading OWS already skipped; trim trailing OWS.
    let mut value_end = q;
    while value_end > value_scan_start && is_ows(buf[value_end - 1]) {
        value_end -= 1;
    }
    // An empty value is pinned at the CR position.
    let (value_start, value_end) = if value_scan_start == value_end {
        (q, q)
    } else {
        (value_scan_start, value_end)
    };
    Ok(ScannedHeader {
        name_start,
        name_end,
        value_start,
        value_end,
        line_end,
    })
}

/// Bounds-checks a header record's spans and equality-checks them against
/// the scanned line (rule B7).
fn check_header_record(
    buf_len: usize,
    scanned: &ScannedHeader,
    record: &HeaderSpan,
) -> Result<(), Error> {
    check_span(record.name, buf_len, "header name")?;
    check_span(record.value, buf_len, "header value")?;
    if record.name.start as usize != scanned.name_start
        || record.name.end as usize != scanned.name_end
    {
        return Err(http_err(scanned.name_start, "header name span mismatch"));
    }
    if record.value.start as usize != scanned.value_start
        || record.value.end as usize != scanned.value_end
    {
        return Err(http_err(scanned.value_start, "header value span mismatch"));
    }
    Ok(())
}

/// Walks header lines from `start` until the blank line terminating the
/// head, in lockstep with `expected`, collecting framing facts.
///
/// Returns the cursor one past the blank line's CRLF (== `head_end`) and the
/// collected [`ParsedHeadInfo`].
fn walk_header_lines(
    buf: &[u8],
    start: usize,
    expected: &[HeaderSpan],
) -> Result<(usize, ParsedHeadInfo), Error> {
    if expected.len() > MAX_HEADER_LINES {
        return Err(Error::Table {
            reason: "more than 128 header records",
        });
    }
    let mut p = start;
    let mut info = ParsedHeadInfo::default();
    let (mut cl_seen, mut te_seen, mut host_seen) = (false, false, false);
    let mut i = 0usize;
    loop {
        if p >= buf.len() {
            return Err(http_err(p, "truncated head"));
        }
        if buf[p] == b'\r' {
            p = expect_lit(buf, p, b"\r\n", "expected LF after CR")?;
            break;
        }
        // The cap applies to the byte-derived line count (rule A).
        if i >= MAX_HEADER_LINES {
            return Err(Error::Table {
                reason: "more than 128 header lines",
            });
        }
        let h = scan_header_line(buf, p)?;
        let Some(record) = expected.get(i) else {
            return Err(http_err(p, "header line without table record"));
        };
        check_header_record(buf.len(), &h, record)?;
        let name = &buf[h.name_start..h.name_end];
        let value = &buf[h.value_start..h.value_end];
        if name.eq_ignore_ascii_case(b"content-length") {
            info.dup_content_length |= cl_seen;
            cl_seen = true;
            // A malformed Content-Length is an immediate error, never
            // ignored (rule C3).
            let n = parse_dec_u64(value)
                .ok_or(http_err(h.value_start, "malformed Content-Length value"))?;
            info.content_length = Some(n);
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            info.dup_transfer_encoding |= te_seen;
            te_seen = true;
            info.te_present = true;
            info.te_chunked = value.eq_ignore_ascii_case(b"chunked");
        } else if name.eq_ignore_ascii_case(b"host") {
            info.dup_host |= host_seen;
            host_seen = true;
        }
        p = h.line_end;
        i += 1;
    }
    if i != expected.len() {
        return Err(http_err(p, "table has extra header records"));
    }
    Ok((p, info))
}

// === head walks ===

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
pub(crate) fn walk_request_head(buf: &[u8], req: &RequestSpans) -> Result<RequestHead, Error> {
    let method_end = scan_token(buf, 0);
    if method_end == 0 {
        return Err(http_err(0, "invalid method token"));
    }
    check_span(req.method, buf.len(), "request.method")?;
    if req.method.start != 0 || req.method.end as usize != method_end {
        return Err(http_err(0, "method span mismatch"));
    }
    let method_is_head = &buf[..method_end] == b"HEAD";
    let mut p = expect_lit(buf, method_end, b" ", "expected SP after method")?;
    let target_start = p;
    while p < buf.len() && is_target_byte(buf[p]) {
        p += 1;
    }
    if p == target_start {
        return Err(http_err(p, "invalid request target"));
    }
    check_span(req.target, buf.len(), "request.target")?;
    if req.target.start as usize != target_start || req.target.end as usize != p {
        return Err(http_err(target_start, "target span mismatch"));
    }
    p = expect_lit(buf, p, b" HTTP/1.1\r\n", "expected ` HTTP/1.1` and CRLF")?;
    let (head_end, info) = walk_header_lines(buf, p, &req.headers)?;
    if req.head_end as usize != head_end {
        return Err(http_err(head_end, "head_end mismatch"));
    }
    Ok(RequestHead {
        head_end,
        info,
        method_is_head,
    })
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
pub(crate) fn walk_response_head(buf: &[u8], resp: &ResponseSpans) -> Result<ResponseHead, Error> {
    let mut p = expect_lit(buf, 0, b"HTTP/1.1 ", "expected `HTTP/1.1 `")?;
    // p == 9: exactly 3 DIGITs, the first in 1..=5.
    if p + 3 > buf.len() {
        return Err(http_err(buf.len(), "truncated status line"));
    }
    if !(b'1'..=b'5').contains(&buf[9]) {
        return Err(http_err(9, "status code must start with a digit in 1-5"));
    }
    if !buf[10].is_ascii_digit() || !buf[11].is_ascii_digit() {
        return Err(http_err(10, "status code must be 3 digits"));
    }
    let status =
        u16::from(buf[9] - b'0') * 100 + u16::from(buf[10] - b'0') * 10 + u16::from(buf[11] - b'0');
    check_span(resp.code, buf.len(), "response.code")?;
    if resp.code.start != 9 || resp.code.end != 12 {
        return Err(http_err(9, "status code span mismatch"));
    }
    p = 12;
    let (reason_start, reason_end) = if p < buf.len() && buf[p] == b' ' {
        // `SP reason` form: the reason is everything up to the CR,
        // untrimmed.
        p += 1;
        let start = p;
        while p < buf.len() && is_value_byte(buf[p]) {
            p += 1;
        }
        (start, p)
    } else {
        // No-reason form: CRLF directly at 12, empty reason pinned there.
        (p, p)
    };
    check_span(resp.reason, buf.len(), "response.reason")?;
    if resp.reason.start as usize != reason_start || resp.reason.end as usize != reason_end {
        return Err(http_err(reason_start, "reason span mismatch"));
    }
    p = expect_lit(buf, p, b"\r\n", "expected CRLF after status line")?;
    let (head_end, info) = walk_header_lines(buf, p, &resp.headers)?;
    if resp.head_end as usize != head_end {
        return Err(http_err(head_end, "head_end mismatch"));
    }
    Ok(ResponseHead {
        head_end,
        info,
        status,
    })
}

// === chunked bodies ===

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
    buf: &[u8],
    start: usize,
    capacity_hint: usize,
) -> Result<ChunkWalkOutcome, Error> {
    let mut p = start;
    // The decoded body can never exceed the source buffer.
    let mut decoded = Vec::with_capacity(capacity_hint.min(buf.len()));
    let mut chunk_map = Vec::new();
    let mut total: u64 = 0;
    loop {
        // Chunk size: 1..=16 HEXDIG starting exactly at the cursor — no
        // leading OWS (rule C6).
        let mut q = p;
        while q < buf.len() && buf[q].is_ascii_hexdigit() {
            q += 1;
        }
        if q == p {
            // Also guards the slice below when `start` is past the buffer.
            return Err(http_err(p, "invalid chunk size"));
        }
        let size = parse_hex_u64(&buf[p..q]).ok_or(http_err(p, "invalid chunk size"))?;
        total = total
            .checked_add(size)
            .filter(|&t| t <= MAX_BODY_LEN)
            .ok_or(http_err(p, "chunked body exceeds 2^30 bytes"))?;
        // Optional extension: bytes until the CR must be in the value
        // charset, and if any non-OWS byte exists the first one must be `;`.
        let ext_start = q;
        p = q;
        let mut first_non_ows = None;
        while p < buf.len() && is_value_byte(buf[p]) {
            if first_non_ows.is_none() && !is_ows(buf[p]) {
                first_non_ows = Some(buf[p]);
            }
            p += 1;
        }
        if let Some(b) = first_non_ows
            && b != b';'
        {
            return Err(http_err(ext_start, "invalid chunk extension"));
        }
        p = expect_lit(buf, p, b"\r\n", "expected CRLF after chunk size")?;
        if size == 0 {
            return Ok(ChunkWalkOutcome {
                decoded,
                chunk_map,
                trailer_start: p,
            });
        }
        // `size <= total <= 2^30`, so it fits a usize even on 32-bit.
        let size = size as usize;
        let data_end = p
            .checked_add(size)
            .filter(|&e| e <= buf.len())
            .ok_or(http_err(p, "chunk data exceeds buffer"))?;
        chunk_map.push(ChunkMapEntry {
            src_start: p as u32,
            body_offset: decoded.len() as u32,
            len: size as u32,
        });
        decoded.extend_from_slice(&buf[p..data_end]);
        p = expect_lit(buf, data_end, b"\r\n", "expected CRLF after chunk data")?;
    }
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
    buf: &[u8],
    start: usize,
    expected: &[HeaderSpan],
) -> Result<usize, Error> {
    if expected.len() > MAX_HEADER_LINES {
        return Err(Error::Table {
            reason: "more than 128 trailer records",
        });
    }
    let mut p = start;
    let mut i = 0usize;
    loop {
        if p >= buf.len() {
            return Err(http_err(p, "truncated trailer section"));
        }
        if buf[p] == b'\r' {
            p = expect_lit(buf, p, b"\r\n", "expected LF after CR")?;
            break;
        }
        if i >= MAX_HEADER_LINES {
            return Err(Error::Table {
                reason: "more than 128 trailer lines",
            });
        }
        let h = scan_header_line(buf, p)?;
        let Some(record) = expected.get(i) else {
            return Err(http_err(p, "trailer line without table record"));
        };
        check_header_record(buf.len(), &h, record)?;
        let name = &buf[h.name_start..h.name_end];
        if name.eq_ignore_ascii_case(b"content-length")
            || name.eq_ignore_ascii_case(b"transfer-encoding")
            || name.eq_ignore_ascii_case(b"host")
        {
            return Err(http_err(h.name_start, "restricted trailer name"));
        }
        p = h.line_end;
        i += 1;
    }
    if i != expected.len() {
        return Err(http_err(p, "table has extra trailer records"));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use super::*;

    fn span(start: usize, end: usize) -> Span {
        Span::new(start as u32, end as u32)
    }

    fn header(ns: usize, ne: usize, vs: usize, ve: usize) -> HeaderSpan {
        HeaderSpan {
            name: span(ns, ne),
            value: span(vs, ve),
        }
    }

    fn req(method: Span, target: Span, head_end: usize, headers: Vec<HeaderSpan>) -> RequestSpans {
        RequestSpans {
            method,
            target,
            head_end: head_end as u32,
            headers,
            body: None,
        }
    }

    fn resp(code: Span, reason: Span, head_end: usize, headers: Vec<HeaderSpan>) -> ResponseSpans {
        ResponseSpans {
            code,
            reason,
            head_end: head_end as u32,
            headers,
            body: None,
        }
    }

    fn assert_http_err<T: core::fmt::Debug>(result: Result<T, Error>, want: &str) {
        match result {
            Err(Error::Http { reason, .. }) => assert_eq!(reason, want),
            other => panic!("expected Http {{ {want:?} }}, got {other:?}"),
        }
    }

    // === parse_dec_u64 ===

    #[test]
    fn parse_dec_accepts_digits() {
        assert_eq!(parse_dec_u64(b"0"), Some(0));
        assert_eq!(parse_dec_u64(b"5"), Some(5));
        assert_eq!(parse_dec_u64(b"42"), Some(42));
        // Leading zeros are valid DIGIT grammar (rule C3).
        assert_eq!(parse_dec_u64(b"05"), Some(5));
        assert_eq!(parse_dec_u64(b"012"), Some(12));
        // 19 digits is the maximum.
        assert_eq!(
            parse_dec_u64(b"9999999999999999999"),
            Some(9_999_999_999_999_999_999)
        );
    }

    #[test]
    fn parse_dec_rejects_non_digits() {
        assert_eq!(parse_dec_u64(b""), None);
        assert_eq!(parse_dec_u64(b"+5"), None);
        assert_eq!(parse_dec_u64(b"-5"), None);
        assert_eq!(parse_dec_u64(b"0x5"), None);
        assert_eq!(parse_dec_u64(b"5,5"), None);
        assert_eq!(parse_dec_u64(b"5 "), None);
        assert_eq!(parse_dec_u64(b" 5"), None);
        assert_eq!(parse_dec_u64(b"5a"), None);
    }

    #[test]
    fn parse_dec_rejects_twenty_digits() {
        // 20 digits is rejected on length alone, even when the value fits.
        assert_eq!(parse_dec_u64(b"00000000000000000005"), None);
        assert_eq!(parse_dec_u64(b"99999999999999999999"), None);
    }

    // === parse_hex_u64 ===

    #[test]
    fn parse_hex_accepts_both_cases() {
        assert_eq!(parse_hex_u64(b"0"), Some(0));
        assert_eq!(parse_hex_u64(b"a"), Some(10));
        assert_eq!(parse_hex_u64(b"A"), Some(10));
        assert_eq!(parse_hex_u64(b"fF"), Some(255));
        assert_eq!(parse_hex_u64(b"10"), Some(16));
        // 16 HEXDIGs is the maximum: exactly u64::MAX.
        assert_eq!(parse_hex_u64(b"ffffffffffffffff"), Some(u64::MAX));
        assert_eq!(parse_hex_u64(b"0000000000000005"), Some(5));
    }

    #[test]
    fn parse_hex_rejects_bad_input() {
        assert_eq!(parse_hex_u64(b""), None);
        assert_eq!(parse_hex_u64(b"g"), None);
        assert_eq!(parse_hex_u64(b"+5"), None);
        assert_eq!(parse_hex_u64(b"0x5"), None);
        // 17 HEXDIGs is rejected on length alone.
        assert_eq!(parse_hex_u64(b"00000000000000005"), None);
        assert_eq!(parse_hex_u64(b"1ffffffffffffffff"), None);
    }

    // === charset predicates ===

    #[test]
    fn tchar_predicate() {
        for b in b"!#$%&'*+-.^_`|~0aZ9" {
            assert!(is_tchar(*b), "{b:?}");
        }
        for b in b" :;,/(){}<>@?=[]\"\\\r\n\0" {
            assert!(!is_tchar(*b), "{b:?}");
        }
        assert!(!is_tchar(0x7F));
        assert!(!is_tchar(0x80));
    }

    #[test]
    fn value_byte_predicate() {
        assert!(is_value_byte(b'\t'));
        assert!(is_value_byte(b' '));
        assert!(is_value_byte(0x21));
        assert!(is_value_byte(0x7E));
        assert!(is_value_byte(0x80));
        assert!(is_value_byte(0xFF));
        assert!(!is_value_byte(0x00));
        assert!(!is_value_byte(0x1F));
        assert!(!is_value_byte(b'\r'));
        assert!(!is_value_byte(b'\n'));
        assert!(!is_value_byte(0x7F));
    }

    #[test]
    fn target_byte_predicate() {
        assert!(is_target_byte(0x21));
        assert!(is_target_byte(b'/'));
        assert!(is_target_byte(0x7E));
        assert!(!is_target_byte(b' '));
        assert!(!is_target_byte(0x7F));
        assert!(!is_target_byte(0x80));
        assert!(!is_target_byte(b'\r'));
    }

    // === cursor primitives ===

    #[test]
    fn expect_lit_matches_and_rejects() {
        assert_eq!(expect_lit(b"abcd", 1, b"bc", "x"), Ok(3));
        assert_http_err(expect_lit(b"abcd", 1, b"bd", "lit"), "lit");
        // Truncated buffer.
        assert_http_err(expect_lit(b"ab", 1, b"bc", "lit"), "lit");
        // Cursor past the end.
        assert_http_err(expect_lit(b"ab", 5, b"b", "lit"), "lit");
    }

    #[test]
    fn skip_ows_and_scan_token() {
        assert_eq!(skip_ows(b"  \tx", 0), 3);
        assert_eq!(skip_ows(b"x", 0), 0);
        assert_eq!(skip_ows(b"  ", 0), 2);
        assert_eq!(scan_token(b"abc def", 0), 3);
        assert_eq!(scan_token(b" abc", 0), 0);
        assert_eq!(scan_token(b"abc", 3), 3);
        assert_eq!(scan_token(b"abc", 9), 9);
    }

    #[test]
    fn check_span_bounds() {
        assert!(check_span(span(0, 4), 4, "x").is_ok());
        assert!(check_span(span(4, 4), 4, "x").is_ok());
        assert!(matches!(
            check_span(span(0, 5), 4, "x"),
            Err(Error::SpanOutOfBounds { .. })
        ));
        // Inverted span.
        assert!(matches!(
            check_span(span(3, 2), 4, "x"),
            Err(Error::SpanOutOfBounds { .. })
        ));
    }

    // === walk_request_head ===

    const GET: &[u8] = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";

    fn get_table() -> RequestSpans {
        req(span(0, 3), span(4, 6), 28, vec![header(17, 21, 23, 24)])
    }

    #[test]
    fn request_head_happy_path() {
        let head = walk_request_head(GET, &get_table()).unwrap();
        assert_eq!(head.head_end, 28);
        assert!(!head.method_is_head);
        assert_eq!(head.info, ParsedHeadInfo::default());
    }

    #[test]
    fn request_head_detects_head_method() {
        let buf = b"HEAD /a HTTP/1.1\r\n\r\n";
        let table = req(span(0, 4), span(5, 7), 20, vec![]);
        assert!(walk_request_head(buf, &table).unwrap().method_is_head);

        // Exact case only: lowercase `head` is a different method.
        let buf = b"head /a HTTP/1.1\r\n\r\n";
        assert!(!walk_request_head(buf, &table).unwrap().method_is_head);
    }

    #[test]
    fn request_head_rejects_method_span_off_by_one() {
        // "GE"
        let mut t = get_table();
        t.method = span(0, 2);
        assert_http_err(walk_request_head(GET, &t), "method span mismatch");
        // "GET " (swallowing the SP)
        let mut t = get_table();
        t.method = span(0, 4);
        assert_http_err(walk_request_head(GET, &t), "method span mismatch");
        // Not starting at 0.
        let mut t = get_table();
        t.method = span(1, 3);
        assert_http_err(walk_request_head(GET, &t), "method span mismatch");
    }

    #[test]
    fn request_head_rejects_method_span_out_of_bounds() {
        let mut t = get_table();
        t.method = span(0, 1000);
        assert!(matches!(
            walk_request_head(GET, &t),
            Err(Error::SpanOutOfBounds { .. })
        ));
    }

    #[test]
    fn request_head_rejects_bad_method_bytes() {
        // Leading SP: empty token.
        let t = req(span(0, 3), span(4, 6), 28, vec![]);
        assert_http_err(
            walk_request_head(b" GET /a HTTP/1.1\r\n\r\n", &t),
            "invalid method token",
        );
        assert_http_err(walk_request_head(b"", &t), "invalid method token");
    }

    #[test]
    fn request_head_rejects_target_span_off_by_one() {
        let mut t = get_table();
        t.target = span(4, 5);
        assert_http_err(walk_request_head(GET, &t), "target span mismatch");
        let mut t = get_table();
        t.target = span(5, 6);
        assert_http_err(walk_request_head(GET, &t), "target span mismatch");
        let mut t = get_table();
        t.target = span(4, 7);
        assert_http_err(walk_request_head(GET, &t), "target span mismatch");
    }

    #[test]
    fn request_head_rejects_empty_or_doubled_sp_target() {
        // Double SP after method: target scan starts on SP -> empty.
        let t = req(span(0, 3), span(5, 7), 29, vec![]);
        assert_http_err(
            walk_request_head(b"GET  /a HTTP/1.1\r\n\r\n", &t),
            "invalid request target",
        );
    }

    #[test]
    fn request_head_rejects_bad_version() {
        let t = req(span(0, 3), span(4, 6), 28, vec![]);
        assert_http_err(
            walk_request_head(b"GET /a HTTP/1.0\r\n\r\n", &t),
            "expected ` HTTP/1.1` and CRLF",
        );
        assert_http_err(
            walk_request_head(b"GET /a http/1.1\r\n\r\n", &t),
            "expected ` HTTP/1.1` and CRLF",
        );
        // Bare LF after the version.
        assert_http_err(
            walk_request_head(b"GET /a HTTP/1.1\n\r\n", &t),
            "expected ` HTTP/1.1` and CRLF",
        );
    }

    #[test]
    fn request_head_rejects_head_end_off_by_one() {
        let mut t = get_table();
        t.head_end = 27;
        assert_http_err(walk_request_head(GET, &t), "head_end mismatch");
        let mut t = get_table();
        t.head_end = 29;
        assert_http_err(walk_request_head(GET, &t), "head_end mismatch");
    }

    #[test]
    fn request_head_rejects_truncated_head() {
        // No terminating blank line.
        let t = req(span(0, 3), span(4, 6), 26, vec![header(17, 21, 23, 24)]);
        assert_http_err(
            walk_request_head(b"GET /a HTTP/1.1\r\nHost: x\r\n", &t),
            "truncated head",
        );
    }

    // === header lines (exercised through walk_request_head) ===

    #[test]
    fn header_value_ows_is_trimmed_canonically() {
        //                   0         1         2
        //                   0123456789012345678901234567890
        let buf = b"GET /a HTTP/1.1\r\nX:  v \t\r\n\r\n";
        // value "v" at offset 21.
        let t = req(span(0, 3), span(4, 6), 28, vec![header(17, 18, 21, 22)]);
        walk_request_head(buf, &t).unwrap();

        // Span extended one byte left into the OWS.
        let t = req(span(0, 3), span(4, 6), 28, vec![header(17, 18, 20, 22)]);
        assert_http_err(walk_request_head(buf, &t), "header value span mismatch");
        // Span extended one byte right into the OWS.
        let t = req(span(0, 3), span(4, 6), 28, vec![header(17, 18, 21, 23)]);
        assert_http_err(walk_request_head(buf, &t), "header value span mismatch");
        // Span swallowing the CR.
        let t = req(span(0, 3), span(4, 6), 28, vec![header(17, 18, 21, 25)]);
        assert_http_err(walk_request_head(buf, &t), "header value span mismatch");
    }

    #[test]
    fn header_empty_value_is_pinned_at_cr() {
        // `X:` form — CR at 19.
        let buf = b"GET /a HTTP/1.1\r\nX:\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 23, vec![header(17, 18, 19, 19)]);
        walk_request_head(buf, &t).unwrap();
        // Wrong pin position.
        let t = req(span(0, 3), span(4, 6), 23, vec![header(17, 18, 18, 18)]);
        assert_http_err(walk_request_head(buf, &t), "header value span mismatch");

        // `X: ` form — CR at 20, value still pinned at the CR.
        let buf = b"GET /a HTTP/1.1\r\nX: \r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 24, vec![header(17, 18, 20, 20)]);
        walk_request_head(buf, &t).unwrap();
        // Pinning at the OWS instead of the CR is rejected.
        let t = req(span(0, 3), span(4, 6), 24, vec![header(17, 18, 19, 19)]);
        assert_http_err(walk_request_head(buf, &t), "header value span mismatch");
    }

    #[test]
    fn header_name_rejects_space_before_colon() {
        let buf = b"GET /a HTTP/1.1\r\nHost : x\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 29, vec![header(17, 21, 24, 25)]);
        assert_http_err(
            walk_request_head(buf, &t),
            "expected colon after header name",
        );
    }

    #[test]
    fn header_name_span_off_by_one_rejected() {
        let mut t = get_table();
        t.headers = vec![header(17, 20, 23, 24)];
        assert_http_err(walk_request_head(GET, &t), "header name span mismatch");
        let mut t = get_table();
        t.headers = vec![header(18, 21, 23, 24)];
        assert_http_err(walk_request_head(GET, &t), "header name span mismatch");
    }

    #[test]
    fn header_rejects_obs_fold() {
        let buf = b"GET /a HTTP/1.1\r\nX: a\r\n b\r\n\r\n";
        let t = req(
            span(0, 3),
            span(4, 6),
            29,
            vec![header(17, 18, 20, 21), header(23, 24, 24, 24)],
        );
        assert_http_err(
            walk_request_head(buf, &t),
            "obs-fold or whitespace before header name",
        );
    }

    #[test]
    fn header_rejects_bare_lf() {
        // Bare LF terminating a header line.
        let buf = b"GET /a HTTP/1.1\r\nX: a\n\r\n";
        let t = req(span(0, 3), span(4, 6), 24, vec![header(17, 18, 20, 21)]);
        assert_http_err(walk_request_head(buf, &t), "invalid byte in header value");
        // Bare CR mid-line (CR not followed by LF).
        let buf = b"GET /a HTTP/1.1\r\nX: a\rb\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 27, vec![header(17, 18, 20, 21)]);
        assert_http_err(walk_request_head(buf, &t), "expected LF after CR");
    }

    #[test]
    fn header_rejects_control_bytes_in_value() {
        let buf = b"GET /a HTTP/1.1\r\nX: a\0b\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 27, vec![header(17, 18, 20, 23)]);
        assert_http_err(walk_request_head(buf, &t), "invalid byte in header value");
        let buf = b"GET /a HTTP/1.1\r\nX: a\x7Fb\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 27, vec![header(17, 18, 20, 23)]);
        assert_http_err(walk_request_head(buf, &t), "invalid byte in header value");
    }

    #[test]
    fn header_accepts_obs_text_value() {
        let buf = b"GET /a HTTP/1.1\r\nX: a\x80\xFFb\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 28, vec![header(17, 18, 20, 24)]);
        walk_request_head(buf, &t).unwrap();
    }

    #[test]
    fn header_record_bijection_enforced() {
        // Bytes have one header, table has none.
        let mut t = get_table();
        t.headers = vec![];
        assert_http_err(
            walk_request_head(GET, &t),
            "header line without table record",
        );
        // Table has one extra record.
        let mut t = get_table();
        t.headers = vec![header(17, 21, 23, 24), header(17, 21, 23, 24)];
        assert_http_err(walk_request_head(GET, &t), "table has extra header records");
    }

    #[test]
    fn header_collects_framing_facts() {
        let buf = b"POST /a HTTP/1.1\r\nContent-Length: 5\r\n\r\n";
        let t = req(span(0, 4), span(5, 7), 39, vec![header(18, 32, 34, 35)]);
        let head = walk_request_head(buf, &t).unwrap();
        assert_eq!(head.info.content_length, Some(5));
        assert!(!head.info.dup_content_length);

        // Case-insensitive names; chunked TE value.
        let buf = b"POST /a HTTP/1.1\r\ntRANSFER-eNCODING: CHUNKED\r\n\r\n";
        let t = req(span(0, 4), span(5, 7), 48, vec![header(18, 35, 37, 44)]);
        let head = walk_request_head(buf, &t).unwrap();
        assert!(head.info.te_present);
        assert!(head.info.te_chunked);

        // Non-chunked TE value: present but not chunked.
        let buf = b"POST /a HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n";
        let t = req(span(0, 4), span(5, 7), 45, vec![header(18, 35, 37, 41)]);
        let head = walk_request_head(buf, &t).unwrap();
        assert!(head.info.te_present);
        assert!(!head.info.te_chunked);
    }

    #[test]
    fn header_flags_duplicates() {
        let buf = b"GET /a HTTP/1.1\r\nHost: x\r\nhost: y\r\n\r\n";
        let t = req(
            span(0, 3),
            span(4, 6),
            37,
            vec![header(17, 21, 23, 24), header(26, 30, 32, 33)],
        );
        let head = walk_request_head(buf, &t).unwrap();
        assert!(head.info.dup_host);

        let buf = b"GET /a HTTP/1.1\r\nContent-Length: 1\r\nCONTENT-LENGTH: 1\r\n\r\n";
        let t = req(
            span(0, 3),
            span(4, 6),
            57,
            vec![header(17, 31, 33, 34), header(36, 50, 52, 53)],
        );
        let head = walk_request_head(buf, &t).unwrap();
        assert!(head.info.dup_content_length);
    }

    #[test]
    fn header_malformed_content_length_is_immediate_error() {
        for (buf, head_end, ve) in [
            (
                &b"GET /a HTTP/1.1\r\nContent-Length: +5\r\n\r\n"[..],
                39,
                35,
            ),
            (
                &b"GET /a HTTP/1.1\r\nContent-Length: 0x5\r\n\r\n"[..],
                40,
                36,
            ),
            (
                &b"GET /a HTTP/1.1\r\nContent-Length: 5,5\r\n\r\n"[..],
                40,
                36,
            ),
            (
                &b"GET /a HTTP/1.1\r\nContent-Length: 5 5\r\n\r\n"[..],
                40,
                36,
            ),
        ] {
            let t = req(
                span(0, 3),
                span(4, 6),
                head_end,
                vec![header(17, 31, 33, ve)],
            );
            assert_http_err(walk_request_head(buf, &t), "malformed Content-Length value");
        }
        // Empty CL value (pinned at CR).
        let buf = b"GET /a HTTP/1.1\r\nContent-Length:\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 36, vec![header(17, 31, 32, 32)]);
        assert_http_err(walk_request_head(buf, &t), "malformed Content-Length value");
        // 20-digit CL value.
        let buf = b"GET /a HTTP/1.1\r\nContent-Length: 99999999999999999999\r\n\r\n";
        let t = req(span(0, 3), span(4, 6), 57, vec![header(17, 31, 33, 53)]);
        assert_http_err(walk_request_head(buf, &t), "malformed Content-Length value");
    }

    #[test]
    fn header_caps_at_128_lines() {
        // 129 header lines, each `A: b\r\n` (6 bytes), table listing all of
        // them: rejected on the byte-derived line count.
        let mut buf = Vec::from(&b"GET /a HTTP/1.1\r\n"[..]);
        let mut headers = Vec::new();
        for i in 0..129 {
            let base = 17 + i * 6;
            buf.extend_from_slice(b"A: b\r\n");
            headers.push(header(base, base + 1, base + 3, base + 4));
        }
        buf.extend_from_slice(b"\r\n");
        let head_end = buf.len();

        // 129 records: the table-side cap fires.
        let t = req(span(0, 3), span(4, 6), head_end, headers.clone());
        assert!(matches!(
            walk_request_head(&buf, &t),
            Err(Error::Table {
                reason: "more than 128 header records"
            })
        ));

        // 128 records but 129 byte lines: the line-count cap fires.
        headers.truncate(128);
        let t = req(span(0, 3), span(4, 6), head_end, headers);
        assert!(matches!(
            walk_request_head(&buf, &t),
            Err(Error::Table {
                reason: "more than 128 header lines"
            })
        ));
    }

    // === walk_response_head ===

    const OK_RESP: &[u8] = b"HTTP/1.1 200 OK\r\nHost: x\r\n\r\n";

    fn ok_table() -> ResponseSpans {
        resp(span(9, 12), span(13, 15), 28, vec![header(17, 21, 23, 24)])
    }

    #[test]
    fn response_head_happy_path() {
        let head = walk_response_head(OK_RESP, &ok_table()).unwrap();
        assert_eq!(head.head_end, 28);
        assert_eq!(head.status, 200);
        assert_eq!(head.info, ParsedHeadInfo::default());
    }

    #[test]
    fn response_head_no_reason_form() {
        // CRLF directly after the code: empty reason pinned at 12.
        let buf = b"HTTP/1.1 204\r\n\r\n";
        let head = walk_response_head(buf, &resp(span(9, 12), span(12, 12), 16, vec![])).unwrap();
        assert_eq!(head.status, 204);
        // Claiming the pin one byte off is rejected.
        assert_http_err(
            walk_response_head(buf, &resp(span(9, 12), span(13, 13), 16, vec![])),
            "reason span mismatch",
        );
    }

    #[test]
    fn response_head_sp_with_empty_reason() {
        // `SP` then CRLF: empty reason pinned at 13.
        let buf = b"HTTP/1.1 200 \r\n\r\n";
        let head = walk_response_head(buf, &resp(span(9, 12), span(13, 13), 17, vec![])).unwrap();
        assert_eq!(head.status, 200);
        assert_http_err(
            walk_response_head(buf, &resp(span(9, 12), span(12, 12), 17, vec![])),
            "reason span mismatch",
        );
    }

    #[test]
    fn response_head_reason_is_untrimmed() {
        // Reason " OK " (leading/trailing SP after the separator SP).
        let buf = b"HTTP/1.1 200  OK \r\n\r\n";
        walk_response_head(buf, &resp(span(9, 12), span(13, 17), 21, vec![])).unwrap();
        // A trimmed claim is rejected.
        assert_http_err(
            walk_response_head(buf, &resp(span(9, 12), span(14, 16), 21, vec![])),
            "reason span mismatch",
        );
    }

    #[test]
    fn response_head_reason_charset() {
        // obs-text accepted.
        let buf = b"HTTP/1.1 200 O\x80K\r\n\r\n";
        walk_response_head(buf, &resp(span(9, 12), span(13, 16), 20, vec![])).unwrap();
        // NUL rejected: the scan stops at it, so a span covering it can
        // never match, and the derived span fails the CRLF expectation.
        let buf = b"HTTP/1.1 200 O\0K\r\n\r\n";
        assert_http_err(
            walk_response_head(buf, &resp(span(9, 12), span(13, 16), 20, vec![])),
            "reason span mismatch",
        );
        assert_http_err(
            walk_response_head(buf, &resp(span(9, 12), span(13, 14), 20, vec![])),
            "expected CRLF after status line",
        );
        // DEL rejected likewise.
        let buf = b"HTTP/1.1 200 O\x7FK\r\n\r\n";
        assert_http_err(
            walk_response_head(buf, &resp(span(9, 12), span(13, 14), 20, vec![])),
            "expected CRLF after status line",
        );
    }

    #[test]
    fn response_head_rejects_bad_status_line() {
        let table = resp(span(9, 12), span(13, 15), 28, vec![]);
        // Wrong literal/case.
        assert_http_err(
            walk_response_head(b"http/1.1 200 OK\r\n\r\n", &table),
            "expected `HTTP/1.1 `",
        );
        assert_http_err(
            walk_response_head(b"HTTP/1.0 200 OK\r\n\r\n", &table),
            "expected `HTTP/1.1 `",
        );
        // First digit out of 1..=5.
        assert_http_err(
            walk_response_head(b"HTTP/1.1 600 OK\r\n\r\n", &table),
            "status code must start with a digit in 1-5",
        );
        assert_http_err(
            walk_response_head(b"HTTP/1.1 999 OK\r\n\r\n", &table),
            "status code must start with a digit in 1-5",
        );
        assert_http_err(
            walk_response_head(b"HTTP/1.1 099 OK\r\n\r\n", &table),
            "status code must start with a digit in 1-5",
        );
        // Non-digit in the code.
        assert_http_err(
            walk_response_head(b"HTTP/1.1 2x0 OK\r\n\r\n", &table),
            "status code must be 3 digits",
        );
        // Two-digit code: byte 11 is SP.
        assert_http_err(
            walk_response_head(b"HTTP/1.1 20 OK\r\n\r\n", &table),
            "status code must be 3 digits",
        );
        // Four-digit code: byte 12 is a digit, neither SP nor CR, so the
        // reason is pinned empty at 12 and the CRLF expectation fails.
        assert_http_err(
            walk_response_head(
                b"HTTP/1.1 2000\r\n\r\n",
                &resp(span(9, 12), span(12, 12), 17, vec![]),
            ),
            "expected CRLF after status line",
        );
        // Truncated.
        assert_http_err(
            walk_response_head(b"HTTP/1.1 20", &table),
            "truncated status line",
        );
        assert_http_err(walk_response_head(b"", &table), "expected `HTTP/1.1 `");
    }

    #[test]
    fn response_head_rejects_code_span_off_by_one() {
        // "20" instead of "200".
        assert_http_err(
            walk_response_head(OK_RESP, &resp(span(9, 11), span(13, 15), 28, vec![])),
            "status code span mismatch",
        );
        assert_http_err(
            walk_response_head(OK_RESP, &resp(span(10, 12), span(13, 15), 28, vec![])),
            "status code span mismatch",
        );
    }

    #[test]
    fn response_head_rejects_reason_span_off_by_one() {
        assert_http_err(
            walk_response_head(OK_RESP, &resp(span(9, 12), span(13, 14), 28, vec![])),
            "reason span mismatch",
        );
        assert_http_err(
            walk_response_head(OK_RESP, &resp(span(9, 12), span(12, 15), 28, vec![])),
            "reason span mismatch",
        );
        // Swallowing the CR.
        assert_http_err(
            walk_response_head(OK_RESP, &resp(span(9, 12), span(13, 16), 28, vec![])),
            "reason span mismatch",
        );
    }

    #[test]
    fn response_head_walks_headers_like_request() {
        let mut t = ok_table();
        t.headers = vec![];
        assert_http_err(
            walk_response_head(OK_RESP, &t),
            "header line without table record",
        );
        let mut t = ok_table();
        t.head_end = 27;
        assert_http_err(walk_response_head(OK_RESP, &t), "head_end mismatch");
    }

    // === walk_chunks ===

    #[test]
    fn chunks_multi_chunk_happy_path() {
        //          0   1    2 34567   8 9
        let buf = b"5\r\nhello\r\n3\r\nfoo\r\n0\r\nrest";
        let out = walk_chunks(buf, 0, 8).unwrap();
        assert_eq!(out.decoded, b"hellofoo");
        assert_eq!(out.trailer_start, 21);
        assert_eq!(
            out.chunk_map,
            vec![
                ChunkMapEntry {
                    src_start: 3,
                    body_offset: 0,
                    len: 5
                },
                ChunkMapEntry {
                    src_start: 13,
                    body_offset: 5,
                    len: 3
                },
            ]
        );
    }

    #[test]
    fn chunks_hex_sizes_both_cases() {
        let buf = b"A\r\n0123456789\r\n0\r\n";
        assert_eq!(walk_chunks(buf, 0, 0).unwrap().decoded, b"0123456789");
        let buf = b"a\r\n0123456789\r\n0\r\n";
        assert_eq!(walk_chunks(buf, 0, 0).unwrap().decoded, b"0123456789");
        // Leading zeros are valid HEXDIG grammar.
        let buf = b"05\r\nhello\r\n0\r\n";
        assert_eq!(walk_chunks(buf, 0, 0).unwrap().decoded, b"hello");
    }

    #[test]
    fn chunks_reject_leading_ows_and_signs() {
        assert_http_err(
            walk_chunks(b" 5\r\nhello\r\n0\r\n", 0, 0),
            "invalid chunk size",
        );
        assert_http_err(
            walk_chunks(b"+5\r\nhello\r\n0\r\n", 0, 0),
            "invalid chunk size",
        );
        assert_http_err(walk_chunks(b"\r\n0\r\n", 0, 0), "invalid chunk size");
        assert_http_err(walk_chunks(b"", 0, 0), "invalid chunk size");
        // Start past the end of the buffer.
        assert_http_err(walk_chunks(b"5\r\n", 9, 0), "invalid chunk size");
    }

    #[test]
    fn chunks_reject_seventeen_hexdigits() {
        assert_http_err(
            walk_chunks(b"00000000000000005\r\nhello\r\n0\r\n", 0, 0),
            "invalid chunk size",
        );
    }

    #[test]
    fn chunks_extension_grammar() {
        // Plain extension accepted.
        let buf = b"5;name=val\r\nhello\r\n0\r\n";
        assert_eq!(walk_chunks(buf, 0, 0).unwrap().decoded, b"hello");
        // BWS before the semicolon accepted; on the terminal chunk too.
        let buf = b"5 ;ext\r\nhello\r\n0;last\r\n";
        assert_eq!(walk_chunks(buf, 0, 0).unwrap().decoded, b"hello");
        // Trailing OWS with no extension accepted.
        let buf = b"5 \r\nhello\r\n0\r\n";
        assert_eq!(walk_chunks(buf, 0, 0).unwrap().decoded, b"hello");
        // First non-OWS byte after the size must be `;`.
        assert_http_err(
            walk_chunks(b"5 x\r\nhello\r\n0\r\n", 0, 0),
            "invalid chunk extension",
        );
        // A CR inside the extension breaks the CRLF expectation.
        assert_http_err(
            walk_chunks(b"5;e\rx\nhello\r\n0\r\n", 0, 0),
            "expected CRLF after chunk size",
        );
        // Bare LF terminating the size line.
        assert_http_err(
            walk_chunks(b"5\nhello\r\n0\r\n", 0, 0),
            "expected CRLF after chunk size",
        );
        // NUL in the extension.
        assert_http_err(
            walk_chunks(b"5;a\0b\r\nhello\r\n0\r\n", 0, 0),
            "expected CRLF after chunk size",
        );
    }

    #[test]
    fn chunks_reject_missing_crlf_after_data() {
        assert_http_err(
            walk_chunks(b"5\r\nhelloX\r\n0\r\n", 0, 0),
            "expected CRLF after chunk data",
        );
        // Data shorter than the size: the length-prefixed window swallows
        // the would-be terminator, so the walk runs off the end.
        assert_http_err(walk_chunks(b"5\r\nhi\r\n0\r\n", 0, 0), "invalid chunk size");
    }

    #[test]
    fn chunks_reject_size_beyond_buffer() {
        assert_http_err(
            walk_chunks(b"ff\r\nhi\r\n", 0, 0),
            "chunk data exceeds buffer",
        );
    }

    #[test]
    fn chunks_reject_sum_over_cap() {
        // 2^30 exactly is allowed by the cap (data bounds then reject).
        assert_http_err(
            walk_chunks(b"40000000\r\nhi\r\n", 0, 0),
            "chunk data exceeds buffer",
        );
        // 2^30 + 1 trips the cap before any data is read.
        assert_http_err(
            walk_chunks(b"40000001\r\nhi\r\n", 0, 0),
            "chunked body exceeds 2^30 bytes",
        );
        // u64::MAX as a single size trips the cap, not an overflow panic.
        assert_http_err(
            walk_chunks(b"ffffffffffffffff\r\nhi\r\n", 0, 0),
            "chunked body exceeds 2^30 bytes",
        );
    }

    #[test]
    fn chunks_capacity_hint_does_not_affect_acceptance() {
        let buf = b"5\r\nhello\r\n0\r\n";
        assert_eq!(
            walk_chunks(buf, 0, 0).unwrap(),
            walk_chunks(buf, 0, 1_000).unwrap()
        );
    }

    // === walk_trailers ===

    #[test]
    fn trailers_empty_section() {
        let buf = b"0\r\n\r\nX";
        assert_eq!(walk_trailers(buf, 3, &[]), Ok(5));
        // Truncated: no final CRLF.
        assert_http_err(walk_trailers(b"0\r\n", 3, &[]), "truncated trailer section");
        assert_http_err(walk_trailers(b"0\r\n\r", 3, &[]), "expected LF after CR");
    }

    #[test]
    fn trailers_single_trailer() {
        //          0123456 789       0
        let buf = b"X-Sum: abc\r\n\r\n";
        let expected = [header(0, 5, 7, 10)];
        assert_eq!(walk_trailers(buf, 0, &expected), Ok(14));
        // Value span off by one.
        let expected = [header(0, 5, 7, 9)];
        assert_http_err(
            walk_trailers(buf, 0, &expected),
            "header value span mismatch",
        );
    }

    #[test]
    fn trailers_lockstep_bijection() {
        let buf = b"X-Sum: abc\r\n\r\n";
        assert_http_err(
            walk_trailers(buf, 0, &[]),
            "trailer line without table record",
        );
        let expected = [header(0, 5, 7, 10), header(0, 5, 7, 10)];
        assert_http_err(
            walk_trailers(buf, 0, &expected),
            "table has extra trailer records",
        );
    }

    #[test]
    fn trailers_reject_restricted_names() {
        for buf in [
            &b"Content-Length: 3\r\n\r\n"[..],
            &b"content-length: 3\r\n\r\n"[..],
            &b"CONTENT-LENGTH: 3\r\n\r\n"[..],
        ] {
            let expected = [header(0, 14, 16, 17)];
            assert_http_err(walk_trailers(buf, 0, &expected), "restricted trailer name");
        }
        let buf = b"Transfer-Encoding: chunked\r\n\r\n";
        let expected = [header(0, 17, 19, 26)];
        assert_http_err(walk_trailers(buf, 0, &expected), "restricted trailer name");
        let buf = b"Host: x\r\n\r\n";
        let expected = [header(0, 4, 6, 7)];
        assert_http_err(walk_trailers(buf, 0, &expected), "restricted trailer name");
    }

    #[test]
    fn trailers_cap_at_128_lines() {
        let mut buf = Vec::new();
        let mut expected = Vec::new();
        for i in 0..129 {
            let base = i * 6;
            buf.extend_from_slice(b"A: b\r\n");
            expected.push(header(base, base + 1, base + 3, base + 4));
        }
        buf.extend_from_slice(b"\r\n");

        // Record-side cap.
        assert!(matches!(
            walk_trailers(&buf, 0, &expected),
            Err(Error::Table {
                reason: "more than 128 trailer records"
            })
        ));
        // Byte-line cap with 128 records.
        expected.truncate(128);
        assert!(matches!(
            walk_trailers(&buf, 0, &expected),
            Err(Error::Table {
                reason: "more than 128 trailer lines"
            })
        ));
    }
}
