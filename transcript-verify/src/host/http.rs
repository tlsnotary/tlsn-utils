//! Host-side HTTP flattening: spansy types → table spans, plus the fallback
//! head scanner for framings spansy cannot parse.
//!
//! Span positions are never copied out of spansy blindly: header lines,
//! trailer lines, and the status line are re-walked with the *validator's
//! own* scanners ([`scan_header_line`], [`expect_lit`], [`walk_chunks`]), so
//! the emitted spans match what the validator derives BY CONSTRUCTION. This
//! sidesteps finding F1 (spansy loses the position of empty header values
//! and empty reason phrases) and finding F2 (spansy cannot parse
//! close-delimited responses or HEAD responses bearing `Content-Length`).
//! Framing decisions funnel through the validator's shared
//! [`derive_request_framing`] / [`derive_response_framing`].

use crate::{
    error::HostError,
    spans::{BodySpans, Framing, HeaderSpan, RequestSpans, ResponseSpans, Span},
    validate::{
        DerivedFraming, ParsedHeadInfo, derive_request_framing, derive_response_framing,
        http::{
            MAX_HEADER_LINES, expect_lit, is_ows, is_value_byte, parse_dec_u64, scan_header_line,
            skip_ows, walk_chunks,
        },
    },
};

/// Shorthand for [`HostError::Unsupported`].
fn unsupported(feature: &'static str, detail: impl Into<String>) -> HostError {
    HostError::Unsupported {
        feature,
        detail: detail.into(),
    }
}

/// Shorthand for a sourceless [`HostError::Internal`].
fn internal(reason: &'static str) -> HostError {
    HostError::Internal {
        reason,
        source: None,
    }
}

/// Maps a validator walk error over host input to
/// [`HostError::Unsupported`]: the bytes themselves violate a rule the
/// validator enforces, so no table this host could emit would validate.
fn map_http(e: crate::Error) -> HostError {
    unsupported("http grammar", e.to_string())
}

/// Maps a shared framing-derivation error to [`HostError::Unsupported`]
/// (e.g. `Content-Length` + `Transfer-Encoding` both present, duplicate
/// `Content-Length`/`Transfer-Encoding`/`Host`, a non-chunked transfer
/// coding).
fn map_framing(e: crate::Error) -> HostError {
    unsupported("framing", e.to_string())
}

/// Builds a table span from `usize` cursor positions.
///
/// Callers guarantee the coordinates fit `u32`: [`super::parse_transcript`]
/// rejects buffers over 2^30 bytes up front, mirroring the validator's
/// rule-A cap.
fn span(start: usize, end: usize) -> Span {
    Span::new(start as u32, end as u32)
}

/// Flattens the request head and body framing of `sent` into table spans.
///
/// Parses with `spansy` first (its error is propagated as
/// [`HostError::Spansy`]), takes the method/target spans from its views, and
/// then re-walks the header region and body with the validator's own
/// scanners (re-deriving empty-header-value positions locally, since spansy
/// loses them — finding F1). The body's `json` claim is left `None`; the
/// orchestrator ([`super::parse_transcript`]) de-chunks and patches the
/// claim in.
pub(crate) fn request_spans(sent: &[u8]) -> Result<RequestSpans, HostError> {
    let parsed = spansy::http::parse_request(sent)?;

    // Method/target are never empty, so their view offsets are reliable.
    let method_view = parsed.request.method.view();
    let method = span(
        method_view.offset(),
        method_view.offset() + method_view.len(),
    );
    let target_view = parsed.request.target.view();
    let target = span(
        target_view.offset(),
        target_view.offset() + target_view.len(),
    );

    if method.start != 0 {
        return Err(internal("spansy method span does not start at byte 0"));
    }
    if sent.get(method.end as usize) != Some(&b' ') || target.start != method.end + 1 {
        return Err(unsupported(
            "request line",
            "method and target must be separated by a single SP",
        ));
    }
    // Pin the exact version the validator requires; spansy also accepts
    // HTTP/1.0 here.
    let headers_start = expect_lit(sent, target.end as usize, b" HTTP/1.1\r\n", "")
        .map_err(|_| unsupported("http version", "request line must end ` HTTP/1.1` + CRLF"))?;

    let walked = walk_head_lines(sent, headers_start)?;
    cross_check_headers(&parsed.headers, &walked.headers)?;

    let framing = derive_request_framing(&walked.info).map_err(map_framing)?;
    let body = build_body(sent, walked.head_end, framing)?;

    Ok(RequestSpans {
        method,
        target,
        head_end: walked.head_end as u32,
        headers: walked.headers,
        body,
    })
}

