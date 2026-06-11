//! Roundtrip integration suite over the committed fixture corpus.
//!
//! For every `fixtures/` pair: host [`parse_transcript`] must accept, guest
//! [`validate`] must accept, and the zero-copy accessors must agree with
//! per-fixture expectations written from `fixtures/README.md`, with
//! independent cross-checks against direct byte slicing, `spansy`'s view
//! (where spansy can parse the message), and `serde_json`.
//!
//! spansy landmines honored here: `spansy::http::parse_response` PANICS on
//! reason-less status lines (`syn_no_reason`) and cannot parse
//! close-delimited responses, HEAD responses, or bodiless 200s without
//! CL/TE — spansy cross-checks run only on an explicit allowlist of
//! fixtures it is known to handle.

#![cfg(feature = "parse")]

use std::{fs, path::PathBuf};

use transcript_verify::{
    Body, Framing, Header, JsonKind, Span, SpanTable, parse_transcript, validate,
};

// === fixture loading (self-contained; no tests/common) ===

/// Returns the fixture corpus root.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Reads a fixture pair; `name` takes a `synthetic/` prefix for the
/// generated corpus.
fn load_pair(name: &str) -> (Vec<u8>, Vec<u8>) {
    let read = |suffix: &str| {
        let path = fixtures_dir().join(format!("{name}.{suffix}.bin"));
        fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    };
    (read("sent"), read("recv"))
}

/// Loads and host-parses a fixture pair, panicking with the fixture name on
/// failure.
fn parsed(name: &str) -> (Vec<u8>, Vec<u8>, SpanTable) {
    let (sent, recv) = load_pair(name);
    let table = parse_transcript(&sent, &recv)
        .unwrap_or_else(|e| panic!("{name}: parse_transcript failed: {e}"));
    (sent, recv, table)
}

/// Discovers every `<name>.sent.bin`/`<name>.recv.bin` pair in `fixtures/`
/// and `fixtures/synthetic/`, sorted by name.
fn discover_corpus() -> Vec<String> {
    let mut names = Vec::new();
    for (dir, prefix) in [
        (fixtures_dir(), ""),
        (fixtures_dir().join("synthetic"), "synthetic/"),
    ] {
        let entries =
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
        for entry in entries {
            let file = entry.expect("dir entry").file_name();
            let file = file.to_string_lossy();
            if let Some(stem) = file.strip_suffix(".sent.bin") {
                assert!(
                    dir.join(format!("{stem}.recv.bin")).is_file(),
                    "{prefix}{stem}: sent fixture has no matching recv fixture"
                );
                names.push(format!("{prefix}{stem}"));
            }
        }
    }
    names.sort();
    names
}

/// Concatenates the bytes covered by `spans` in `buf`, in order.
fn concat_spans(buf: &[u8], spans: &[Span]) -> Vec<u8> {
    spans
        .iter()
        .flat_map(|span| buf[span.as_range()].iter().copied())
        .collect()
}

/// Media-type check mirroring the host's JSON-claim policy: the first
/// `Content-Type`'s media type (parameters ignored) is `application/json`,
/// ASCII case-insensitively.
fn is_json_media_type(value: &[u8]) -> bool {
    value
        .split(|&b| b == b';')
        .next()
        .unwrap_or(b"")
        .trim_ascii()
        .eq_ignore_ascii_case(b"application/json")
}

// === per-fixture expectations (written from fixtures/README.md) ===

/// Expected body shape of one message side: `(framing, JSON claim expected,
/// decoded content length)`; `None` means the side must have no body.
type SideExpect = Option<(Framing, bool, u32)>;

const fn cl(json: bool, len: u32) -> SideExpect {
    Some((Framing::ContentLength, json, len))
}
const fn chunked(json: bool, len: u32) -> SideExpect {
    Some((Framing::Chunked, json, len))
}
const fn close(json: bool, len: u32) -> SideExpect {
    Some((Framing::Close, json, len))
}

/// One fixture's expected request line, status line, and body shapes.
struct Expect {
    name: &'static str,
    method: &'static str,
    target: &'static str,
    status: u16,
    reason: &'static str,
    request: SideExpect,
    response: SideExpect,
}

