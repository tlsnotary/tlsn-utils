//! Host-side span-table production (feature `parse`).
//!
//! Uses `spansy` for HTTP message structure, a small fallback head scanner
//! for the framings spansy cannot parse (close-delimited responses, HEAD
//! responses bearing `Content-Length`), and this crate's own span-emitting
//! JSON pass over the decoded body. Framing decisions reuse the validator's
//! shared derivation functions, and every emitted table is self-checked
//! with [`validate`](crate::validate()) before being returned.

pub(crate) mod http;
pub(crate) mod json;

use std::borrow::Cow;

use crate::{
    error::HostError,
    spans::{BodySpans, FORMAT_VERSION, Framing, HeaderSpan, JsonSpans, SpanTable},
};

/// Maximum buffer length accepted by the host: 2^30 bytes, mirroring the
/// validator's rule-A coordinate cap (kept in sync by the self-check).
const MAX_BUF_LEN: usize = 1 << 30;

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
pub fn parse_transcript(sent: &[u8], recv: &[u8]) -> Result<SpanTable, HostError> {
    // Mirror the validator's coordinate cap up front so oversized inputs
    // surface as `Unsupported`, not as a self-check failure.
    if sent.len() > MAX_BUF_LEN || recv.len() > MAX_BUF_LEN {
        return Err(HostError::Unsupported {
            feature: "buffer size",
            detail: format!(
                "buffers are limited to 2^30 bytes (sent: {}, recv: {})",
                sent.len(),
                recv.len()
            ),
        });
    }

    let mut request = http::request_spans(sent)?;
    let method_is_head = &sent[request.method.as_range()] == b"HEAD";
    let mut response = http::response_spans(recv, method_is_head)?;

    attach_json_claim(sent, &request.headers, &mut request.body)?;
    attach_json_claim(recv, &response.headers, &mut response.body)?;

    let table = SpanTable {
        version: FORMAT_VERSION,
        request,
        response,
    };

    // Self-check: host-accepted ⊆ validator-accepted, at the API boundary.
    if let Err(source) = crate::validate(sent, recv, &table) {
        return Err(HostError::Internal {
            reason: "emitted span table failed the validator self-check",
            source: Some(source),
        });
    }
    Ok(table)
}