/// Flattens the response head and body framing of `recv` into table spans.
///
/// Needs request context (`request_method_is_head`) for framing. The spans
/// are built by this module's own scan-based walk; spansy CANNOT parse two
/// legal framings (finding F2: close-delimited responses, HEAD responses
/// bearing `Content-Length`), so `spansy::http::parse_response` is consulted
/// only opportunistically as a sanity cross-check and its failure is
/// silently ignored. As with [`request_spans`], the `json` claim is left
/// `None` for the orchestrator to patch.
pub(crate) fn response_spans(
    recv: &[u8],
    request_method_is_head: bool,
) -> Result<ResponseSpans, HostError> {
    let status_line = walk_status_line(recv)?;
    let walked = walk_head_lines(recv, status_line.line_end)?;

    // Opportunistic spansy cross-check; a spansy error alone never fails the
    // host parse (finding F2 framings are valid but unparseable for it).
    //
    // Guard: spansy must not even be CALLED on a reason-less status line
    // (`HTTP/1.1 200\r\n`, no SP) — httparse hands it a static empty reason
    // slice not backed by `recv`, and spansy's `get_range` pointer
    // subtraction (spansy/src/http/span.rs:263) overflows on it (panics in
    // debug). The SP-form empty reason (`HTTP/1.1 200 \r\n`) is safe.
    let reasonless = recv.get(12) != Some(&b' ');
    if !reasonless && let Ok(parsed) = spansy::http::parse_response(recv) {
        let code = parsed.status.code.view();
        if code.offset() != 9 || code.len() != 3 {
            return Err(internal("spansy and host walk disagree on the code span"));
        }
        // Empty reason views lose their position in spansy (finding F1), so
        // only non-empty reasons are comparable.
        let reason = parsed.status.reason.view();
        if !reason.is_empty()
            && !status_line.reason.is_empty()
            && (reason.offset() != status_line.reason.start as usize
                || reason.len() != status_line.reason.len() as usize)
        {
            return Err(internal("spansy and host walk disagree on the reason span"));
        }
        cross_check_headers(&parsed.headers, &walked.headers)?;
    }

    let framing = derive_response_framing(
        request_method_is_head,
        status_line.status,
        &walked.info,
        recv.len() > walked.head_end,
    )
    .map_err(map_framing)?;
    let body = build_body(recv, walked.head_end, framing)?;

    Ok(ResponseSpans {
        code: status_line.code,
        reason: status_line.reason,
        head_end: walked.head_end as u32,
        headers: walked.headers,
        body,
    })
}

/// Facts from the host's own status-line walk, mirroring the validator's
/// `HTTP/1.1 NNN[ SP reason]\r\n` rules (D1).
struct WalkedStatusLine {
    /// The status-code span, pinned to `[9, 12)`.
    code: Span,
    /// The reason span: untrimmed; empty and pinned after the code (or after
    /// the SP) when absent.
    reason: Span,
    /// The parsed status code, `100..=599`.
    status: u16,
    /// Cursor one past the status line's CRLF.
    line_end: usize,
}

/// Walks the status line of `recv`.
fn walk_status_line(buf: &[u8]) -> Result<WalkedStatusLine, HostError> {
    let p = expect_lit(buf, 0, b"HTTP/1.1 ", "")
        .map_err(|_| unsupported("status line", "response must start with `HTTP/1.1 `"))?;
    debug_assert_eq!(p, 9);
    if buf.len() < 12 {
        return Err(unsupported("status line", "truncated status line"));
    }
    if !(b'1'..=b'5').contains(&buf[9]) || !buf[10].is_ascii_digit() || !buf[11].is_ascii_digit() {
        return Err(unsupported(
            "status line",
            "status code must be 3 digits with the first in 1-5",
        ));
    }
    let status =
        u16::from(buf[9] - b'0') * 100 + u16::from(buf[10] - b'0') * 10 + u16::from(buf[11] - b'0');

    let mut p = 12;
    let (reason_start, reason_end) = if p < buf.len() && buf[p] == b' ' {
        // `SP reason` form: everything up to the CR, untrimmed.
        p += 1;
        let start = p;
        while p < buf.len() && is_value_byte(buf[p]) {
            p += 1;
        }
        (start, p)
    } else {
        // No-reason form: empty reason pinned directly after the code.
        (p, p)
    };
    let line_end =
        expect_lit(buf, p, b"\r\n", "expected CRLF after status line").map_err(|_| {
            unsupported(
                "status line",
                "expected CRLF after the status line (CR/LF/NUL/DEL are not reason bytes)",
            )
        })?;

    Ok(WalkedStatusLine {
        code: span(9, 12),
        reason: span(reason_start, reason_end),
        status,
        line_end,
    })
}