const fn row(
    name: &'static str,
    method: &'static str,
    target: &'static str,
    status: u16,
    reason: &'static str,
    request: SideExpect,
    response: SideExpect,
) -> Expect {
    Expect {
        name,
        method,
        target,
        status,
        reason,
        request,
        response,
    }
}

/// All 33 corpus pairs. The sweep fails on any fixture missing from this
/// table and on any row missing from disk, so corpus and expectations can
/// only drift loudly.
const EXPECTATIONS: &[Expect] = &[
    row("example_head", "HEAD", "/", 200, "OK", None, None),
    row(
        "example_html",
        "GET",
        "/",
        200,
        "OK",
        None,
        chunked(false, 559),
    ),
    row("github_head", "HEAD", "/zen", 200, "OK", None, None),
    row("github_zen", "GET", "/zen", 200, "OK", None, cl(false, 15)),
    row(
        "httpbingo_get",
        "GET",
        "/get",
        200,
        "OK",
        None,
        cl(true, 672),
    ),
    row(
        "httpbingo_png",
        "GET",
        "/image/png",
        200,
        "OK",
        None,
        chunked(false, 8090),
    ),
    row(
        "httpbingo_post_multipart",
        "POST",
        "/post",
        200,
        "OK",
        cl(false, 413),
        cl(true, 1513),
    ),
    row(
        "httpbingo_xml",
        "GET",
        "/xml",
        200,
        "OK",
        None,
        cl(false, 522),
    ),
    row("icanhazip", "GET", "/", 200, "OK", None, cl(false, 14)),
    row(
        "jsonplaceholder_post",
        "POST",
        "/posts",
        201,
        "Created",
        cl(true, 39),
        cl(true, 65),
    ),
    row(
        "pokeapi_ditto",
        "GET",
        "/api/v2/pokemon/ditto",
        200,
        "OK",
        None,
        chunked(true, 25564),
    ),
    row(
        "postman_204",
        "GET",
        "/status/204",
        204,
        "No Content",
        None,
        None,
    ),
    row(
        "postman_post_form",
        "POST",
        "/post",
        200,
        "OK",
        cl(false, 38),
        cl(true, 415),
    ),
    row(
        "postman_post_json",
        "POST",
        "/post",
        200,
        "OK",
        cl(true, 83),
        cl(true, 462),
    ),
    row(
        "swapi_films",
        "GET",
        "/api/films",
        200,
        "OK",
        None,
        chunked(true, 20524),
    ),
    row(
        "synthetic/syn_chunk_split_token",
        "GET",
        "/api/split",
        200,
        "OK",
        None,
        chunked(true, 37),
    ),
    row(
        "synthetic/syn_chunked_request",
        "POST",
        "/api/items",
        200,
        "OK",
        chunked(true, 13),
        cl(true, 23),
    ),
    row(
        "synthetic/syn_cl_zero",
        "POST",
        "/api/ping",
        200,
        "OK",
        // `Content-Length: 0` yields no body record at all.
        None,
        cl(true, 13),
    ),
    row(
        "synthetic/syn_close_delim",
        "GET",
        "/api/status",
        200,
        "OK",
        None,
        close(true, 11),
    ),
    row(
        "synthetic/syn_deep_127",
        "GET",
        "/api/deep/127",
        200,
        "OK",
        None,
        cl(true, 255),
    ),
    row(
        "synthetic/syn_deep_128",
        "GET",
        "/api/deep/128",
        200,
        "OK",
        None,
        cl(true, 257),
    ),
    row(
        "synthetic/syn_dup_set_cookie",
        "GET",
        "/login",
        200,
        "OK",
        None,
        cl(true, 14),
    ),
    row(
        "synthetic/syn_empty_containers",
        "GET",
        "/api/empty",
        200,
        "OK",
        None,
        cl(true, 45),
    ),
    row(
        "synthetic/syn_empty_header_value",
        "GET",
        "/headers/empty-value",
        200,
        "OK",
        None,
        cl(true, 14),
    ),
    row(
        "synthetic/syn_empty_reason",
        "GET",
        "/reason/empty",
        200,
        // `HTTP/1.1 200 ` — SP present, empty reason.
        "",
        None,
        cl(false, 38),
    ),
    row(
        "synthetic/syn_no_reason",
        "GET",
        "/reason/none",
        200,
        // `HTTP/1.1 200` — no SP, no reason.
        "",
        None,
        cl(false, 39),
    ),
    row(
        "synthetic/syn_obs_text",
        "GET",
        "/headers/obs-text",
        200,
        "OK",
        None,
        cl(false, 49),
    ),
    row(
        "synthetic/syn_root_bool",
        "GET",
        "/api/root/bool",
        200,
        "OK",
        None,
        cl(true, 4),
    ),
    row(
        "synthetic/syn_root_null",
        "GET",
        "/api/root/null",
        200,
        "OK",
        None,
        cl(true, 4),
    ),
    row(
        "synthetic/syn_root_number",
        "GET",
        "/api/root/number",
        200,
        "OK",
        None,
        cl(true, 2),
    ),
    row(
        "synthetic/syn_root_string",
        "GET",
        "/api/root/string",
        200,
        "OK",
        None,
        cl(true, 7),
    ),
    row(
        "synthetic/syn_trailers",
        "GET",
        "/api/data",
        200,
        "OK",
        None,
        chunked(true, 24),
    ),
    row(
        "synthetic/syn_unicode",
        "GET",
        "/api/unicode",
        200,
        "OK",
        None,
        cl(true, 52),
    ),
];

