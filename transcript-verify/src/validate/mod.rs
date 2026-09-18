//! Transcript validation — the in-VM entry point.
//!
//! [`validate`] performs a single forward pass over each buffer: table
//! well-formedness checks (rule group A), the request head walk (B), framing
//! derivation (C/D), the body walk with de-chunking (E), and lockstep JSON
//! checking (F). No rule may search — every record is pinned by cursor
//! equality.

pub(crate) mod http;
pub(crate) mod json;

use alloc::vec::Vec;

use crate::{
    error::Error,
    spans::{BodySpans, FORMAT_VERSION, Framing, SpanTable},
    transcript::{BodyData, Transcript, ValidatedBody},
};

/// Maximum buffer (and decoded-body) length: 2^30 bytes (rule A), so that
/// any two in-bounds offsets can be added without `u32` overflow.
const MAX_BUF_LEN: usize = 1 << 30;

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
    sent: &'a [u8],
    recv: &'a [u8],
    table: &'a SpanTable,
) -> Result<Transcript<'a>, Error> {
    // Rule group A: version and coordinate-space caps.
    if table.version != FORMAT_VERSION {
        return Err(Error::Version {
            expected: FORMAT_VERSION,
            found: table.version,
        });
    }
    if sent.len() > MAX_BUF_LEN {
        return Err(Error::TooLarge { len: sent.len() });
    }
    if recv.len() > MAX_BUF_LEN {
        return Err(Error::TooLarge { len: recv.len() });
    }

    // Request: head walk (B), framing derivation (C), body walk (C/E/F).
    let req_head = http::walk_request_head(sent, &table.request)?;
    let req_framing = derive_request_framing(&req_head.info)?;
    let req_body = check_body(
        sent,
        req_head.head_end,
        req_framing,
        table.request.body.as_ref(),
    )?;

    // Response: head walk (D), framing derivation (D), body walk (D/E/F).
    let resp_head = http::walk_response_head(recv, &table.response)?;
    let resp_framing = derive_response_framing(
        req_head.method_is_head,
        resp_head.status,
        &resp_head.info,
        recv.len() > resp_head.head_end,
    )?;
    let resp_body = check_body(
        recv,
        resp_head.head_end,
        resp_framing,
        table.response.body.as_ref(),
    )?;

    Ok(Transcript::new(
        sent,
        recv,
        table,
        resp_head.status,
        req_body,
        resp_body,
    ))
}

/// Checks a message's body record against the derived framing and walks the
/// body bytes (rule groups C, D, E; F via the JSON claim).
///
/// `head_end` is the verified first-body-byte offset; `derived` the framing
/// computed from the verified head. Returns the validated body, `None` when
/// the message has none.
fn check_body<'a>(
    buf: &'a [u8],
    head_end: usize,
    derived: DerivedFraming,
    record: Option<&BodySpans>,
) -> Result<Option<ValidatedBody<'a>>, Error> {
    // Normalize: `Content-Length: 0` yields no body section at all.
    let derived = match derived {
        DerivedFraming::ContentLength(0) => DerivedFraming::None,
        other => other,
    };

    if derived == DerivedFraming::None {
        if record.is_some() {
            return Err(Error::Framing {
                reason: "body record present but derived framing yields no body",
            });
        }
        // Coverage: a bodiless message must end exactly at the head.
        if head_end != buf.len() {
            return Err(Error::Http {
                at: head_end as u32,
                reason: "bytes remain after message without body",
            });
        }
        return Ok(None);
    }

    let Some(body) = record else {
        return Err(Error::Framing {
            reason: "missing body record for derived framing",
        });
    };
    let tag_matches = matches!(
        (derived, body.framing),
        (DerivedFraming::ContentLength(_), Framing::ContentLength)
            | (DerivedFraming::Chunked, Framing::Chunked)
            | (DerivedFraming::Close, Framing::Close)
    );
    if !tag_matches {
        return Err(Error::Framing {
            reason: "framing tag mismatch",
        });
    }

    match body.framing {
        Framing::ContentLength | Framing::Close => {
            if let DerivedFraming::ContentLength(len) = derived {
                // `head_end <= buf.len() <= 2^30` and `len <= 2^30`: the sum
                // cannot overflow a usize.
                if head_end + len as usize != buf.len() {
                    return Err(Error::Framing {
                        reason: "Content-Length body must end exactly at end of buffer",
                    });
                }
            }
            // Close framing extends to the end of the buffer by definition
            // (and `derive_response_framing` only yields it when bytes
            // remain), so for both framings the body is `head_end..len`.
            finish_contiguous_body(buf, head_end, body)
        }
        Framing::Chunked => {
            let hint = (body.content_len as usize).min(buf.len().saturating_sub(head_end));
            let outcome = http::walk_chunks(buf, head_end, hint)?;
            let end = http::walk_trailers(buf, outcome.trailer_start, &body.trailers)?;
            if end != buf.len() {
                return Err(Error::Http {
                    at: end as u32,
                    reason: "bytes remain after chunked message",
                });
            }
            http::check_span(body.raw, buf.len(), "body.raw")?;
            if body.raw.start as usize != head_end || body.raw.end as usize != end {
                return Err(Error::Framing {
                    reason: "body raw span mismatch",
                });
            }
            if body.content_len as usize != outcome.decoded.len() {
                return Err(Error::Framing {
                    reason: "content_len mismatch",
                });
            }
            check_json_claim(&outcome.decoded, body)?;
            Ok(Some(ValidatedBody {
                data: BodyData::Decoded(outcome.decoded),
                chunk_map: outcome.chunk_map,
            }))
        }
    }
}

