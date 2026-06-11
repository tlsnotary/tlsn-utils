//! V2 soundness probes against `validate`.
//!
//! Each test is an ATTACK building `(sent, recv, table)`. A test named
//! `forgery_*` that PASSES means `validate` accepted a table misrepresenting
//! the bytes -- a CRITICAL soundness break. A `defended_*` test asserts the
//! rejection of an attack class (passes when the code is sound), documenting
//! coverage.

use transcript_verify::{
    BodySpans, FORMAT_VERSION, Framing, HeaderSpan, JsonKind, JsonNode, JsonSpans, RequestSpans,
    ResponseSpans, Span, SpanTable, validate,
};

fn sp(start: u32, end: u32) -> Span {
    Span::new(start, end)
}

fn hdr(ns: u32, ne: u32, vs: u32, ve: u32) -> HeaderSpan {
    HeaderSpan {
        name: sp(ns, ne),
        value: sp(vs, ve),
    }
}

fn node(kind: JsonKind, start: u32, end: u32, size: u32) -> JsonNode {
    JsonNode {
        kind,
        start,
        end,
        size,
    }
}

const GET_SENT: &[u8] = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";

fn get_request() -> RequestSpans {
    RequestSpans {
        method: sp(0, 3),
        target: sp(4, 6),
        head_end: 28,
        headers: vec![hdr(17, 21, 23, 24)],
        body: None,
    }
}

/// `200 OK` + Content-Length JSON body over `content`. Head is fixed-length so
/// every span is hand-pinned. Returns `(recv, table)`; caller may mutate.
fn cl_json(content: &[u8], nodes: Vec<JsonNode>) -> (Vec<u8>, SpanTable) {
    let cl_val = format!("{}", content.len());
    let mut recv = Vec::new();
    recv.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: ");
    let cl_start = recv.len() as u32;
    recv.extend_from_slice(cl_val.as_bytes());
    let cl_end = recv.len() as u32;
    recv.extend_from_slice(b"\r\n\r\n");
    let head_end = recv.len() as u32;
    recv.extend_from_slice(content);
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 15),
        head_end,
        headers: vec![hdr(17, 31, cl_start, cl_end)],
        body: Some(BodySpans {
            framing: Framing::ContentLength,
            raw: sp(head_end, head_end + content.len() as u32),
            content_len: content.len() as u32,
            trailers: vec![],
            json: Some(JsonSpans { nodes }),
        }),
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response,
    };
    (recv, table)
}

fn set_nodes(table: &mut SpanTable, nodes: Vec<JsonNode>) {
    table.response.body.as_mut().unwrap().json = Some(JsonSpans { nodes });
}

// === harness sanity ===

#[test]
fn sanity_honest_json_validates() {
    let (recv, table) = cl_json(
        br#"{"a":1}"#,
        vec![
            node(JsonKind::Object, 0, 7, 3),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 5, 6, 1),
        ],
    );
    assert!(validate(GET_SENT, &recv, &table).is_ok(), "harness broken");
}