/// Checks one message side of a validated transcript against its
/// [`SideExpect`] row plus the dynamic JSON-claim invariant.
fn check_side(
    name: &str,
    side: &str,
    buf: &[u8],
    content_type: Option<Header<'_>>,
    body: Option<Body<'_>>,
    exp: SideExpect,
) {
    let Some((framing, json_expected, len)) = exp else {
        assert!(body.is_none(), "{name}: unexpected {side} body");
        return;
    };
    let body = body.unwrap_or_else(|| panic!("{name}: missing {side} body"));
    assert_eq!(body.framing(), framing, "{name}: {side} framing");
    let content = body.content();
    assert_eq!(content.len(), len as usize, "{name}: {side} decoded length");
    // Non-chunked bodies are the final `len` bytes of the buffer: recompute
    // the expected content by direct slicing, independent of the table.
    if framing != Framing::Chunked {
        assert_eq!(
            content,
            &buf[buf.len() - len as usize..],
            "{name}: {side} content differs from the direct buffer slice"
        );
    }
    // Static expectation: JSON claim present iff the README says the body
    // is application/json (the host must not have fallen back to opaque).
    assert_eq!(
        body.json().is_some(),
        json_expected,
        "{name}: {side} JSON claim presence"
    );
    // Dynamic invariant, independent of the static table: a body under an
    // `application/json` content type must carry a verified JSON claim — a
    // bare `is_none` here means the host silently fell back to opaque on
    // valid corpus JSON (a FINDING, not a tolerance); any other media type
    // must stay opaque under the host policy.
    let ct_is_json = content_type.is_some_and(|h| is_json_media_type(h.value()));
    if ct_is_json {
        assert!(
            body.json().is_some(),
            "{name}: {side} body is application/json but the host claimed it opaque"
        );
    } else {
        assert!(
            body.json().is_none(),
            "{name}: {side} body is not application/json but carries a JSON claim"
        );
    }
}

// === 1. corpus sweep ===

#[test]
fn corpus_sweep_roundtrips_every_pair() {
    let names = discover_corpus();
    assert!(
        names.len() >= 33,
        "silent corpus loss: only {} fixture pairs found (expected >= 33): {names:?}",
        names.len()
    );
    for exp in EXPECTATIONS {
        assert!(
            names.iter().any(|n| n == exp.name),
            "{}: expectation row has no fixture pair on disk",
            exp.name
        );
    }
    for name in &names {
        let exp = EXPECTATIONS
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name}: fixture pair has no EXPECTATIONS row; add one"));
        let (sent, recv, table) = parsed(name);
        let t = validate(&sent, &recv, &table)
            .unwrap_or_else(|e| panic!("{name}: validate failed: {e}"));
        assert_eq!(t.request().method(), exp.method, "{name}: method");
        assert_eq!(t.request().target(), exp.target, "{name}: target");
        assert_eq!(t.response().status(), exp.status, "{name}: status");
        assert_eq!(t.response().reason(), exp.reason, "{name}: reason");
        check_side(
            name,
            "request",
            &sent,
            t.request().header("Content-Type"),
            t.request().body(),
            exp.request,
        );
        check_side(
            name,
            "response",
            &recv,
            t.response().header("Content-Type"),
            t.response().body(),
            exp.response,
        );
    }
}