/// One message head's worth of facts from the host's own header-line walk.
struct WalkedHeaders {
    /// Cursor one past the blank line's CRLF (== `head_end`).
    head_end: usize,
    /// Header spans built directly from the validator's own line scanner, so
    /// positions (including empty-value CR pinning) match the validator by
    /// construction.
    headers: Vec<HeaderSpan>,
    /// Framing facts for the shared derivation functions.
    info: ParsedHeadInfo,
}

/// Walks header lines from `start` until the blank line terminating the
/// head, building [`HeaderSpan`]s and collecting [`ParsedHeadInfo`].
///
/// Strictness violations (`+5` `Content-Length`, bare LF, control bytes,
/// more than 128 lines, ...) surface as [`HostError::Unsupported`].
fn walk_head_lines(buf: &[u8], start: usize) -> Result<WalkedHeaders, HostError> {
    let mut p = start;
    let mut headers = Vec::new();
    let mut info = ParsedHeadInfo::default();
    let (mut cl_seen, mut te_seen, mut host_seen) = (false, false, false);
    loop {
        if p >= buf.len() {
            return Err(unsupported(
                "http head",
                format!("truncated head at byte {p}"),
            ));
        }
        if buf[p] == b'\r' {
            p = expect_lit(buf, p, b"\r\n", "expected LF after CR").map_err(map_http)?;
            break;
        }
        if headers.len() >= MAX_HEADER_LINES {
            return Err(unsupported("http head", "more than 128 header lines"));
        }
        let h = scan_header_line(buf, p).map_err(map_http)?;
        let name = &buf[h.name_start..h.name_end];
        let value = &buf[h.value_start..h.value_end];
        if name.eq_ignore_ascii_case(b"content-length") {
            info.dup_content_length |= cl_seen;
            cl_seen = true;
            // Strict grammar (1..=19 DIGITs), exactly as the validator;
            // spansy would accept e.g. `+5` here (rule C3).
            let n = parse_dec_u64(value).ok_or_else(|| {
                unsupported(
                    "Content-Length",
                    format!("not 1..=19 DIGITs: {:?}", String::from_utf8_lossy(value)),
                )
            })?;
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
        headers.push(HeaderSpan {
            name: span(h.name_start, h.name_end),
            value: span(h.value_start, h.value_end),
        });
        p = h.line_end;
    }
    Ok(WalkedHeaders {
        head_end: p,
        headers,
        info,
    })
}

/// Cross-checks spansy's parsed headers against the host's own walk.
///
/// Only position facts spansy preserves reliably are compared: the header
/// count, name spans, and the value spans of NON-empty values (empty spansy
/// views lose their position — finding F1). A mismatch is an
/// [`HostError::Internal`] bug in this module, not bad input: both parsers
/// accepted the same bytes.
fn cross_check_headers<S: spansy::Store>(
    parsed: &[spansy::http::Header<S>],
    walked: &[HeaderSpan],
) -> Result<(), HostError> {
    if parsed.len() != walked.len() {
        return Err(internal("spansy and host walk disagree on header count"));
    }
    for (sh, wh) in parsed.iter().zip(walked) {
        let name = sh.name.view();
        if name.offset() != wh.name.start as usize || name.len() != wh.name.len() as usize {
            return Err(internal("spansy and host walk disagree on a name span"));
        }
        let value = sh.value.view();
        if !value.is_empty()
            && !wh.value.is_empty()
            && (value.offset() != wh.value.start as usize || value.len() != wh.value.len() as usize)
        {
            return Err(internal("spansy and host walk disagree on a value span"));
        }
    }
    Ok(())
}

/// Builds the body record for a message whose verified head ends at
/// `head_end`, per the framing derived by the validator's shared functions.
///
/// Enforces, with `Unsupported` errors, the same coverage the validator
/// will: the message must end exactly at the end of its buffer. The `json`
/// claim is left `None`.
fn build_body(
    buf: &[u8],
    head_end: usize,
    derived: DerivedFraming,
) -> Result<Option<BodySpans>, HostError> {
    let body = match derived {
        DerivedFraming::None | DerivedFraming::ContentLength(0) => {
            if head_end != buf.len() {
                return Err(unsupported(
                    "message framing",
                    format!(
                        "{} trailing bytes after a bodiless message",
                        buf.len() - head_end
                    ),
                ));
            }
            None
        }
        DerivedFraming::ContentLength(n) => {
            // `head_end <= buf.len() <= 2^30` and `n <= 2^30`: no overflow.
            if head_end + n as usize != buf.len() {
                return Err(unsupported(
                    "message framing",
                    format!(
                        "Content-Length is {n} but {} body bytes remain",
                        buf.len() - head_end
                    ),
                ));
            }
            Some(BodySpans {
                framing: Framing::ContentLength,
                raw: span(head_end, buf.len()),
                content_len: n,
                trailers: Vec::new(),
                json: None,
            })
        }
        DerivedFraming::Chunked => {
            // The validator's own table-free de-chunker (rule C6).
            let outcome = walk_chunks(buf, head_end, 0).map_err(map_http)?;
            let (end, trailers) = walk_trailer_lines(buf, outcome.trailer_start)?;
            if end != buf.len() {
                return Err(unsupported(
                    "message framing",
                    format!(
                        "{} trailing bytes after the chunked message",
                        buf.len() - end
                    ),
                ));
            }
            Some(BodySpans {
                framing: Framing::Chunked,
                raw: span(head_end, end),
                content_len: outcome.decoded.len() as u32,
                trailers,
                json: None,
            })
        }
        DerivedFraming::Close => Some(BodySpans {
            framing: Framing::Close,
            raw: span(head_end, buf.len()),
            content_len: (buf.len() - head_end) as u32,
            trailers: Vec::new(),
            json: None,
        }),
    };
    Ok(body)
}

/// Walks the trailer section of a chunked body starting at `start` (the
/// first byte after the terminal chunk's CRLF), BUILDING the trailer
/// [`HeaderSpan`]s.
///
/// The validator's `walk_trailers` checks records in lockstep, so the host
/// scans the lines itself with the same `scan_header_line`, applying the
/// same restricted-name and line-cap rules (C7). Returns the cursor one past
/// the final CRLF — the chunked message end — and the trailer records.
fn walk_trailer_lines(buf: &[u8], start: usize) -> Result<(usize, Vec<HeaderSpan>), HostError> {
    let mut p = start;
    let mut trailers = Vec::new();
    loop {
        if p >= buf.len() {
            return Err(unsupported(
                "chunked trailers",
                format!("truncated trailer section at byte {p}"),
            ));
        }
        if buf[p] == b'\r' {
            p = expect_lit(buf, p, b"\r\n", "expected LF after CR").map_err(map_http)?;
            break;
        }
        if trailers.len() >= MAX_HEADER_LINES {
            return Err(unsupported(
                "chunked trailers",
                "more than 128 trailer lines",
            ));
        }
        let h = scan_header_line(buf, p).map_err(map_http)?;
        let name = &buf[h.name_start..h.name_end];
        if name.eq_ignore_ascii_case(b"content-length")
            || name.eq_ignore_ascii_case(b"transfer-encoding")
            || name.eq_ignore_ascii_case(b"host")
        {
            return Err(unsupported(
                "chunked trailers",
                format!(
                    "restricted trailer name: {:?}",
                    String::from_utf8_lossy(name)
                ),
            ));
        }
        trailers.push(HeaderSpan {
            name: span(h.name_start, h.name_end),
            value: span(h.value_start, h.value_end),
        });
        p = h.line_end;
    }
    Ok((p, trailers))
}

/// Extracts the media type from a `Content-Type` value: the bytes before the
/// first `;`, OWS-trimmed on both sides (parameters such as `charset` are
/// ignored).
pub(crate) fn media_type(value: &[u8]) -> &[u8] {
    let end = value.iter().position(|&b| b == b';').unwrap_or(value.len());
    let mt = &value[skip_ows(&value[..end], 0)..end];
    let mut trimmed = mt.len();
    while trimmed > 0 && is_ows(mt[trimmed - 1]) {
        trimmed -= 1;
    }
    &mt[..trimmed]
}

/// Returns `true` if the `Content-Type` value's media type is exactly
/// `application/json`, ASCII case-insensitive. `application/jsonp` and
/// suffixed types like `application/ld+json` do NOT match; parameters
/// (`;charset=utf-8`) are ignored.
pub(crate) fn is_json_media_type(value: &[u8]) -> bool {
    media_type(value).eq_ignore_ascii_case(b"application/json")
}

/// Returns the value bytes of the FIRST header named `name`
/// (ASCII case-insensitive) among `headers`, resolved against `buf`.
pub(crate) fn first_header_value<'a>(
    buf: &'a [u8],
    headers: &[HeaderSpan],
    name: &[u8],
) -> Option<&'a [u8]> {
    headers
        .iter()
        .find(|h| buf[h.name.as_range()].eq_ignore_ascii_case(name))
        .map(|h| &buf[h.value.as_range()])
}