// ===========================================================================
// PROBE 1: root Number claimed shorter than maximal lexeme. scan_number is the
// authority; node.end is equality-checked against the scan, and the cursor is
// advanced to the SCAN end (not the claimed end). Expect rejection.
// ===========================================================================
#[test]
fn defended_root_number_not_maximal() {
    let (recv, mut table) = cl_json(b"123", vec![node(JsonKind::Number, 0, 3, 1)]);
    set_nodes(&mut table, vec![node(JsonKind::Number, 0, 2, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: short root number"
    );
}

// ===========================================================================
// PROBE 2: a non-number byte glued to a number inside an array. The byte after
// the lexeme must be a separator; 'x' is not. Expect rejection.
// ===========================================================================
#[test]
fn defended_number_glued_garbage_in_array() {
    let (recv, table) = cl_json(
        b"[1x]",
        vec![
            node(JsonKind::Array, 0, 4, 2),
            node(JsonKind::Number, 1, 2, 1),
        ],
    );
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: 'x' as separator"
    );
}

// ===========================================================================
// PROBE 3: string boundary inside an escape. content bytes: " a \ \ " (the two
// backslashes are an escaped backslash), so the only terminator is the final
// quote and content is [1,4). Claiming a shorter end must fail.
// ===========================================================================
#[test]
fn defended_string_boundary_inside_escape() {
    let mut content = vec![b'"', b'a', b'\\', b'\\', b'"'];
    assert_eq!(content.len(), 5);
    let (recv, mut table) = cl_json(&content, vec![node(JsonKind::String, 1, 4, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_ok(),
        "honest escaped-backslash string"
    );
    set_nodes(&mut table, vec![node(JsonKind::String, 1, 2, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: string end inside escape"
    );
    content.clear();
}

// ===========================================================================
// PROBE 4: duplicate object keys that DECODE equal via '/' raw vs '\/' escape.
// RFC 8259 lets '/' be escaped or not. decode_key maps both to '/'. The unit
// tests covered \" and \t aliases but not \/. Expect duplicate rejection.
// Build {"/":1,"\/":2}.
// ===========================================================================
#[test]
fn defended_duplicate_slash_escape_keys() {
    // bytes: { " / " : 1 , " \ / " : 2 }
    let content: Vec<u8> = vec![
        b'{', b'"', b'/', b'"', b':', b'1', b',', b'"', b'\\', b'/', b'"', b':', b'2', b'}',
    ];
    assert_eq!(content.len(), 14);
    // key1 content [2,3) "/"; num1 [5,6); key2 content [8,10) "\/"; num2 [12,13)
    let nodes = vec![
        node(JsonKind::Object, 0, 14, 5),
        node(JsonKind::Key, 2, 3, 1),
        node(JsonKind::Number, 5, 6, 1),
        node(JsonKind::Key, 8, 10, 1),
        node(JsonKind::Number, 12, 13, 1),
    ];
    let (recv, table) = cl_json(&content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: '/' vs '\\/' dup keys"
    );
}

// ===========================================================================
// PROBE 5: duplicate keys equal via \b (0x08). decode_key maps \b -> 0x08; a
// raw 0x08 is a control byte rejected by scan_string. So the only collision is
// \b vs . Build {"\b":1,"":2}. Expect rejection.
// ===========================================================================
#[test]
fn defended_duplicate_backspace_escape_keys() {
    // key1 = \b (2 bytes), key2 =  (6 bytes)
    let content: Vec<u8> = {
        let mut v = vec![b'{', b'"', b'\\', b'b', b'"', b':', b'1', b',', b'"'];
        v.extend_from_slice(b"\\u0008");
        v.extend_from_slice(b"\":2}");
        v
    };
    // { at0 "at1 key1[2,4) "at4 :5 1@6 ,7 "@8 key2[9,15) "@15 :16 2@17 }@18 ->
    // len19
    assert_eq!(content.len(), 19);
    let nodes = vec![
        node(JsonKind::Object, 0, 19, 5),
        node(JsonKind::Key, 2, 4, 1),
        node(JsonKind::Number, 6, 7, 1),
        node(JsonKind::Key, 9, 15, 1),
        node(JsonKind::Number, 17, 18, 1),
    ];
    let (recv, table) = cl_json(&content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: backspace dup keys"
    );
}

// ===========================================================================
// PROBE 6: smuggle a second response after a Content-Length body. recv has a
// CL:2 body "hi" then an extra "X". The honest message ends at buf-1. Any table
// claiming the body must satisfy head_end + len == buf.len(), so the trailing
// "X" cannot be hidden. Expect rejection.
// ===========================================================================
#[test]
fn defended_cl_trailing_smuggle() {
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhiX";
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 15),
        head_end: 38,
        headers: vec![hdr(17, 31, 33, 34)],
        body: Some(BodySpans {
            framing: Framing::ContentLength,
            raw: sp(38, 40),
            content_len: 2,
            trailers: vec![],
            json: None,
        }),
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response,
    };
    assert!(
        validate(GET_SENT, recv, &table).is_err(),
        "FORGERY: trailing byte after CL body"
    );
}

// ===========================================================================
// PROBE 7: Content-Length value with a leading-zero pad that overstates the
// claimed content_len vs raw. CL: 02 over body "hi" (len 2). parse_dec_u64
// accepts "02" -> 2. content_len must equal 2. Try claiming content_len 3 and
// raw covering an extra byte. There IS no extra byte; expect rejection of any
// inconsistent table. (sanity: honest 02 validates; tamper rejected)
// ===========================================================================
#[test]
fn defended_cl_leading_zero_consistent() {
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 02\r\n\r\nhi";
    let mk = |content_len: u32, raw_end: u32| SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response: ResponseSpans {
            code: sp(9, 12),
            reason: sp(13, 15),
            head_end: 39,
            headers: vec![hdr(17, 31, 33, 35)],
            body: Some(BodySpans {
                framing: Framing::ContentLength,
                raw: sp(39, raw_end),
                content_len,
                trailers: vec![],
                json: None,
            }),
        },
    };
    // Honest: CL "02" -> 2 bytes "hi".
    assert!(
        validate(GET_SENT, recv, &mk(2, 41)).is_ok(),
        "honest CL 02 must validate"
    );
    // Tamper: claim content_len 3 (overstating) -> must reject.
    assert!(
        validate(GET_SENT, recv, &mk(3, 41)).is_err(),
        "FORGERY: content_len overstated"
    );
}

// ===========================================================================
// PROBE 8: chunked body, claim content_len SMALLER than the true decode while
// keeping raw correct. recv decodes "abc" (3 bytes). Claim content_len 2.
// content_len is equality-checked vs the real decode; expect rejection.
// ===========================================================================
#[test]
fn defended_chunked_content_len_understated() {
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    let mk = |content_len: u32| SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response: ResponseSpans {
            code: sp(9, 12),
            reason: sp(13, 15),
            head_end: 47,
            headers: vec![hdr(17, 34, 36, 43)],
            body: Some(BodySpans {
                framing: Framing::Chunked,
                raw: sp(47, recv.len() as u32),
                content_len,
                trailers: vec![],
                json: None,
            }),
        },
    };
    assert!(
        validate(GET_SENT, recv, &mk(3)).is_ok(),
        "honest chunked must validate"
    );
    assert!(
        validate(GET_SENT, recv, &mk(2)).is_err(),
        "FORGERY: chunked content_len understated"
    );
}

// ===========================================================================
// PROBE 9: chunk-size line with uppercase hex 'A' (10 bytes). Honest decode is
// 10 bytes. Then try to expose the size line itself as body by shifting raw.
// raw is equality-checked to head_end..end; expect rejection of a shifted raw.
// ===========================================================================
#[test]
fn defended_chunk_raw_shifted_exposes_metadata() {
    // 0123456789... head ends at 47 as before. chunk:
    // "A\r\n0123456789\r\n0\r\n\r\n"
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nA\r\n0123456789\r\n0\r\n\r\n";
    let end = recv.len() as u32;
    let mk = |raw_start: u32| SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response: ResponseSpans {
            code: sp(9, 12),
            reason: sp(13, 15),
            head_end: 47,
            headers: vec![hdr(17, 34, 36, 43)],
            body: Some(BodySpans {
                framing: Framing::Chunked,
                raw: sp(raw_start, end),
                content_len: 10,
                trailers: vec![],
                json: None,
            }),
        },
    };
    assert!(
        validate(GET_SENT, recv, &mk(47)).is_ok(),
        "honest chunked must validate"
    );
    // Shift raw.start to 50 (into the chunk data): must reject.
    assert!(
        validate(GET_SENT, recv, &mk(50)).is_err(),
        "FORGERY: raw shifted past size line"
    );
}