// === 3. determinism ===

#[test]
fn parse_transcript_is_deterministic() {
    let (sent, recv) = load_pair("pokeapi_ditto");
    let first = parse_transcript(&sent, &recv).expect("first parse");
    let second = parse_transcript(&sent, &recv).expect("second parse");
    assert_eq!(
        first, second,
        "pokeapi_ditto: span tables differ across runs"
    );
}

// === 2. header cross-checks (incl. duplicate Set-Cookie) ===

/// Asserts our header view and spansy's agree pairwise (count, order, name,
/// value bytes) for one message side.
fn assert_headers_match_spansy<'a, S: spansy::Store>(
    name: &str,
    side: &str,
    ours: impl Iterator<Item = Header<'a>>,
    theirs: &[spansy::http::Header<S>],
) {
    let ours: Vec<_> = ours.collect();
    assert_eq!(ours.len(), theirs.len(), "{name}: {side} header count");
    for (i, (our, sp)) in ours.iter().zip(theirs).enumerate() {
        assert_eq!(
            our.name(),
            &*sp.name.as_str(),
            "{name}: {side} header {i} name"
        );
        assert_eq!(
            our.value(),
            &*sp.value.as_bytes(),
            "{name}: {side} header {i} value"
        );
    }
}

#[test]
fn headers_postman_post_json_three_set_cookies_vs_spansy() {
    let (sent, recv, table) = parsed("postman_post_json");
    let t = validate(&sent, &recv, &table).expect("validate");
    let resp = t.response();

    // Three real Set-Cookie response headers, mixed name case on the wire,
    // in order of appearance.
    let cookies: Vec<_> = resp.headers_with_name("Set-Cookie").collect();
    assert_eq!(cookies.len(), 3, "three Set-Cookie headers");
    assert!(cookies[0].value().starts_with(b"sails.sid=s%3A"));
    assert!(cookies[1].value().starts_with(b"__cf_bm="));
    assert!(cookies[2].value().starts_with(b"_cfuvid="));
    // First-match-wins lookup returns the first occurrence's bytes.
    assert_eq!(
        resp.header("SET-COOKIE").expect("lookup").value(),
        cookies[0].value()
    );

    // spansy sees the identical headers on both sides.
    let sp_resp = spansy::http::parse_response(&recv[..]).expect("spansy response");
    assert_headers_match_spansy(
        "postman_post_json",
        "response",
        resp.headers(),
        &sp_resp.headers,
    );
    let sp_req = spansy::http::parse_request(&sent[..]).expect("spansy request");
    assert_headers_match_spansy(
        "postman_post_json",
        "request",
        t.request().headers(),
        &sp_req.headers,
    );
    // And the same duplicate-name view.
    let sp_cookies: Vec<_> = sp_resp.headers_with_name("set-cookie").collect();
    assert_eq!(sp_cookies.len(), 3);
    for (our, sp) in cookies.iter().zip(&sp_cookies) {
        assert_eq!(our.value(), &*sp.value.as_bytes());
    }
}

#[test]
fn headers_syn_dup_set_cookie_first_match_and_order() {
    let (sent, recv, table) = parsed("synthetic/syn_dup_set_cookie");
    let t = validate(&sent, &recv, &table).expect("validate");
    let resp = t.response();

    let values: Vec<_> = resp
        .headers_with_name("set-cookie")
        .map(|h| h.value())
        .collect();
    assert_eq!(
        values,
        [
            b"session=abc123; Path=/; HttpOnly".as_slice(),
            b"theme=dark; Max-Age=3600".as_slice(),
        ],
        "duplicate Set-Cookie values, in order of appearance"
    );
    assert_eq!(
        resp.header("Set-Cookie").expect("first match").value(),
        b"session=abc123; Path=/; HttpOnly"
    );

    let sp = spansy::http::parse_response(&recv[..]).expect("spansy response");
    assert_headers_match_spansy(
        "syn_dup_set_cookie",
        "response",
        resp.headers(),
        &sp.headers,
    );
}