#[cfg(test)]
mod tests {
    use super::*;

    // === media_type / is_json_media_type ===

    #[test]
    fn media_type_strips_parameters_and_ows() {
        assert_eq!(media_type(b"application/json"), b"application/json");
        assert_eq!(
            media_type(b"application/json;charset=utf-8"),
            b"application/json"
        );
        assert_eq!(
            media_type(b"application/json ; charset=utf-8"),
            b"application/json"
        );
        assert_eq!(media_type(b"application/json\t;x=1"), b"application/json");
        assert_eq!(media_type(b"text/plain"), b"text/plain");
        // Degenerate values stay degenerate, never panic.
        assert_eq!(media_type(b""), b"");
        assert_eq!(media_type(b";x=1"), b"");
        assert_eq!(media_type(b" \t "), b"");
    }

    #[test]
    fn json_media_type_matches_case_insensitively_with_params() {
        assert!(is_json_media_type(b"application/json"));
        assert!(is_json_media_type(b"application/json;charset=utf-8"));
        assert!(is_json_media_type(b"APPLICATION/JSON; charset=UTF-8"));
        assert!(is_json_media_type(b"Application/Json"));
    }

    #[test]
    fn json_media_type_rejects_lookalikes() {
        // Prefix/suffix lookalikes must NOT match.
        assert!(!is_json_media_type(b"application/jsonp"));
        assert!(!is_json_media_type(b"application/json-seq"));
        assert!(!is_json_media_type(b"application/ld+json"));
        assert!(!is_json_media_type(b"text/json"));
        assert!(!is_json_media_type(b"application/xml"));
        assert!(!is_json_media_type(b""));
        // The parameter must not rescue a non-JSON type.
        assert!(!is_json_media_type(b"text/plain;profile=application/json"));
    }