// ===========================================================================
// PROBE 10: request claimed close-delimited. Requests are never close. A POST
// with no CL/TE and trailing bytes + a Close body record must be rejected.
// ===========================================================================
#[test]
fn defended_request_close_framing() {
    let sent = b"POST /a HTTP/1.1\r\nHost: x\r\n\r\nbody";
    let request = RequestSpans {
        method: sp(0, 4),
        target: sp(5, 7),
        head_end: 28,
        headers: vec![hdr(18, 22, 24, 25)],
        body: Some(BodySpans {
            framing: Framing::Close,
            raw: sp(28, 32),
            content_len: 4,
            trailers: vec![],
            json: None,
        }),
    };
    let recv = b"HTTP/1.1 204 No Content\r\n\r\n";
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 23),
        head_end: 27,
        headers: vec![],
        body: None,
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request,
        response,
    };
    assert!(
        validate(sent, recv, &table).is_err(),
        "FORGERY: close-delimited request"
    );
}

// ===========================================================================
// PROBE 11: SANCTIONED FREEDOM G2 — valid-JSON bytes claimed opaque (json:None)
// must be ACCEPTED. This is the only intended degree of freedom; confirm it so
// the freedom boundary is understood (a PASS here is correct, not a forgery).
// ===========================================================================
#[test]
fn sanctioned_valid_json_claimed_opaque_accepts() {
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}";
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 15),
        head_end: 38,
        headers: vec![hdr(17, 31, 33, 34)],
        body: Some(BodySpans {
            framing: Framing::ContentLength,
            raw: sp(38, 45),
            content_len: 7,
            trailers: vec![],
            json: None, // opaque claim over valid JSON: allowed (G2)
        }),
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response,
    };
    assert!(
        validate(GET_SENT, recv, &table).is_ok(),
        "G2: opaque-over-JSON must be accepted"
    );
}