/// Patches the JSON claim into `body`.
///
/// Per rule G1 the claim is the prover's choice: the host claims JSON only
/// when the message's FIRST `Content-Type` header (case-insensitive) has the
/// `application/json` media type AND the decoded body passes this crate's
/// JSON rules ([`json::emit`]); on a JSON parse failure it falls back to the
/// opaque claim (`json: None`) rather than failing the host parse. Bodies
/// with other (or missing) media types are never attempted.
fn attach_json_claim(
    buf: &[u8],
    headers: &[HeaderSpan],
    body: &mut Option<BodySpans>,
) -> Result<(), HostError> {
    let Some(body) = body.as_mut() else {
        return Ok(());
    };
    let Some(content_type) = http::first_header_value(buf, headers, b"content-type") else {
        return Ok(());
    };
    if !http::is_json_media_type(content_type) {
        return Ok(());
    }
    let content: Cow<'_, [u8]> = match body.framing {
        // Decoded-body coordinates: de-chunk with the validator's own
        // walker (the same bytes already walked once by `build_body`).
        Framing::Chunked => {
            let outcome = crate::validate::http::walk_chunks(
                buf,
                body.raw.start as usize,
                body.content_len as usize,
            )
            .map_err(|_| HostError::Internal {
                reason: "chunk re-walk failed after a successful body build",
                source: None,
            })?;
            Cow::Owned(outcome.decoded)
        }
        Framing::ContentLength | Framing::Close => Cow::Borrowed(&buf[body.raw.as_range()]),
    };
    if let Ok(nodes) = json::emit(&content) {
        body.json = Some(JsonSpans { nodes });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Span, validate};

    /// Reads a fixture pair from `fixtures/` (use a `synthetic/` prefix for
    /// the generated corpus).
    fn fixture(name: &str) -> (Vec<u8>, Vec<u8>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let read = |suffix: &str| {
            let path = dir.join(format!("{name}.{suffix}.bin"));
            std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
        };
        (read("sent"), read("recv"))
    }

    /// Parses a fixture pair. An `Ok` alone already proves the validator
    /// accepted the table: the host self-checks before returning.
    fn parse_fixture(name: &str) -> (Vec<u8>, Vec<u8>, SpanTable) {
        let (sent, recv) = fixture(name);
        let table = parse_transcript(&sent, &recv).unwrap_or_else(|e| panic!("{name}: {e}"));
        (sent, recv, table)
    }

    /// A minimal valid bodiless response for request-focused tests.
    const RECV_204: &[u8] = b"HTTP/1.1 204 No Content\r\n\r\n";
    /// A minimal valid request for response-focused tests.
    const SENT_GET: &[u8] = b"GET /r HTTP/1.1\r\nHost: t\r\n\r\n";

    fn unsupported_feature(err: HostError) -> &'static str {
        match err {
            HostError::Unsupported { feature, .. } => feature,
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    // === real fixtures (non-JSON content types only: reaching the JSON
    // emitter is out of scope for this module's tests) ===

    #[test]
    fn fixture_httpbingo_xml_content_length_xml() {
        let (sent, recv, table) = parse_fixture("httpbingo_xml");
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.request().method(), "GET");
        assert_eq!(t.request().target(), "/xml");
        assert!(t.request().body().is_none());
        assert_eq!(t.response().status(), 200);
        // Lowercase header names in the wild.
        assert_eq!(
            t.response().header("Content-Type").unwrap().value(),
            b"application/xml"
        );
        let body = t.response().body().unwrap();
        assert_eq!(body.framing(), Framing::ContentLength);
        assert!(body.json().is_none(), "non-JSON body must be opaque");
        let content = body.content();
        assert_eq!(content.len(), 522);
        assert_eq!(content[0], b'<');
        assert_eq!(*content.last().unwrap(), b'\n');
    }

    #[test]
    fn fixture_httpbingo_png_chunked_binary() {
        let (sent, recv, table) = parse_fixture("httpbingo_png");
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.response().status(), 200);
        assert_eq!(
            t.response().header("content-type").unwrap().value(),
            b"image/png"
        );
        let body = t.response().body().unwrap();
        assert_eq!(body.framing(), Framing::Chunked);
        assert!(body.json().is_none());
        let content = body.content();
        assert_eq!(content.len(), 8090);
        assert_eq!(&content[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(*content.last().unwrap(), 0x82); // IEND CRC tail
        // Two chunks captured: the decoded body maps back to two source
        // ranges.
        assert_eq!(body.content_to_source(0..content.len() as u32).len(), 2);
    }

    #[test]
    fn fixture_example_html_chunked_html() {
        let (sent, recv, table) = parse_fixture("example_html");
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.response().status(), 200);
        assert_eq!(
            t.response().header("Content-Type").unwrap().value(),
            b"text/html"
        );
        let body = t.response().body().unwrap();
        assert_eq!(body.framing(), Framing::Chunked);
        assert_eq!(body.content().len(), 559);
        assert!(body.content().starts_with(b"<!doctype html>"));
        assert!(body.content().ends_with(b"</html>\n"));
    }

    #[test]
    fn fixture_example_head_no_framing_headers() {
        let (sent, recv, table) = parse_fixture("example_head");
        // spansy cannot parse a bodiless 200 response without CL/TE at all:
        // an Ok proves the host's own fallback walk produced the spans.
        assert!(spansy::http::parse_response(&recv[..]).is_err());
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.request().method(), "HEAD");
        assert_eq!(t.response().status(), 200);
        assert!(t.response().body().is_none());
        assert_eq!(table.response.head_end as usize, recv.len());
    }

    #[test]
    fn fixture_github_head_content_length_without_body() {
        let (sent, recv, table) = parse_fixture("github_head");
        // The HEAD-with-Content-Length framing is unparseable for spansy
        // (finding F2): the fallback path, not spansy, produced this table.
        assert!(spansy::http::parse_response(&recv[..]).is_err());
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.request().method(), "HEAD");
        assert_eq!(t.response().status(), 200);
        assert_eq!(
            t.response().header("content-length").unwrap().value(),
            b"15"
        );
        assert!(t.response().body().is_none());
    }

    #[test]
    fn fixture_github_zen_text_plain() {
        let (sent, recv, table) = parse_fixture("github_zen");
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.response().status(), 200);
        assert_eq!(
            t.response().header("Content-Type").unwrap().value(),
            b"text/plain;charset=utf-8"
        );
        let body = t.response().body().unwrap();
        assert_eq!(body.framing(), Framing::ContentLength);
        assert!(body.json().is_none());
        assert_eq!(body.content().len(), 15);
        assert_eq!(body.content(), &recv[recv.len() - 15..]);
    }

    #[test]
    fn fixture_icanhazip_minimal_text() {
        let (sent, recv, table) = parse_fixture("icanhazip");
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.request().target(), "/");
        assert_eq!(t.response().status(), 200);
        assert_eq!(
            t.response().header("content-type").unwrap().value(),
            b"text/plain"
        );
        let body = t.response().body().unwrap();
        assert_eq!(body.framing(), Framing::ContentLength);
        assert_eq!(body.content().len(), 14);
        assert_eq!(*body.content().last().unwrap(), b'\n');
    }

    #[test]
    fn fixture_postman_204_no_body() {
        let (sent, recv, table) = parse_fixture("postman_204");
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.response().status(), 204);
        assert_eq!(t.response().reason(), "No Content");
        assert!(t.request().body().is_none());
        assert!(t.response().body().is_none());
        // Duplicate Set-Cookie names in the wild (different cases).
        assert!(t.response().headers_with_name("set-cookie").count() >= 2);
    }

    // === synthetic fixtures (non-JSON pairs) ===

    #[test]
    fn fixture_syn_empty_reason() {
        let (sent, recv, table) = parse_fixture("synthetic/syn_empty_reason");
        // `HTTP/1.1 200 \r\n`: SP present, empty reason pinned at 13.
        assert_eq!(table.response.reason, Span::new(13, 13));
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.response().status(), 200);
        assert_eq!(t.response().reason(), "");
        let body = t.response().body().unwrap();
        assert_eq!(body.framing(), Framing::ContentLength);
        assert_eq!(body.content().len(), 38);
    }

    #[test]
    fn fixture_syn_no_reason() {
        let (sent, recv, table) = parse_fixture("synthetic/syn_no_reason");
        // `HTTP/1.1 200\r\n`: no SP, empty reason pinned at 12.
        assert_eq!(table.response.reason, Span::new(12, 12));
        let t = validate(&sent, &recv, &table).unwrap();
        assert_eq!(t.response().reason(), "");
        assert_eq!(t.response().body().unwrap().content().len(), 39);
    }

    #[test]
    fn fixture_syn_obs_text_header_value() {
        let (sent, recv, table) = parse_fixture("synthetic/syn_obs_text");
        let t = validate(&sent, &recv, &table).unwrap();
        // Raw 0xC3 0xA9 bytes in the header value survive untouched.
        assert_eq!(
            t.response().header("X-Obs").unwrap().value(),
            "café".as_bytes()
        );
        assert_eq!(t.response().body().unwrap().content().len(), 49);
    }

    // === crafted transcripts: framings and shapes the corpus lacks ===

    #[test]
    fn close_delimited_response_via_fallback() {
        let recv =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nstream until close";
        // spansy rejects close-delimited responses outright (finding F2)...
        assert!(spansy::http::parse_response(&recv[..]).is_err());
        // ...so an Ok here proves the host's own walk produced the table.
        let table = parse_transcript(SENT_GET, recv).unwrap();
        let head_end = recv.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let body = table.response.body.as_ref().unwrap();
        assert_eq!(body.framing, Framing::Close);
        assert_eq!(body.raw, Span::new(head_end as u32, recv.len() as u32));
        assert_eq!(body.content_len, 18);
        let t = validate(SENT_GET, recv, &table).unwrap();
        let vb = t.response().body().unwrap();
        assert_eq!(vb.content(), b"stream until close");
        assert!(vb.json().is_none());
    }

    #[test]
    fn close_delimited_with_no_reason_and_binary_tail() {
        // A second spansy-unparseable-but-valid shape: close-delimited,
        // reason-less status line, no Content-Type, non-UTF-8 body bytes.
        // (spansy is not even consulted here: it PANICS on reason-less
        // status lines — see the guard in `http::response_spans` — so an Ok
        // can only come from the host's own walk.)
        let recv = b"HTTP/1.1 200\r\n\r\nraw close-delimited bytes \xC3\xA9\xFF";
        let table = parse_transcript(SENT_GET, recv).unwrap();
        assert_eq!(table.response.reason, Span::new(12, 12));
        let body = table.response.body.as_ref().unwrap();
        assert_eq!(body.framing, Framing::Close);
        // No Content-Type at all: the JSON attempt is never made (an
        // attempt would panic in the emitter stub).
        assert!(body.json.is_none());
        let t = validate(SENT_GET, recv, &table).unwrap();
        assert!(
            t.response()
                .body()
                .unwrap()
                .content()
                .ends_with(b"\xC3\xA9\xFF")
        );
    }

    #[test]
    fn head_with_content_length_and_zero_body() {
        let sent = b"HEAD /h HTTP/1.1\r\nHost: t\r\n\r\n";
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\n";
        // Without request context spansy fails its body bounds check.
        assert!(spansy::http::parse_response(&recv[..]).is_err());
        let table = parse_transcript(sent, recv).unwrap();
        assert!(table.response.body.is_none());
        assert_eq!(table.response.head_end as usize, recv.len());
        let t = validate(sent, recv, &table).unwrap();
        assert_eq!(t.response().header("Content-Length").unwrap().value(), b"5");
    }

    #[test]
    fn chunked_response_with_trailers() {
        let recv =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n\
                     5\r\nhello\r\n3\r\nfoo\r\n0\r\nX-Checksum: abc\r\nX-After: ok\r\n\r\n";
        let table = parse_transcript(SENT_GET, recv).unwrap();
        let body = table.response.body.as_ref().unwrap();
        assert_eq!(body.framing, Framing::Chunked);
        assert_eq!(body.content_len, 8);
        assert_eq!(body.trailers.len(), 2);
        let t = validate(SENT_GET, recv, &table).unwrap();
        let vb = t.response().body().unwrap();
        assert_eq!(vb.content(), b"hellofoo");
        assert_eq!(vb.trailer("x-checksum").unwrap().value(), b"abc");
        assert_eq!(vb.trailer("X-After").unwrap().value(), b"ok");
        // Trailers are a separate namespace, never returned as headers.
        assert!(t.response().header("X-Checksum").is_none());
    }

    #[test]
    fn chunked_request_body_text_plain() {
        let sent = b"POST /c HTTP/1.1\r\nHost: t\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n\
                     3\r\nabc\r\n1\r\nd\r\n0\r\n\r\n";
        let table = parse_transcript(sent, RECV_204).unwrap();
        let body = table.request.body.as_ref().unwrap();
        assert_eq!(body.framing, Framing::Chunked);
        assert_eq!(body.content_len, 4);
        assert!(body.json.is_none());
        let t = validate(sent, RECV_204, &table).unwrap();
        assert_eq!(t.request().body().unwrap().content(), b"abcd");
    }

    #[test]
    fn form_urlencoded_request_body() {
        let sent = b"POST /f HTTP/1.1\r\nHost: t\r\n\
                     Content-Type: application/x-www-form-urlencoded\r\nContent-Length: 7\r\n\r\na=1&b=2";
        // `X-Pad: v \r\n` doubles as a trailing-OWS-trim canary for the
        // spansy cross-check.
        let recv =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nX-Pad: v \r\n\r\nok";
        let table = parse_transcript(sent, recv).unwrap();
        let body = table.request.body.as_ref().unwrap();
        assert_eq!(body.framing, Framing::ContentLength);
        assert_eq!(body.content_len, 7);
        assert!(body.json.is_none(), "form body is opaque");
        let t = validate(sent, recv, &table).unwrap();
        assert_eq!(t.request().body().unwrap().content(), b"a=1&b=2");
        assert_eq!(t.response().body().unwrap().content(), b"ok");
        assert_eq!(t.response().header("X-Pad").unwrap().value(), b"v");
    }

    #[test]
    fn empty_header_values_pinned_at_cr() {
        //           0123456789012345678 9 0123 4 5678901 2 3...
        let sent = b"GET /e HTTP/1.1\r\nX-E:\r\nX-F: \r\nHost: t\r\n\r\n";
        let recv = b"HTTP/1.1 204 No Content\r\nX-G:\t\r\n\r\n";
        // An Ok means the validator agreed with every pin (self-check).
        let table = parse_transcript(sent, recv).unwrap();
        // `X-E:` — CR at 21; `X-F: ` — CR at 28, pinned past the OWS.
        assert_eq!(table.request.headers[0].value, Span::new(21, 21));
        assert_eq!(table.request.headers[1].value, Span::new(28, 28));
        // `X-G:<TAB>` — CR at 30.
        assert_eq!(table.response.headers[0].value, Span::new(30, 30));
        let t = validate(sent, recv, &table).unwrap();
        assert_eq!(t.request().header("X-E").unwrap().value(), b"");
        assert_eq!(t.request().header("x-f").unwrap().value(), b"");
        assert_eq!(t.response().header("X-G").unwrap().value(), b"");
    }

    #[test]
    fn obs_text_header_value_crafted() {
        let recv = b"HTTP/1.1 200 OK\r\nX-Obs: caf\xC3\xA9 \xFF\r\nContent-Length: 2\r\nContent-Type: text/plain\r\n\r\nok";
        let table = parse_transcript(SENT_GET, recv).unwrap();
        let t = validate(SENT_GET, recv, &table).unwrap();
        let header = t.response().header("x-obs").unwrap();
        assert_eq!(header.value(), b"caf\xC3\xA9 \xFF");
        // 0xFF alone is not UTF-8; the raw bytes are still exposed.
        assert!(header.value_str().is_none());
    }

    #[test]
    fn duplicate_set_cookie_headers() {
        let recv = b"HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\
                     Content-Length: 2\r\nContent-Type: text/plain\r\n\r\nok";
        let table = parse_transcript(SENT_GET, recv).unwrap();
        let t = validate(SENT_GET, recv, &table).unwrap();
        // First match wins; duplicates remain iterable.
        assert_eq!(t.response().header("set-cookie").unwrap().value(), b"a=1");
        let values: Vec<_> = t
            .response()
            .headers_with_name("SET-COOKIE")
            .map(|h| h.value())
            .collect();
        assert_eq!(values, [b"a=1", b"b=2"]);
    }

    #[test]
    fn json_content_type_without_body_is_never_attempted() {
        // A JSON Content-Type on a bodiless message must not reach the
        // emitter (which would panic in the stub): no body, no claim.
        let sent = b"GET /j HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\n\r\n";
        let recv = b"HTTP/1.1 204 No Content\r\nContent-Type: application/json\r\n\r\n";
        let table = parse_transcript(sent, recv).unwrap();
        assert!(table.request.body.is_none());
        assert!(table.response.body.is_none());
    }

    #[test]
    fn json_lookalike_media_type_is_not_attempted() {
        // Valid JSON bytes under a lookalike media type stay opaque (if the
        // extractor matched, the emitter stub would panic this test).
        let recv =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/jsonp\r\nContent-Length: 2\r\n\r\n{}";
        let table = parse_transcript(SENT_GET, recv).unwrap();
        let body = table.response.body.as_ref().unwrap();
        assert!(body.json.is_none());
    }

    // === negatives: spansy-lax inputs the host must reject up front ===

    #[test]
    fn rejects_plus_sign_content_length_in_request() {
        let sent = b"POST /p HTTP/1.1\r\nHost: t\r\nContent-Length: +5\r\n\r\nhello";
        // spansy itself accepts `+5` (Rust integer parsing laxness)...
        assert!(spansy::http::parse_request(&sent[..]).is_ok());
        // ...the host's strict walk must not.
        let err = parse_transcript(sent, RECV_204).unwrap_err();
        assert_eq!(unsupported_feature(err), "Content-Length");
    }

    #[test]
    fn rejects_plus_sign_content_length_in_response() {
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: +2\r\n\r\nok";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "Content-Length");
    }

    #[test]
    fn rejects_content_length_with_transfer_encoding() {
        // Valid chunked bytes, so spansy (which lets TE override CL)
        // parses happily; the shared derivation rejects (rule C1).
        let sent = b"POST /p HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\n\
                     Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(spansy::http::parse_request(&sent[..]).is_ok());
        let err = parse_transcript(sent, RECV_204).unwrap_err();
        let HostError::Unsupported { feature, detail } = err else {
            panic!("expected Unsupported");
        };
        assert_eq!(feature, "framing");
        assert!(
            detail.contains("Content-Length and Transfer-Encoding"),
            "{detail}"
        );
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nCONTENT-LENGTH: 2\r\n\r\nok";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "framing");
    }

    #[test]
    fn rejects_trailing_garbage_after_content_length_body() {
        let sent = b"POST /p HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\n\r\nhelloGARBAGE";
        assert!(spansy::http::parse_request(&sent[..]).is_ok());
        let err = parse_transcript(sent, RECV_204).unwrap_err();
        assert_eq!(unsupported_feature(err), "message framing");
        // Truncated bodies are equally rejected (response side; spansy
        // fails too, but the host's own walk classifies it).
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "message framing");
    }

    #[test]
    fn rejects_head_response_with_body_bytes() {
        let sent = b"HEAD /h HTTP/1.1\r\nHost: t\r\n\r\n";
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let err = parse_transcript(sent, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "message framing");
    }

    #[test]
    fn rejects_trailing_bytes_after_chunked_message() {
        let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\
                     Content-Type: text/plain\r\n\r\n2\r\nok\r\n0\r\n\r\nEXTRA";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "message framing");
    }

    #[test]
    fn rejects_restricted_trailer_name() {
        let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\
                     Content-Type: text/plain\r\n\r\n2\r\nok\r\n0\r\nContent-Length: 2\r\n\r\n";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "chunked trailers");
    }

    #[test]
    fn rejects_ows_chunk_size_spansy_accepts() {
        // spansy trims chunk-size lines; the validator (rule C6) does not.
        let sent = b"POST /c HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\
                     Content-Type: text/plain\r\n\r\n 3\r\nabc\r\n0\r\n\r\n";
        assert!(spansy::http::parse_request(&sent[..]).is_ok());
        let err = parse_transcript(sent, RECV_204).unwrap_err();
        assert_eq!(unsupported_feature(err), "http grammar");
    }

    #[test]
    fn rejects_http_1_0_either_side() {
        // spansy accepts HTTP/1.0 requests; the host pins HTTP/1.1.
        let sent = b"GET /a HTTP/1.0\r\nHost: t\r\n\r\n";
        assert!(spansy::http::parse_request(&sent[..]).is_ok());
        let err = parse_transcript(sent, RECV_204).unwrap_err();
        assert_eq!(unsupported_feature(err), "http version");

        let recv = b"HTTP/1.0 204 No Content\r\n\r\n";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "status line");
    }

    #[test]
    fn rejects_bad_status_codes() {
        for recv in [
            &b"HTTP/1.1 600 Oops\r\n\r\n"[..],
            &b"HTTP/1.1 999 X\r\n\r\n"[..],
        ] {
            let err = parse_transcript(SENT_GET, recv).unwrap_err();
            assert_eq!(unsupported_feature(err), "status line");
        }
    }

    #[test]
    fn rejects_header_injection_bytes_in_response() {
        // Bare LF terminating a header line.
        let recv = b"HTTP/1.1 204 No Content\r\nX: a\n\r\n";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "http grammar");
        // NUL inside a header value.
        let recv = b"HTTP/1.1 204 No Content\r\nX: a\0b\r\n\r\n";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "http grammar");
        // Obs-fold continuation line.
        let recv = b"HTTP/1.1 204 No Content\r\nX: a\r\n b\r\n\r\n";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "http grammar");
    }

    #[test]
    fn rejects_more_than_128_response_headers() {
        let mut recv = Vec::from(&b"HTTP/1.1 204 No Content\r\n"[..]);
        for _ in 0..129 {
            recv.extend_from_slice(b"A: b\r\n");
        }
        recv.extend_from_slice(b"\r\n");
        let err = parse_transcript(SENT_GET, &recv).unwrap_err();
        assert_eq!(unsupported_feature(err), "http head");
    }

    #[test]
    fn rejects_unsupported_transfer_coding() {
        let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\nxx";
        let err = parse_transcript(SENT_GET, recv).unwrap_err();
        let HostError::Unsupported { feature, detail } = err else {
            panic!("expected Unsupported");
        };
        assert_eq!(feature, "framing");
        assert!(detail.contains("unsupported transfer coding"), "{detail}");
    }

    #[test]
    fn spansy_error_propagates_for_unparseable_request() {
        // A truncated request head is a hard spansy error, not a fallback.
        let err = parse_transcript(b"GET /a HTTP/1.1\r\nHost: t\r\n", RECV_204).unwrap_err();
        assert!(matches!(err, HostError::Spansy(_)), "{err:?}");
    }

    #[test]
    fn table_spans_pin_request_line() {
        let table = parse_transcript(SENT_GET, RECV_204).unwrap();
        assert_eq!(table.version, FORMAT_VERSION);
        assert_eq!(table.request.method, Span::new(0, 3));
        assert_eq!(table.request.target, Span::new(4, 6));
        assert_eq!(table.request.head_end as usize, SENT_GET.len());
        assert_eq!(table.request.headers.len(), 1);
        assert_eq!(table.response.code, Span::new(9, 12));
        assert!(table.request.body.is_none());
        assert!(table.response.body.is_none());
    }
}