    // === first_header_value ===

    #[test]
    fn first_header_value_is_case_insensitive_first_match() {
        //               0         1         2
        //               0123456789012345678901234567
        let buf = b"Content-Type: a\r\nCONTENT-TYPE: b\r\n";
        let headers = [
            HeaderSpan {
                name: Span::new(0, 12),
                value: Span::new(14, 15),
            },
            HeaderSpan {
                name: Span::new(17, 29),
                value: Span::new(31, 32),
            },
        ];
        assert_eq!(
            first_header_value(buf, &headers, b"content-type"),
            Some(&b"a"[..])
        );
        assert_eq!(first_header_value(buf, &headers, b"x-missing"), None);
        assert_eq!(first_header_value(buf, &[], b"content-type"), None);
    }

    // === walk_status_line ===

    #[test]
    fn status_line_forms() {
        let s = walk_status_line(b"HTTP/1.1 200 OK\r\n").unwrap();
        assert_eq!(
            (s.code, s.reason, s.status, s.line_end),
            (Span::new(9, 12), Span::new(13, 15), 200, 17)
        );
        // SP + empty reason: pinned at 13.
        let s = walk_status_line(b"HTTP/1.1 404 \r\n").unwrap();
        assert_eq!((s.reason, s.status), (Span::new(13, 13), 404));
        // No SP at all: pinned at 12.
        let s = walk_status_line(b"HTTP/1.1 204\r\n").unwrap();
        assert_eq!((s.reason, s.status), (Span::new(12, 12), 204));
        // Untrimmed reason with obs-text.
        let s = walk_status_line(b"HTTP/1.1 500  Oops \xFF\r\n").unwrap();
        assert_eq!((s.reason, s.status), (Span::new(13, 20), 500));
    }