// ===========================================================================
// PROBE 12: HEAD request, response carries Transfer-Encoding: chunked AND real
// chunk bytes. Per D3, a HEAD response never has a body, so the chunk bytes are
// uncovered trailing data and the message must be rejected (cannot expose chunk
// metadata as a body).
// ===========================================================================
#[test]
fn defended_head_response_with_chunk_bytes() {
    let sent = b"HEAD /a HTTP/1.1\r\n\r\n";
    let request = RequestSpans {
        method: sp(0, 4),
        target: sp(5, 7),
        head_end: 20,
        headers: vec![],
        body: None,
    };
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    // Try to claim a chunked body anyway.
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 15),
        head_end: 47,
        headers: vec![hdr(17, 34, 36, 43)],
        body: Some(BodySpans {
            framing: Framing::Chunked,
            raw: sp(47, recv.len() as u32),
            content_len: 3,
            trailers: vec![],
            json: None,
        }),
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request,
        response,
    };
    assert!(
        validate(sent, recv, &table).is_err(),
        "FORGERY: HEAD response with chunk body"
    );
}

// ===========================================================================
// PROBE 13: 1xx (status 100) with a Content-Length body claim. Per D3, 1xx
// never has a body. Expect rejection of a body record / trailing bytes.
// ===========================================================================
#[test]
fn defended_1xx_with_body() {
    let recv = b"HTTP/1.1 100 Continue\r\nContent-Length: 2\r\n\r\nhi";
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 21),
        head_end: 45,
        headers: vec![hdr(23, 37, 39, 40)],
        body: Some(BodySpans {
            framing: Framing::ContentLength,
            raw: sp(45, 47),
            content_len: 2,
            trailers: vec![],
            json: None,
        }),
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response,
    };
    assert!(
        validate(GET_SENT, recv, &table).is_err(),
        "FORGERY: 1xx with body"
    );
}