#[test]
fn headers_httpbingo_get_vs_spansy_both_sides() {
    let (sent, recv, table) = parsed("httpbingo_get");
    let t = validate(&sent, &recv, &table).expect("validate");

    // Lowercase header names in the wild; lookup is case-insensitive but
    // the returned name preserves the wire bytes.
    let ct = t.response().header("Content-Type").expect("content-type");
    assert_eq!(ct.name(), "content-type");
    assert_eq!(ct.value(), b"application/json; charset=utf-8");

    let sp_req = spansy::http::parse_request(&sent[..]).expect("spansy request");
    assert_eq!(t.request().method(), &*sp_req.request.method.as_str());
    assert_eq!(t.request().target(), &*sp_req.request.target.as_str());
    assert_headers_match_spansy(
        "httpbingo_get",
        "request",
        t.request().headers(),
        &sp_req.headers,
    );

    let sp_resp = spansy::http::parse_response(&recv[..]).expect("spansy response");
    assert_eq!(
        t.response().status().to_string(),
        &*sp_resp.status.code.as_str()
    );
    assert_eq!(t.response().reason(), &*sp_resp.status.reason.as_str());
    assert_headers_match_spansy(
        "httpbingo_get",
        "response",
        t.response().headers(),
        &sp_resp.headers,
    );
}

#[test]
fn headers_syn_empty_values_ours_only() {
    // Empty header values, with and without OWS (spansy is deliberately not
    // consulted here; the empty-value pinning is this crate's own rule).
    let (sent, recv, table) = parsed("synthetic/syn_empty_header_value");
    let t = validate(&sent, &recv, &table).expect("validate");
    assert_eq!(
        t.response().header("X-Empty").expect("X-Empty").value(),
        b""
    );
    assert_eq!(
        t.response()
            .header("x-empty-ows")
            .expect("X-Empty-Ows")
            .value(),
        b""
    );
    assert_eq!(
        t.response().body().expect("body").content(),
        b"{\"empty\":true}"
    );
}

// === 2. body content cross-checks ===

#[test]
fn chunked_bodies_match_spansy_decode() {
    // Chunked fixtures spansy is known to parse; its `content_data()`
    // concatenates the chunk payloads independently of our de-chunker.
    for name in [
        "pokeapi_ditto",
        "swapi_films",
        "example_html",
        "httpbingo_png",
    ] {
        let (sent, recv, table) = parsed(name);
        let t = validate(&sent, &recv, &table).unwrap_or_else(|e| panic!("{name}: {e}"));
        let body = t
            .response()
            .body()
            .unwrap_or_else(|| panic!("{name}: body"));
        assert_eq!(body.framing(), Framing::Chunked, "{name}");
        let sp = spansy::http::parse_response(&recv[..])
            .unwrap_or_else(|e| panic!("{name}: spansy: {e}"));
        let sp_body = sp.body.unwrap_or_else(|| panic!("{name}: spansy body"));
        assert_eq!(
            body.content(),
            &*sp_body.content_data(),
            "{name}: decoded chunked content differs from spansy's"
        );
    }
}