    #[test]
    fn status_line_rejections() {
        for bad in [
            &b"HTTP/1.0 200 OK\r\n"[..],   // wrong version
            &b"http/1.1 200 OK\r\n"[..],   // wrong case
            &b"HTTP/1.1 600 OK\r\n"[..],   // first digit out of 1..=5
            &b"HTTP/1.1 0xA OK\r\n"[..],   // non-digits
            &b"HTTP/1.1 20\r\n"[..],       // 2-digit code
            &b"HTTP/1.1 200 OK\n"[..],     // bare LF
            &b"HTTP/1.1 200 O\0K\r\n"[..], // NUL in reason
            &b"HTTP/1.1 20"[..],           // truncated
            &b""[..],
        ] {
            assert!(
                matches!(
                    walk_status_line(bad),
                    Err(HostError::Unsupported {
                        feature: "status line",
                        ..
                    })
                ),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    // === walk_head_lines / walk_trailer_lines ===

    #[test]
    fn head_lines_build_spans_and_facts() {
        //               0          1          2          3
        let buf = b"X-A: v\r\nContent-Length: 12\r\n\r\n";
        let walked = walk_head_lines(buf, 0).unwrap();
        assert_eq!(walked.head_end, buf.len());
        assert_eq!(
            walked.headers,
            vec![
                HeaderSpan {
                    name: Span::new(0, 3),
                    value: Span::new(5, 6),
                },
                HeaderSpan {
                    name: Span::new(8, 22),
                    value: Span::new(24, 26),
                },
            ]
        );
        assert_eq!(walked.info.content_length, Some(12));
        assert!(!walked.info.dup_content_length);
    }

    #[test]
    fn head_lines_reject_truncation_and_overflow() {
        assert!(matches!(
            walk_head_lines(b"X-A: v\r\n", 0),
            Err(HostError::Unsupported {
                feature: "http head",
                ..
            })
        ));
        let mut buf = Vec::new();
        for _ in 0..129 {
            buf.extend_from_slice(b"A: b\r\n");
        }
        buf.extend_from_slice(b"\r\n");
        assert!(matches!(
            walk_head_lines(&buf, 0),
            Err(HostError::Unsupported {
                feature: "http head",
                ..
            })
        ));
    }

    #[test]
    fn trailer_lines_build_spans_and_reject_restricted_names() {
        let buf = b"X-Sum: abc\r\n\r\n";
        let (end, trailers) = walk_trailer_lines(buf, 0).unwrap();
        assert_eq!(end, buf.len());
        assert_eq!(
            trailers,
            vec![HeaderSpan {
                name: Span::new(0, 5),
                value: Span::new(7, 10),
            }]
        );

        for restricted in [
            &b"Content-Length: 3\r\n\r\n"[..],
            &b"transfer-encoding: chunked\r\n\r\n"[..],
            &b"HOST: x\r\n\r\n"[..],
        ] {
            assert!(matches!(
                walk_trailer_lines(restricted, 0),
                Err(HostError::Unsupported {
                    feature: "chunked trailers",
                    ..
                })
            ));
        }
        assert!(matches!(
            walk_trailer_lines(b"X: y\r\n", 6),
            Err(HostError::Unsupported {
                feature: "chunked trailers",
                ..
            })
        ));
    }

    // === response_spans: request context drives framing ===

    #[test]
    fn response_spans_uses_request_context() {
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\n\r\nok";
        // As a GET response: a 2-byte Content-Length body.
        let resp = response_spans(recv, false).unwrap();
        let body = resp.body.expect("body");
        assert_eq!(body.framing, Framing::ContentLength);
        assert_eq!(body.content_len, 2);
        // The same bytes as a HEAD response would have to end at the head.
        assert!(matches!(
            response_spans(recv, true),
            Err(HostError::Unsupported {
                feature: "message framing",
                ..
            })
        ));
        // HEAD + Content-Length + zero body bytes: valid, no body record.
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\n\r\n";
        let resp = response_spans(recv, true).unwrap();
        assert!(resp.body.is_none());
        assert_eq!(resp.head_end as usize, recv.len());
    }
}