// ===========================================================================
// PROBE 14: number "-0" round trips and a claim of "-0" with wrong end is
// rejected. Also confirm "-0" is a valid maximal lexeme. (sanity + defended)
// ===========================================================================
#[test]
fn defended_negative_zero_number() {
    let (recv, mut table) = cl_json(b"-0", vec![node(JsonKind::Number, 0, 2, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_ok(),
        "honest -0 must validate"
    );
    // claim end short (just "-")
    set_nodes(&mut table, vec![node(JsonKind::Number, 0, 1, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: -0 truncated to -"
    );
}

// ===========================================================================
// PROBE 15: object with reordered SIBLING members where key/value spans are
// internally consistent but pre-order is violated. Build {"a":1,"b":2} but
// list nodes in order key"b",val2,key"a",val1. Pre-order pins record order to
// document order; expect rejection.
// ===========================================================================
#[test]
fn defended_object_member_reorder() {
    let content = br#"{"a":1,"b":2}"#;
    // honest node order
    let honest = vec![
        node(JsonKind::Object, 0, 13, 5),
        node(JsonKind::Key, 2, 3, 1), // "a"
        node(JsonKind::Number, 5, 6, 1),
        node(JsonKind::Key, 8, 9, 1), // "b"
        node(JsonKind::Number, 11, 12, 1),
    ];
    let (recv, mut table) = cl_json(content, honest);
    assert!(
        validate(GET_SENT, &recv, &table).is_ok(),
        "honest object must validate"
    );
    // reordered: b's member before a's
    set_nodes(
        &mut table,
        vec![
            node(JsonKind::Object, 0, 13, 5),
            node(JsonKind::Key, 8, 9, 1), // "b" claimed first
            node(JsonKind::Number, 11, 12, 1),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 5, 6, 1),
        ],
    );
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: object members reordered"
    );
}

// ===========================================================================
// PROBE 16: chunked body decoding to JSON {"x": 12} with the number split
// across a chunk boundary, then a TAMPERED number node span. JSON validates
// over decoded coords; a wrong span must be rejected even though de-chunking
// succeeded. recv chunks: {"x"  ": 1  2}  -> decoded {"x": 12} (len 9).
// ===========================================================================
#[test]
fn defended_chunked_json_node_tamper() {
    // Build a chunked recv whose decode is {"x": 12}
    // chunk data pieces: 4,3,2 bytes
    let p1 = b"{\"x\""; // 4 bytes
    let p2 = b": 1"; // 3 bytes
    let p3 = b"2}"; // 2 bytes
    let mut recv = Vec::new();
    recv.extend_from_slice(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
    let head_end = recv.len() as u32;
    for p in [&p1[..], &p2[..], &p3[..]] {
        recv.extend_from_slice(format!("{:x}\r\n", p.len()).as_bytes());
        recv.extend_from_slice(p);
        recv.extend_from_slice(b"\r\n");
    }
    recv.extend_from_slice(b"0\r\n\r\n");
    let end = recv.len() as u32;
    // decoded {"x": 12}: Object[0,9], Key"x"[2,3], Number"12"[6,8]
    let honest = vec![
        node(JsonKind::Object, 0, 9, 3),
        node(JsonKind::Key, 2, 3, 1),
        node(JsonKind::Number, 6, 8, 1),
    ];
    let mk = |nodes: Vec<JsonNode>| SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response: ResponseSpans {
            code: sp(9, 12),
            reason: sp(13, 15),
            head_end,
            headers: vec![hdr(17, 34, 36, 43)],
            body: Some(BodySpans {
                framing: Framing::Chunked,
                raw: sp(head_end, end),
                content_len: 9,
                trailers: vec![],
                json: Some(JsonSpans { nodes }),
            }),
        },
    };
    assert!(
        validate(GET_SENT, &recv, &mk(honest)).is_ok(),
        "honest chunked JSON must validate"
    );
    // tamper: number claimed [6,7) (just "1")
    let bad = vec![
        node(JsonKind::Object, 0, 9, 3),
        node(JsonKind::Key, 2, 3, 1),
        node(JsonKind::Number, 6, 7, 1),
    ];
    assert!(
        validate(GET_SENT, &recv, &mk(bad)).is_err(),
        "FORGERY: chunked JSON number span tamper"
    );
}

// ===========================================================================
// PROBE 17: THREE keys all decoding to the same logical key via different
// encodings: raw "A", A, and... a third distinct encoding. Only two
// encodings of 'A' exist (raw, A), so use {"A":1,"A":2}. Confirm dup
// rejection (decoded comparison catches the alias).
// ===========================================================================
#[test]
fn defended_duplicate_uxxxx_ascii_alias() {
    // {"A":1,"A":2}
    let content: Vec<u8> = {
        let mut v = Vec::new();
        v.extend_from_slice(b"{\"A\":1,\"");
        v.extend_from_slice(b"\\u0041");
        v.extend_from_slice(b"\":2}");
        v
    };
    // { at0 "1 A[2,3] "3 :4 1@5 ,6 "7 key2[8,14] "@14 :15 2@16 }@17 -> len18
    assert_eq!(content.len(), 18);
    let nodes = vec![
        node(JsonKind::Object, 0, 18, 5),
        node(JsonKind::Key, 2, 3, 1),
        node(JsonKind::Number, 5, 6, 1),
        node(JsonKind::Key, 8, 14, 1),
        node(JsonKind::Number, 16, 17, 1),
    ];
    let (recv, table) = cl_json(&content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: A vs \\u0041 dup keys"
    );
}

// ===========================================================================
// PROBE 18: a string whose content includes a raw multi-byte char (é = C3 A9),
// node end claimed to split it (end at the C3). scan_string ends at the quote,
// so content end is forced; a split-claim must be rejected.
// content: " é " -> 0x22 0xC3 0xA9 0x22 (len 4). content [1,3).
// ===========================================================================
#[test]
fn defended_string_end_splits_multibyte() {
    let content: Vec<u8> = vec![0x22, 0xC3, 0xA9, 0x22];
    let (recv, mut table) = cl_json(&content, vec![node(JsonKind::String, 1, 3, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_ok(),
        "honest é string must validate"
    );
    // claim end [1,2) splitting é
    set_nodes(&mut table, vec![node(JsonKind::String, 1, 2, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: string end splits multibyte"
    );
}

// ===========================================================================
// PROBE 19: duplicate Host header end-to-end (rule B8). Two Host lines must be
// rejected via the dup flag even though each line is well-formed.
// ===========================================================================
#[test]
fn defended_duplicate_host_end_to_end() {
    let sent = b"GET /a HTTP/1.1\r\nHost: x\r\nHost: y\r\n\r\n";
    let request = RequestSpans {
        method: sp(0, 3),
        target: sp(4, 6),
        head_end: 37,
        headers: vec![hdr(17, 21, 23, 24), hdr(26, 30, 32, 33)],
        body: None,
    };
    let recv = b"HTTP/1.1 204 No Content\r\n\r\n";
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 23),
        head_end: 27,
        headers: vec![],
        body: None,
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request,
        response,
    };
    assert!(
        validate(sent, recv, &table).is_err(),
        "FORGERY: duplicate Host accepted"
    );
}

// ===========================================================================
// PROBE 20: Transfer-Encoding: chunked bytes but body.framing claims
// ContentLength. The derived framing is Chunked; tag mismatch must reject.
// ===========================================================================
#[test]
fn defended_chunked_bytes_claimed_contentlength() {
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 15),
        head_end: 47,
        headers: vec![hdr(17, 34, 36, 43)],
        body: Some(BodySpans {
            framing: Framing::ContentLength, // lie: bytes are chunked
            raw: sp(47, recv.len() as u32),
            content_len: 3,
            trailers: vec![],
            json: None,
        }),
    };
    let table = SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response,
    };
    assert!(
        validate(GET_SENT, recv, &table).is_err(),
        "FORGERY: chunked claimed CL"
    );
}

// ===========================================================================
// PROBE 21: duplicate keys via escaped-newline alias. key1 uses the simple
// escape backslash-n (decodes to 0x0A); key2 uses the unicode escape u000a
// (also 0x0A). decode_key must alias them so the duplicate is rejected.
// ===========================================================================
#[test]
fn defended_duplicate_newline_escape_keys() {
    // object bytes: { "\n" : 1 , "\u000a" : 2 }
    let content: Vec<u8> = {
        let mut v = Vec::new();
        v.extend_from_slice(b"{\"\\n\":1,\"");
        v.extend_from_slice(b"\\u000a");
        v.extend_from_slice(b"\":2}");
        v
    };
    // { @0 ; " @1 ; key1 content [2,4) = backslash-n ; " @4 ; : @5 ; 1 @6 ;
    // , @7 ; " @8 ; key2 content [9,15) = u000a ; " @15 ; : @16 ; 2 @17 ; } @18
    assert_eq!(content.len(), 19);
    let nodes = vec![
        node(JsonKind::Object, 0, 19, 5),
        node(JsonKind::Key, 2, 4, 1),
        node(JsonKind::Number, 6, 7, 1),
        node(JsonKind::Key, 9, 15, 1),
        node(JsonKind::Number, 17, 18, 1),
    ];
    let (recv, table) = cl_json(&content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: newline-escape dup keys"
    );
}

// ===========================================================================
// PROBE 22: vertical tab (0x0B) as a gap byte inside a container is NOT JSON
// whitespace; must reject. [1<VT>] where the table claims [1] (array size 2).
// ===========================================================================
#[test]
fn defended_vertical_tab_gap_rejected() {
    let content: Vec<u8> = vec![b'[', b'1', 0x0B, b']'];
    let (recv, table) = cl_json(
        &content,
        vec![
            node(JsonKind::Array, 0, 4, 2),
            node(JsonKind::Number, 1, 2, 1),
        ],
    );
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: VT accepted as gap"
    );
}

// ===========================================================================
// PROBE 23: root JSON with a hidden second value after whitespace. body
// "1 2" claimed as Number[0,1). Coverage (F1/F10) must reject the trailing 2.
// ===========================================================================
#[test]
fn defended_hidden_second_root_value() {
    let (recv, table) = cl_json(b"1 2", vec![node(JsonKind::Number, 0, 1, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: second root value hidden"
    );
}

// ===========================================================================
// PROBE 24: JSON root span must equal the JWS-trimmed extent. body "  {}  "
// with the object claimed but a hidden prefix char. Use " x{}" - the 'x' is a
// non-JWS prefix; root must start at first non-ws which is 'x' (invalid value).
// ===========================================================================
#[test]
fn defended_non_ws_prefix_rejected() {
    let (recv, table) = cl_json(b"x{}", vec![node(JsonKind::Object, 1, 3, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: non-ws prefix before root"
    );
}