/// Finishes validation of a contiguous (`ContentLength`/`Close`) body
/// spanning `head_end..buf.len()`.
fn finish_contiguous_body<'a>(
    buf: &'a [u8],
    head_end: usize,
    body: &BodySpans,
) -> Result<Option<ValidatedBody<'a>>, Error> {
    http::check_span(body.raw, buf.len(), "body.raw")?;
    if body.raw.start as usize != head_end || body.raw.end as usize != buf.len() {
        return Err(Error::Framing {
            reason: "body raw span mismatch",
        });
    }
    let content = &buf[head_end..];
    if body.content_len as usize != content.len() {
        return Err(Error::Framing {
            reason: "content_len mismatch",
        });
    }
    if !body.trailers.is_empty() {
        return Err(Error::Table {
            reason: "trailers on a non-chunked body",
        });
    }
    check_json_claim(content, body)?;
    Ok(Some(ValidatedBody {
        data: BodyData::Borrowed(content),
        chunk_map: Vec::new(),
    }))
}

/// Runs the lockstep JSON check if the body claims JSON (rule group F,
/// gated by the rule-A node cap).
fn check_json_claim(content: &[u8], body: &BodySpans) -> Result<(), Error> {
    if let Some(json) = &body.json {
        // Rule A: every node owns at least one body byte, so more nodes
        // than bytes can never validate; reject before walking.
        if json.nodes.len() > content.len() {
            return Err(Error::Table {
                reason: "more JSON nodes than body bytes",
            });
        }
        json::validate_json(content, &json.nodes)?;
    }
    Ok(())
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

/// Shared steps 1-3 of both framing derivations: duplicate flags, CL+TE
/// exclusion, and the supported-transfer-coding check.
fn check_head_facts(headers: &ParsedHeadInfo) -> Result<(), Error> {
    if headers.dup_content_length {
        return Err(Error::Framing {
            reason: "duplicate Content-Length header",
        });
    }
    if headers.dup_transfer_encoding {
        return Err(Error::Framing {
            reason: "duplicate Transfer-Encoding header",
        });
    }
    if headers.dup_host {
        return Err(Error::Framing {
            reason: "duplicate Host header",
        });
    }
    if headers.content_length.is_some() && headers.te_present {
        return Err(Error::Framing {
            reason: "Content-Length and Transfer-Encoding both present",
        });
    }
    if headers.te_present && !headers.te_chunked {
        return Err(Error::Framing {
            reason: "unsupported transfer coding",
        });
    }
    Ok(())
}

/// Maps a verified `Content-Length` value to a framing, enforcing the 2^30
/// coordinate cap.
fn content_length_framing(n: u64) -> Result<DerivedFraming, Error> {
    if n > MAX_BUF_LEN as u64 {
        return Err(Error::Framing {
            reason: "Content-Length exceeds 2^30",
        });
    }
    Ok(DerivedFraming::ContentLength(n as u32))
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
pub(crate) fn derive_request_framing(headers: &ParsedHeadInfo) -> Result<DerivedFraming, Error> {
    check_head_facts(headers)?;
    if headers.te_chunked {
        return Ok(DerivedFraming::Chunked);
    }
    if let Some(n) = headers.content_length {
        return content_length_framing(n);
    }
    Ok(DerivedFraming::None)
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
    request_method_is_head: bool,
    status: u16,
    headers: &ParsedHeadInfo,
    bytes_remain: bool,
) -> Result<DerivedFraming, Error> {
    check_head_facts(headers)?;
    if request_method_is_head || (100..=199).contains(&status) || status == 204 || status == 304 {
        return Ok(DerivedFraming::None);
    }
    if headers.te_chunked {
        return Ok(DerivedFraming::Chunked);
    }
    if let Some(n) = headers.content_length {
        return content_length_framing(n);
    }
    Ok(if bytes_remain {
        DerivedFraming::Close
    } else {
        DerivedFraming::None
    })
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::spans::{
        HeaderSpan, JsonKind, JsonNode, JsonSpans, RequestSpans, ResponseSpans, Span,
    };

    fn span(start: usize, end: usize) -> Span {
        Span::new(start as u32, end as u32)
    }

    fn header(ns: usize, ne: usize, vs: usize, ve: usize) -> HeaderSpan {
        HeaderSpan {
            name: span(ns, ne),
            value: span(vs, ve),
        }
    }

    fn body(framing: Framing, start: usize, end: usize, content_len: usize) -> BodySpans {
        BodySpans {
            framing,
            raw: span(start, end),
            content_len: content_len as u32,
            trailers: vec![],
            json: None,
        }
    }

    fn tbl(request: RequestSpans, response: ResponseSpans) -> SpanTable {
        SpanTable {
            version: FORMAT_VERSION,
            request,
            response,
        }
    }

    fn assert_framing_err(result: Result<Transcript<'_>, Error>, want: &str) {
        match result {
            Err(Error::Framing { reason }) => assert_eq!(reason, want),
            other => panic!("expected Framing {{ {want:?} }}, got {other:?}"),
        }
    }

    // === fixtures ===

    const GET_SENT: &[u8] = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";

    fn get_request() -> RequestSpans {
        RequestSpans {
            method: span(0, 3),
            target: span(4, 6),
            head_end: 28,
            headers: vec![header(17, 21, 23, 24)],
            body: None,
        }
    }

    const NO_CONTENT_RECV: &[u8] = b"HTTP/1.1 204 No Content\r\n\r\n";

    fn no_content_response(body: Option<BodySpans>) -> ResponseSpans {
        ResponseSpans {
            code: span(9, 12),
            reason: span(13, 23),
            head_end: 27,
            headers: vec![],
            body,
        }
    }

    const POST_CL_SENT: &[u8] = b"POST /p HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";

    fn post_cl_request(body: Option<BodySpans>) -> RequestSpans {
        RequestSpans {
            method: span(0, 4),
            target: span(5, 7),
            head_end: 39,
            headers: vec![header(18, 32, 34, 35)],
            body,
        }
    }

    const CL_RECV: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi";

    fn cl_response(body: Option<BodySpans>) -> ResponseSpans {
        ResponseSpans {
            code: span(9, 12),
            reason: span(13, 15),
            head_end: 38,
            headers: vec![header(17, 31, 33, 34)],
            body,
        }
    }

    // === framing derivation: requests ===

    fn cl_info(n: u64) -> ParsedHeadInfo {
        ParsedHeadInfo {
            content_length: Some(n),
            ..Default::default()
        }
    }

    fn chunked_info() -> ParsedHeadInfo {
        ParsedHeadInfo {
            te_present: true,
            te_chunked: true,
            ..Default::default()
        }
    }

    #[test]
    fn request_framing_decision_table() {
        let none = ParsedHeadInfo::default();
        assert_eq!(derive_request_framing(&none), Ok(DerivedFraming::None));
        assert_eq!(
            derive_request_framing(&cl_info(5)),
            Ok(DerivedFraming::ContentLength(5))
        );
        assert_eq!(
            derive_request_framing(&cl_info(0)),
            Ok(DerivedFraming::ContentLength(0))
        );
        assert_eq!(
            derive_request_framing(&chunked_info()),
            Ok(DerivedFraming::Chunked)
        );
        // TE wins over CL never happens: both present is rejected first.
        assert_eq!(
            derive_request_framing(&cl_info(1 << 30)),
            Ok(DerivedFraming::ContentLength(1 << 30))
        );
        assert_eq!(
            derive_request_framing(&cl_info((1 << 30) + 1)),
            Err(Error::Framing {
                reason: "Content-Length exceeds 2^30"
            })
        );
    }

    #[test]
    fn request_framing_rejects_toxic_heads() {
        let dup_cl = ParsedHeadInfo {
            dup_content_length: true,
            ..cl_info(5)
        };
        assert_eq!(
            derive_request_framing(&dup_cl),
            Err(Error::Framing {
                reason: "duplicate Content-Length header"
            })
        );
        let dup_te = ParsedHeadInfo {
            dup_transfer_encoding: true,
            ..chunked_info()
        };
        assert_eq!(
            derive_request_framing(&dup_te),
            Err(Error::Framing {
                reason: "duplicate Transfer-Encoding header"
            })
        );
        let dup_host = ParsedHeadInfo {
            dup_host: true,
            ..Default::default()
        };
        assert_eq!(
            derive_request_framing(&dup_host),
            Err(Error::Framing {
                reason: "duplicate Host header"
            })
        );
        // CL + TE (rule C1) — also when the TE value is not chunked.
        let cl_te = ParsedHeadInfo {
            content_length: Some(5),
            ..chunked_info()
        };
        assert_eq!(
            derive_request_framing(&cl_te),
            Err(Error::Framing {
                reason: "Content-Length and Transfer-Encoding both present"
            })
        );
        let cl_te_gzip = ParsedHeadInfo {
            content_length: Some(5),
            te_present: true,
            ..Default::default()
        };
        assert_eq!(
            derive_request_framing(&cl_te_gzip),
            Err(Error::Framing {
                reason: "Content-Length and Transfer-Encoding both present"
            })
        );
        // Unsupported transfer coding (rule C2).
        let te_gzip = ParsedHeadInfo {
            te_present: true,
            ..Default::default()
        };
        assert_eq!(
            derive_request_framing(&te_gzip),
            Err(Error::Framing {
                reason: "unsupported transfer coding"
            })
        );
    }

    // === framing derivation: responses ===

    #[test]
    fn response_framing_no_body_statuses() {
        // HEAD requests: never a body, regardless of CL/TE.
        assert_eq!(
            derive_response_framing(true, 200, &cl_info(5), false),
            Ok(DerivedFraming::None)
        );
        assert_eq!(
            derive_response_framing(true, 200, &chunked_info(), true),
            Ok(DerivedFraming::None)
        );
        // 1xx / 204 / 304, with CL and without.
        for status in [100, 101, 199, 204, 304] {
            assert_eq!(
                derive_response_framing(false, status, &cl_info(5), false),
                Ok(DerivedFraming::None),
                "status {status}"
            );
            assert_eq!(
                derive_response_framing(false, status, &ParsedHeadInfo::default(), false),
                Ok(DerivedFraming::None),
                "status {status}"
            );
        }
        // 304 + CL:0 is the documented ACCEPT case.
        assert_eq!(
            derive_response_framing(false, 304, &cl_info(0), false),
            Ok(DerivedFraming::None)
        );
    }

    #[test]
    fn response_framing_decision_table() {
        let none = ParsedHeadInfo::default();
        assert_eq!(
            derive_response_framing(false, 200, &chunked_info(), true),
            Ok(DerivedFraming::Chunked)
        );
        assert_eq!(
            derive_response_framing(false, 200, &cl_info(5), true),
            Ok(DerivedFraming::ContentLength(5))
        );
        assert_eq!(
            derive_response_framing(false, 205, &cl_info(5), false),
            Ok(DerivedFraming::ContentLength(5))
        );
        // Close-delimited only when neither CL nor TE is present.
        assert_eq!(
            derive_response_framing(false, 200, &none, true),
            Ok(DerivedFraming::Close)
        );
        assert_eq!(
            derive_response_framing(false, 200, &none, false),
            Ok(DerivedFraming::None)
        );
        assert_eq!(
            derive_response_framing(false, 200, &cl_info((1 << 30) + 1), true),
            Err(Error::Framing {
                reason: "Content-Length exceeds 2^30"
            })
        );
    }

    #[test]
    fn response_framing_checks_facts_before_status() {
        // Even a 204/HEAD response rejects toxic heads.
        let dup_cl = ParsedHeadInfo {
            dup_content_length: true,
            ..cl_info(5)
        };
        assert_eq!(
            derive_response_framing(false, 204, &dup_cl, false),
            Err(Error::Framing {
                reason: "duplicate Content-Length header"
            })
        );
        let cl_te = ParsedHeadInfo {
            content_length: Some(5),
            ..chunked_info()
        };
        assert_eq!(
            derive_response_framing(true, 200, &cl_te, false),
            Err(Error::Framing {
                reason: "Content-Length and Transfer-Encoding both present"
            })
        );
        let te_gzip = ParsedHeadInfo {
            te_present: true,
            ..Default::default()
        };
        assert_eq!(
            derive_response_framing(false, 200, &te_gzip, true),
            Err(Error::Framing {
                reason: "unsupported transfer coding"
            })
        );
    }

    // === validate() end-to-end (non-JSON transcripts only) ===

    #[test]
    fn validate_get_no_body() {
        let table = tbl(get_request(), no_content_response(None));
        assert!(validate(GET_SENT, NO_CONTENT_RECV, &table).is_ok());
    }

    #[test]
    fn validate_post_content_length_bodies() {
        let table = tbl(
            post_cl_request(Some(body(Framing::ContentLength, 39, 44, 5))),
            cl_response(Some(body(Framing::ContentLength, 38, 40, 2))),
        );
        assert!(validate(POST_CL_SENT, CL_RECV, &table).is_ok());
    }

    const CHUNKED_SENT: &[u8] =
        b"POST /c HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";

    fn chunked_request(body: Option<BodySpans>) -> RequestSpans {
        RequestSpans {
            method: span(0, 4),
            target: span(5, 7),
            head_end: 48,
            headers: vec![header(18, 35, 37, 44)],
            body,
        }
    }

    const CHUNKED_RECV: &[u8] =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nX-T: v\r\n\r\n";

    fn chunked_response(body: Option<BodySpans>) -> ResponseSpans {
        ResponseSpans {
            code: span(9, 12),
            reason: span(13, 15),
            head_end: 47,
            headers: vec![header(17, 34, 36, 43)],
            body,
        }
    }

    #[test]
    fn validate_chunked_request_and_response_with_trailers() {
        let mut resp_body = body(Framing::Chunked, 47, 68, 3);
        resp_body.trailers = vec![header(58, 61, 63, 64)];
        let table = tbl(
            chunked_request(Some(body(Framing::Chunked, 48, 63, 5))),
            chunked_response(Some(resp_body)),
        );
        assert!(validate(CHUNKED_SENT, CHUNKED_RECV, &table).is_ok());
    }

    #[test]
    fn validate_chunked_rejects_missing_trailer_record() {
        // Same bytes, but the table omits the trailer record.
        let table = tbl(
            chunked_request(Some(body(Framing::Chunked, 48, 63, 5))),
            chunked_response(Some(body(Framing::Chunked, 47, 68, 3))),
        );
        assert!(matches!(
            validate(CHUNKED_SENT, CHUNKED_RECV, &table),
            Err(Error::Http {
                reason: "trailer line without table record",
                ..
            })
        ));
    }

    #[test]
    fn validate_head_response_with_content_length_and_no_body() {
        let sent = b"HEAD /h HTTP/1.1\r\n\r\n";
        let request = RequestSpans {
            method: span(0, 4),
            target: span(5, 7),
            head_end: 20,
            headers: vec![],
            body: None,
        };
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n";
        let response = ResponseSpans {
            code: span(9, 12),
            reason: span(13, 15),
            head_end: 38,
            headers: vec![header(17, 31, 33, 34)],
            body: None,
        };
        assert!(validate(sent, recv, &tbl(request, response)).is_ok());
    }

    #[test]
    fn validate_rejects_204_with_body_bytes() {
        let recv = b"HTTP/1.1 204 No Content\r\n\r\nops";
        // No body record: trailing bytes are uncovered.
        let table = tbl(get_request(), no_content_response(None));
        assert!(matches!(
            validate(GET_SENT, recv, &table),
            Err(Error::Http {
                at: 27,
                reason: "bytes remain after message without body"
            })
        ));
        // A body record cannot legitimize them either.
        let table = tbl(
            get_request(),
            no_content_response(Some(body(Framing::Close, 27, 30, 3))),
        );
        assert_framing_err(
            validate(GET_SENT, recv, &table),
            "body record present but derived framing yields no body",
        );
    }

    const CLOSE_RECV: &[u8] = b"HTTP/1.1 200 OK\r\n\r\nstream until close";

    fn close_response(body: Option<BodySpans>) -> ResponseSpans {
        ResponseSpans {
            code: span(9, 12),
            reason: span(13, 15),
            head_end: 19,
            headers: vec![],
            body,
        }
    }

    #[test]
    fn validate_close_delimited_response() {
        let table = tbl(
            get_request(),
            close_response(Some(body(Framing::Close, 19, 37, 18))),
        );
        assert!(validate(GET_SENT, CLOSE_RECV, &table).is_ok());

        // A close body stopping early is rejected.
        let table = tbl(
            get_request(),
            close_response(Some(body(Framing::Close, 19, 30, 11))),
        );
        assert_framing_err(
            validate(GET_SENT, CLOSE_RECV, &table),
            "body raw span mismatch",
        );
    }

    #[test]
    fn validate_rejects_close_claim_despite_content_length() {
        // recv carries `Content-Length: 2`; claiming Close hides framing.
        let table = tbl(
            get_request(),
            cl_response(Some(body(Framing::Close, 38, 40, 2))),
        );
        assert_framing_err(validate(GET_SENT, CL_RECV, &table), "framing tag mismatch");
        // And `Transfer-Encoding: chunked` claimed Close.
        let table = tbl(
            get_request(),
            chunked_response(Some(body(Framing::Close, 47, 68, 21))),
        );
        assert_framing_err(
            validate(GET_SENT, CHUNKED_RECV, &table),
            "framing tag mismatch",
        );
    }

    #[test]
    fn validate_rejects_trailing_bytes_after_cl_body() {
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhix";
        let table = tbl(
            get_request(),
            cl_response(Some(body(Framing::ContentLength, 38, 41, 3))),
        );
        assert_framing_err(
            validate(GET_SENT, recv, &table),
            "Content-Length body must end exactly at end of buffer",
        );
        // Truncated body (one byte short) is equally rejected.
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nh";
        let table = tbl(
            get_request(),
            cl_response(Some(body(Framing::ContentLength, 38, 39, 1))),
        );
        assert_framing_err(
            validate(GET_SENT, recv, &table),
            "Content-Length body must end exactly at end of buffer",
        );
    }

    #[test]
    fn validate_rejects_second_message_after_chunked_end() {
        let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\nX";
        let table = tbl(
            get_request(),
            chunked_response(Some(body(Framing::Chunked, 47, 52, 0))),
        );
        assert!(matches!(
            validate(GET_SENT, recv, &table),
            Err(Error::Http {
                at: 52,
                reason: "bytes remain after chunked message"
            })
        ));
    }

    #[test]
    fn validate_rejects_version_mismatch() {
        let mut table = tbl(get_request(), no_content_response(None));
        table.version = 2;
        assert!(matches!(
            validate(GET_SENT, NO_CONTENT_RECV, &table),
            Err(Error::Version {
                expected: FORMAT_VERSION,
                found: 2
            })
        ));
    }

    #[test]
    fn validate_rejects_request_close_framing() {
        // A request is never close-delimited (rule C5): no CL/TE means no
        // body, so the record is rejected and the bytes are uncovered.
        let sent = b"GET /a HTTP/1.1\r\n\r\nbody";
        let request = RequestSpans {
            method: span(0, 3),
            target: span(4, 6),
            head_end: 19,
            headers: vec![],
            body: Some(body(Framing::Close, 19, 23, 4)),
        };
        let table = tbl(request, no_content_response(None));
        assert_framing_err(
            validate(sent, NO_CONTENT_RECV, &table),
            "body record present but derived framing yields no body",
        );
        // With Content-Length present, a Close tag is a mismatch.
        let table = tbl(
            post_cl_request(Some(body(Framing::Close, 39, 44, 5))),
            no_content_response(None),
        );
        assert_framing_err(
            validate(POST_CL_SENT, NO_CONTENT_RECV, &table),
            "framing tag mismatch",
        );
    }

    #[test]
    fn validate_content_length_zero_means_no_body_record() {
        let sent = b"GET /z HTTP/1.1\r\nContent-Length: 0\r\n\r\n";
        let request = RequestSpans {
            method: span(0, 3),
            target: span(4, 6),
            head_end: 38,
            headers: vec![header(17, 31, 33, 34)],
            body: None,
        };
        let table = tbl(request.clone(), no_content_response(None));
        assert!(validate(sent, NO_CONTENT_RECV, &table).is_ok());

        // A body record for CL(0) is rejected.
        let mut request = request;
        request.body = Some(body(Framing::ContentLength, 38, 38, 0));
        let table = tbl(request, no_content_response(None));
        assert_framing_err(
            validate(sent, NO_CONTENT_RECV, &table),
            "body record present but derived framing yields no body",
        );
    }

    #[test]
    fn validate_rejects_missing_body_record() {
        let table = tbl(post_cl_request(None), no_content_response(None));
        assert_framing_err(
            validate(POST_CL_SENT, NO_CONTENT_RECV, &table),
            "missing body record for derived framing",
        );
    }

    #[test]
    fn validate_rejects_body_record_mismatches() {
        // raw span shifted.
        let table = tbl(
            post_cl_request(Some(body(Framing::ContentLength, 40, 44, 5))),
            no_content_response(None),
        );
        assert_framing_err(
            validate(POST_CL_SENT, NO_CONTENT_RECV, &table),
            "body raw span mismatch",
        );
        // raw span out of bounds.
        let table = tbl(
            post_cl_request(Some(body(Framing::ContentLength, 39, 4400, 5))),
            no_content_response(None),
        );
        assert!(matches!(
            validate(POST_CL_SENT, NO_CONTENT_RECV, &table),
            Err(Error::SpanOutOfBounds {
                what: "body.raw",
                ..
            })
        ));
        // content_len off by one.
        let table = tbl(
            post_cl_request(Some(body(Framing::ContentLength, 39, 44, 4))),
            no_content_response(None),
        );
        assert_framing_err(
            validate(POST_CL_SENT, NO_CONTENT_RECV, &table),
            "content_len mismatch",
        );
        // Trailers on a non-chunked body.
        let mut b = body(Framing::ContentLength, 39, 44, 5);
        b.trailers = vec![header(0, 1, 2, 3)];
        let table = tbl(post_cl_request(Some(b)), no_content_response(None));
        assert!(matches!(
            validate(POST_CL_SENT, NO_CONTENT_RECV, &table),
            Err(Error::Table {
                reason: "trailers on a non-chunked body"
            })
        ));
        // Chunked content_len disagreeing with the decode.
        let table = tbl(
            chunked_request(Some(body(Framing::Chunked, 48, 63, 6))),
            no_content_response(None),
        );
        assert_framing_err(
            validate(CHUNKED_SENT, NO_CONTENT_RECV, &table),
            "content_len mismatch",
        );
    }

    #[test]
    fn validate_rejects_json_node_overflow() {
        // 6 nodes for a 5-byte body trips the rule-A cap before the JSON
        // walk is ever entered.
        let node = JsonNode {
            kind: JsonKind::Null,
            start: 0,
            end: 1,
            size: 1,
        };
        let mut b = body(Framing::ContentLength, 39, 44, 5);
        b.json = Some(JsonSpans {
            nodes: vec![node; 6],
        });
        let table = tbl(post_cl_request(Some(b)), no_content_response(None));
        assert!(matches!(
            validate(POST_CL_SENT, NO_CONTENT_RECV, &table),
            Err(Error::Table {
                reason: "more JSON nodes than body bytes"
            })
        ));
    }

    #[test]
    fn validate_rejects_duplicate_content_length_end_to_end() {
        let sent = b"GET /a HTTP/1.1\r\nContent-Length: 1\r\nCONTENT-LENGTH: 1\r\n\r\n";
        let request = RequestSpans {
            method: span(0, 3),
            target: span(4, 6),
            head_end: 57,
            headers: vec![header(17, 31, 33, 34), header(36, 50, 52, 53)],
            body: None,
        };
        let table = tbl(request, no_content_response(None));
        assert_framing_err(
            validate(sent, NO_CONTENT_RECV, &table),
            "duplicate Content-Length header",
        );
    }

    #[test]
    fn validate_rejects_content_length_with_transfer_encoding() {
        let sent = b"POST /p HTTP/1.1\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n";
        let request = RequestSpans {
            method: span(0, 4),
            target: span(5, 7),
            head_end: 67,
            headers: vec![header(18, 32, 34, 35), header(37, 54, 56, 63)],
            body: None,
        };
        let table = tbl(request, no_content_response(None));
        assert_framing_err(
            validate(sent, NO_CONTENT_RECV, &table),
            "Content-Length and Transfer-Encoding both present",
        );
    }
}