#[test]
fn chunked_bodies_known_bytes() {
    // syn_trailers: two chunks reassemble into one JSON document.
    let (sent, recv, table) = parsed("synthetic/syn_trailers");
    let t = validate(&sent, &recv, &table).expect("validate");
    assert_eq!(
        t.response().body().expect("body").content(),
        b"{\"data\":\"chunked\",\"n\":7}"
    );

    // syn_chunk_split_token: chunk boundaries inside a number lexeme and a
    // string lexeme still decode to the document.
    let (sent, recv, table) = parsed("synthetic/syn_chunk_split_token");
    let t = validate(&sent, &recv, &table).expect("validate");
    assert_eq!(
        t.response().body().expect("body").content(),
        b"{\"price\":12345.678,\"name\":\"alphabet\"}"
    );

    // httpbingo_png: binary body; PNG signature prefix and IEND CRC tail.
    let (sent, recv, table) = parsed("httpbingo_png");
    let t = validate(&sent, &recv, &table).expect("validate");
    let content = t.response().body().expect("body").content();
    assert_eq!(&content[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(content.last(), Some(&0x82));
}

#[test]
fn content_length_and_close_bodies_known_bytes() {
    let (sent, recv, table) = parsed("github_zen");
    let t = validate(&sent, &recv, &table).expect("validate");
    assert_eq!(
        t.response().body().expect("body").content(),
        b"Encourage flow."
    );

    let (sent, recv, table) = parsed("icanhazip");
    let t = validate(&sent, &recv, &table).expect("validate");
    assert_eq!(
        t.response().body().expect("body").content(),
        b"205.147.28.36\n"
    );

    // Close-delimited framing (no CL, no TE): body runs to end of buffer
    // and the JSON claim still applies.
    let (sent, recv, table) = parsed("synthetic/syn_close_delim");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    assert_eq!(body.framing(), Framing::Close);
    assert_eq!(body.content(), b"{\"ok\":true}");
    assert_eq!(
        body.json()
            .expect("json")
            .get("ok")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

// === 2. JSON accessors vs serde_json ===

#[test]
fn json_pokeapi_vs_serde() {
    let (sent, recv, table) = parsed("pokeapi_ditto");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    let json = body.json().expect("JSON claim");
    let sd: serde_json::Value = serde_json::from_slice(body.content()).expect("serde parse");

    let name = json.get("name").expect("name").as_str().expect("string");
    assert_eq!(name, "ditto");
    assert_eq!(Some(name), sd["name"].as_str());

    let ability = json
        .get("abilities.0.ability.name")
        .expect("abilities.0.ability.name")
        .as_str()
        .expect("string");
    assert_eq!(ability, "limber");
    assert_eq!(
        Some(ability),
        sd.pointer("/abilities/0/ability/name")
            .and_then(|v| v.as_str())
    );

    // Container sizes agree too.
    let abilities = json.get("abilities").expect("abilities");
    assert_eq!(
        abilities.array_len().map(|n| n as usize),
        sd["abilities"].as_array().map(Vec::len)
    );
}

#[test]
fn json_swapi_array_root_vs_serde() {
    let (sent, recv, table) = parsed("swapi_films");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    let json = body.json().expect("JSON claim");
    let sd: serde_json::Value = serde_json::from_slice(body.content()).expect("serde parse");

    assert_eq!(json.kind(), JsonKind::Array, "array root");
    assert_eq!(
        json.array_len().map(|n| n as usize),
        sd.as_array().map(Vec::len)
    );
    let title = json
        .get("0.title")
        .expect("0.title")
        .as_str()
        .expect("string");
    assert_eq!(title, "A New Hope");
    assert_eq!(Some(title), sd.pointer("/0/title").and_then(|v| v.as_str()));
}

#[test]
fn json_httpbingo_get_header_echo_vs_serde() {
    let (sent, recv, table) = parsed("httpbingo_get");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    let json = body.json().expect("JSON claim");
    let sd: serde_json::Value = serde_json::from_slice(body.content()).expect("serde parse");

    // The response echoes the request headers we sent.
    let ua = json
        .get("headers.User-Agent.0")
        .expect("headers.User-Agent.0")
        .as_str()
        .expect("string");
    assert_eq!(ua, "transcript-verify-fixtures/0.1");
    assert_eq!(
        Some(ua),
        sd.pointer("/headers/User-Agent/0").and_then(|v| v.as_str())
    );
    assert_eq!(
        json.get("url").expect("url").as_str(),
        Some("https://httpbingo.org/get")
    );
}

#[test]
fn json_jsonplaceholder_post_both_sides_vs_serde() {
    let (sent, recv, table) = parsed("jsonplaceholder_post");
    let t = validate(&sent, &recv, &table).expect("validate");

    // Request body: the JSON we posted.
    let req_body = t.request().body().expect("request body");
    let req_json = req_body.json().expect("request JSON claim");
    let req_sd: serde_json::Value =
        serde_json::from_slice(req_body.content()).expect("serde parse");
    assert_eq!(req_json.get("title").expect("title").as_str(), Some("foo"));
    assert_eq!(req_sd["title"].as_str(), Some("foo"));

    // Response body: 201 Created echo with the assigned id.
    let resp_body = t.response().body().expect("response body");
    let resp_json = resp_body.json().expect("response JSON claim");
    let resp_sd: serde_json::Value =
        serde_json::from_slice(resp_body.content()).expect("serde parse");
    let id = resp_json
        .get("id")
        .expect("id")
        .as_number_str()
        .expect("number")
        .parse::<u64>()
        .expect("u64 lexeme");
    assert_eq!(Some(id), resp_sd["id"].as_u64());
    assert_eq!(id, 101);
}

#[test]
fn json_syn_unicode_lone_surrogate_ours_only() {
    let (sent, recv, table) = parsed("synthetic/syn_unicode");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    let json = body.json().expect("JSON claim");

    // Escapes are exposed raw (not decoded): the `\uXXXX` sequences stay as
    // literal backslash-u text next to real multi-byte UTF-8 characters.
    assert_eq!(
        json.get("u").expect("u").as_str(),
        Some("A\u{e9}\u{4e2d}\u{1f600} via \\uD83D\\uDE00")
    );
    // The lone surrogate `\uD800` is accepted by RFC 8259's grammar and by
    // this crate, but serde_json refuses to decode it — so the lone
    // surrogate is asserted through OUR accessors only.
    assert_eq!(json.get("lone").expect("lone").as_str(), Some("\\uD800"));

    // serde_json rejects the whole document over the lone surrogate
    // (documented divergence). Keep the cross-check soft: if a future
    // serde_json accepts it, the object shape must still agree.
    match serde_json::from_slice::<serde_json::Value>(body.content()) {
        // Expected today: "unexpected end of hex escape" — serde_json
        // demands a low surrogate after `\uD800`.
        Err(_) => {}
        Ok(sd) => assert_eq!(sd.as_object().map(serde_json::Map::len), Some(2)),
    }
}

#[test]
fn json_synthetic_roots_and_empty_containers_vs_serde() {
    // Non-container roots.
    type RootCheck = fn(transcript_verify::JsonValue<'_>);
    let cases: [(&str, RootCheck); 4] = [
        ("synthetic/syn_root_number", |v| {
            assert_eq!(v.as_number_str(), Some("42"));
        }),
        ("synthetic/syn_root_string", |v| {
            assert_eq!(v.as_str(), Some("hello"));
        }),
        ("synthetic/syn_root_bool", |v| {
            assert_eq!(v.as_bool(), Some(true));
        }),
        ("synthetic/syn_root_null", |v| assert!(v.is_null())),
    ];
    for (name, check) in cases {
        let (sent, recv, table) = parsed(name);
        let t = validate(&sent, &recv, &table).unwrap_or_else(|e| panic!("{name}: {e}"));
        let body = t
            .response()
            .body()
            .unwrap_or_else(|| panic!("{name}: body"));
        check(body.json().unwrap_or_else(|| panic!("{name}: JSON claim")));
        // serde_json accepts all four root documents.
        serde_json::from_slice::<serde_json::Value>(body.content())
            .unwrap_or_else(|e| panic!("{name}: serde: {e}"));
    }

    // Empty containers, nested.
    let (sent, recv, table) = parsed("synthetic/syn_empty_containers");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    let json = body.json().expect("JSON claim");
    let sd: serde_json::Value = serde_json::from_slice(body.content()).expect("serde parse");
    assert_eq!(
        json.get("obj")
            .expect("obj")
            .iter_object()
            .expect("object")
            .count(),
        0
    );
    assert_eq!(json.get("arr").expect("arr").array_len(), Some(0));
    assert_eq!(json.get("str").expect("str").as_str(), Some(""));
    assert_eq!(
        json.get("nested.0").expect("nested.0").kind(),
        JsonKind::Object
    );
    assert_eq!(json.get("nested.1").expect("nested.1").array_len(), Some(0));
    assert_eq!(sd["nested"].as_array().map(Vec::len), Some(2));
}

#[test]
fn json_chunked_request_side_vs_serde() {
    // syn_chunked_request: the REQUEST body is chunked with the split
    // inside a string token; the JSON claim applies to the decoded bytes.
    let (sent, recv, table) = parsed("synthetic/syn_chunked_request");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.request().body().expect("request body");
    assert_eq!(body.framing(), Framing::Chunked);
    assert_eq!(body.content(), b"{\"a\":[1,2,3]}");
    let json = body.json().expect("request JSON claim");
    let sd: serde_json::Value = serde_json::from_slice(body.content()).expect("serde parse");
    assert_eq!(json.get("a").expect("a").array_len(), Some(3));
    assert_eq!(sd["a"].as_array().map(Vec::len), Some(3));
    assert_eq!(json.get("a.2").expect("a.2").as_number_str(), Some("3"));

    // Response side: plain CL JSON.
    let resp_json = t
        .response()
        .body()
        .expect("response body")
        .json()
        .expect("JSON");
    assert_eq!(resp_json.get("id").expect("id").as_number_str(), Some("7"));
}

// === 2. HEAD / 204 / trailers ===

#[test]
fn head_and_204_responses_have_no_body() {
    for name in ["example_head", "github_head", "postman_204"] {
        let (sent, recv, table) = parsed(name);
        let t = validate(&sent, &recv, &table).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(t.response().body().is_none(), "{name}: must have no body");
    }
    // github_head: the Content-Length header survives even though the HEAD
    // rule yields zero body bytes.
    let (sent, recv, table) = parsed("github_head");
    let t = validate(&sent, &recv, &table).expect("validate");
    assert_eq!(t.request().method(), "HEAD");
    assert_eq!(
        t.response()
            .header("Content-Length")
            .expect("CL header")
            .value(),
        b"15"
    );
}

#[test]
fn syn_trailers_trailer_namespace() {
    let (sent, recv, table) = parsed("synthetic/syn_trailers");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");

    assert_eq!(body.trailers().count(), 1);
    let trailer = body
        .trailer("x-checksum")
        .expect("case-insensitive trailer lookup");
    assert_eq!(trailer.name(), "X-Checksum");
    assert_eq!(trailer.value(), b"abc123");

    // Trailers are a separate namespace: never visible as head headers.
    assert!(t.response().header("X-Checksum").is_none());
    assert_eq!(t.response().headers_with_name("X-Checksum").count(), 0);
}

// === 2. content_to_source round-trips ===

#[test]
fn content_to_source_roundtrip_split_token() {
    // syn_chunk_split_token chunks: `{"price":123` | `45.678,"name":"alph`
    // | `abet"}` — both checked lexemes straddle a chunk boundary.
    let (sent, recv, table) = parsed("synthetic/syn_chunk_split_token");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    let json = body.json().expect("JSON claim");

    let price = json.get("price").expect("price");
    let span = price.span();
    let spans = body.content_to_source(span.start..span.end);
    assert_eq!(spans.len(), 2, "number lexeme straddles one chunk boundary");
    assert_eq!(concat_spans(&recv, &spans), b"12345.678");
    assert_eq!(
        concat_spans(&recv, &spans),
        &body.content()[span.as_range()]
    );

    let name = json.get("name").expect("name");
    let span = name.span();
    let spans = body.content_to_source(span.start..span.end);
    assert_eq!(
        spans.len(),
        2,
        "string content straddles one chunk boundary"
    );
    assert_eq!(concat_spans(&recv, &spans), b"alphabet");
    assert_eq!(
        concat_spans(&recv, &spans),
        &body.content()[span.as_range()]
    );

    // The full decoded range maps to all three chunk data runs.
    let full = body.content_to_source(0..body.content().len() as u32);
    assert_eq!(full.len(), 3);
    assert_eq!(concat_spans(&recv, &full), body.content());
}

#[test]
fn content_to_source_roundtrip_png() {
    // httpbingo_png: 2 chunks (2530 + 5560 bytes) of binary data.
    let (sent, recv, table) = parsed("httpbingo_png");
    let t = validate(&sent, &recv, &table).expect("validate");
    let body = t.response().body().expect("body");
    let content = body.content();

    let full = body.content_to_source(0..content.len() as u32);
    assert_eq!(full.len(), 2, "one source span per chunk");
    assert_eq!(concat_spans(&recv, &full), content);

    // A range inside the first chunk maps to one contiguous wire span.
    let magic = body.content_to_source(0..8);
    assert_eq!(magic.len(), 1);
    assert_eq!(&recv[magic[0].as_range()], b"\x89PNG\r\n\x1a\n");

    // A range straddling the chunk boundary (2530) maps to two wire spans
    // whose concatenation equals the decoded slice.
    let straddle = body.content_to_source(2526..2534);
    assert_eq!(straddle.len(), 2);
    assert_eq!(concat_spans(&recv, &straddle), &content[2526..2534]);
}
