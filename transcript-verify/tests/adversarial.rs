//! Adversarial integration suite for `transcript-verify`.
//!
//! The soundness property under test: for fixed `sent`/`recv` bytes,
//! [`validate`] accepts AT MOST the honest [`SpanTable`] (the one
//! [`parse_transcript`] emits), and **never panics** on a hostile table.
//!
//! Part 1 is a generic single-field mutator run over every committed fixture
//! pair: the honest table must validate `Ok`, and every single-field mutation
//! must be rejected with `Err` (or, per rule G1 below, re-accepted only when it
//! is the one consumer-visible degree of freedom).
//!
//! Part 2 is a battery of named, hand-crafted targeted attacks (forged tables
//! and/or hostile wire bytes), each commented with the rule it probes.
//!
//! Rule G1 (the ONLY accepted mutation): dropping a JSON claim
//! (`body.json: Some(..) -> None`) downgrades the body to opaque and must still
//! validate `Ok`; conjuring a JSON claim (`None -> Some(..)`) must `Err`.
//!
//! Any mutation that validates `Ok` (other than G1) or panics is a SOUNDNESS
//! FINDING; such a case is isolated into an `#[ignore]`d `finding_*` test with
//! a full repro rather than left to break the suite.

#![cfg(feature = "parse")]

use std::{
    fs,
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
};

use transcript_verify::{
    BodySpans, FORMAT_VERSION, Framing, HeaderSpan, JsonKind, JsonNode, JsonSpans, RequestSpans,
    ResponseSpans, Span, SpanTable, parse_transcript, validate,
};

// === fixture loading (self-contained; mirrors tests/roundtrip.rs) ===

/// Returns the fixture corpus root.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Reads a fixture pair; `name` takes a `synthetic/` prefix for the generated
/// corpus.
fn load_pair(name: &str) -> (Vec<u8>, Vec<u8>) {
    let read = |suffix: &str| {
        let path = fixtures_dir().join(format!("{name}.{suffix}.bin"));
        fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    };
    (read("sent"), read("recv"))
}

/// Discovers every `<name>.sent.bin`/`<name>.recv.bin` pair in `fixtures/` and
/// `fixtures/synthetic/`, sorted by name.
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
                names.push(format!("{prefix}{stem}"));
            }
        }
    }
    names.sort();
    names
}

/// Loads and host-parses a fixture pair, panicking with the fixture name on
/// failure.
fn parsed(name: &str) -> (Vec<u8>, Vec<u8>, SpanTable) {
    let (sent, recv) = load_pair(name);
    let table = parse_transcript(&sent, &recv)
        .unwrap_or_else(|e| panic!("{name}: parse_transcript failed: {e}"));
    (sent, recv, table)
}

// === mutation helpers ===

/// Returns a clone of `t` with `f` applied — the single-field mutation helper.
fn mutated(t: &SpanTable, f: impl Fn(&mut SpanTable)) -> SpanTable {
    let mut c = t.clone();
    f(&mut c);
    c
}

/// Calls [`validate`] under `catch_unwind`, mapping the outcome to one of three
/// states so the generic mutator can distinguish a clean rejection
/// (`Ok(false)`) from an acceptance (`Ok(true)`, a soundness hole) and from a
/// panic (`Err(_)`, a robustness hole). The default panic hook is silenced for
/// the duration so the (expected-zero) panics do not flood test output.
fn validate_outcome(sent: &[u8], recv: &[u8], table: &SpanTable) -> Result<bool, ()> {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let res = panic::catch_unwind(AssertUnwindSafe(|| validate(sent, recv, table).is_ok()));
    panic::set_hook(prev);
    res.map_err(|_| ())
}

// === PART 1: generic single-field mutator ===
//
// Every mutation below is a single-field perturbation of the honest table. The
// validator re-derives each span/tag/count from the bytes and equality-checks
// it, so any perturbation that moves the table off the canonical parse MUST be
// rejected (`Ok(false)`); a panic (`Err(())`) is a robustness break and an
// acceptance (`Ok(true)`) a soundness break. The one sanctioned acceptance is
// rule G1 (drop a JSON claim), tested separately by `part1_g1_drop_json_claim`.

/// Collected mutations for one table: each is a (label, candidate) pair. A
/// candidate equal to the original is dropped by `push` (PartialEq), so the
/// caller never needs to special-case no-op perturbations.
struct Mutations<'a> {
    base: &'a SpanTable,
    items: Vec<(String, SpanTable)>,
}

impl<'a> Mutations<'a> {
    fn new(base: &'a SpanTable) -> Self {
        Self {
            base,
            items: Vec::new(),
        }
    }

    /// Builds a candidate by cloning the base and applying `f`; keeps it only
    /// if it actually differs from the base.
    fn push(&mut self, label: impl Into<String>, f: impl Fn(&mut SpanTable)) {
        let cand = mutated(self.base, f);
        if &cand != self.base {
            self.items.push((label.into(), cand));
        }
    }

    /// Pushes `value+1` and (when non-zero) `value-1` for the `u32` selected by
    /// `get`/`set`. At 0 the `-1` arm is skipped (it cannot decrement without
    /// underflow and would alias `+1`'s neighbourhood for no extra coverage);
    /// `+1` is always exercised.
    fn push_pm1(
        &mut self,
        label: &str,
        get: impl Fn(&SpanTable) -> u32,
        set: impl Fn(&mut SpanTable, u32) + Copy,
    ) {
        let v = get(self.base);
        self.push(format!("{label}+1"), move |t| set(t, v.wrapping_add(1)));
        if v != 0 {
            self.push(format!("{label}-1"), move |t| set(t, v - 1));
        }
    }
}

/// Indices of the JSON nodes to perturb. For large trees (`> 200` nodes) we
/// sample {0, 1, 2, every 37th, last} to keep the sweep linear-ish and well
/// under the runtime budget while still touching the root, the first few
/// members, a stride across the body, and the final leaf; small trees are
/// covered exhaustively.
fn sampled_node_indices(n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    if n <= 200 {
        return (0..n).collect();
    }
    let mut idx = vec![0usize, 1, 2];
    let mut i = 0;
    while i < n {
        idx.push(i);
        i += 37;
    }
    idx.push(n - 1);
    idx.sort_unstable();
    idx.dedup();
    idx
}

/// The three `Framing` variants, for cycling a body's framing tag.
const FRAMINGS: [Framing; 3] = [Framing::ContentLength, Framing::Chunked, Framing::Close];

/// The other two `JsonKind`s to swap a node to, given its current kind. Cycles
/// through the 7-variant enum so every sampled node is retagged two ways.
fn other_kinds(k: JsonKind) -> [JsonKind; 2] {
    use JsonKind::*;
    const ALL: [JsonKind; 7] = [Null, Bool, Number, String, Key, Object, Array];
    let i = ALL.iter().position(|&x| x == k).unwrap();
    [ALL[(i + 1) % 7], ALL[(i + 2) % 7]]
}

/// A minimal `BodySpans` for the None->Some structural mutation (deliberately
/// implausible: a zero-length Content-Length body claimed at offset 0). Used
/// only to prove the validator rejects conjuring a body where the derived
/// framing yields none.
fn minimal_body() -> BodySpans {
    BodySpans {
        framing: Framing::ContentLength,
        raw: Span::new(0, 0),
        content_len: 0,
        trailers: Vec::new(),
        json: None,
    }
}

/// Borrows the body record for the chosen side, if present.
fn side_body(t: &SpanTable, response: bool) -> Option<&BodySpans> {
    if response {
        t.response.body.as_ref()
    } else {
        t.request.body.as_ref()
    }
}

/// Mutably borrows the header vector for the chosen side.
fn side_headers_mut(t: &mut SpanTable, response: bool) -> &mut Vec<HeaderSpan> {
    if response {
        &mut t.response.headers
    } else {
        &mut t.request.headers
    }
}

/// Enumerates every single-field mutation for one side's head + body and pushes
/// it into `m`. `response` selects which side; `field` makes labels unique.
fn collect_side_mutations(m: &mut Mutations, response: bool) {
    let base = m.base;
    let side = if response { "resp" } else { "req" };

    // head_end +-1
    m.push_pm1(
        &format!("{side}.head_end"),
        move |t| {
            if response {
                t.response.head_end
            } else {
                t.request.head_end
            }
        },
        move |t, v| {
            if response {
                t.response.head_end = v;
            } else {
                t.request.head_end = v;
            }
        },
    );

    // request-line / status-line spans. `first` selects code/method vs
    // reason/target; `end` selects the span endpoint.
    for first in [true, false] {
        let which = match (response, first) {
            (true, true) => "code",
            (true, false) => "reason",
            (false, true) => "method",
            (false, false) => "target",
        };
        for (end, label) in [(false, "start"), (true, "end")] {
            m.push_pm1(
                &format!("{side}.{which}.{label}"),
                move |t| {
                    let s = line_span(t, response, first);
                    if end { s.end } else { s.start }
                },
                move |t, v| {
                    let s = line_span_mut(t, response, first);
                    if end {
                        s.end = v;
                    } else {
                        s.start = v;
                    }
                },
            );
        }
    }

    // header records: every name/value endpoint +-1; plus structural
    // delete/duplicate/swap.
    let nheaders = if response {
        base.response.headers.len()
    } else {
        base.request.headers.len()
    };
    for hi in 0..nheaders {
        for (is_value, end, label) in [
            (false, false, "name.start"),
            (false, true, "name.end"),
            (true, false, "value.start"),
            (true, true, "value.end"),
        ] {
            m.push_pm1(
                &format!("{side}.hdr[{hi}].{label}"),
                move |t| {
                    let h = side_headers_ref(t, response)[hi];
                    let s = if is_value { h.value } else { h.name };
                    if end { s.end } else { s.start }
                },
                move |t, v| {
                    let h = &mut side_headers_mut(t, response)[hi];
                    let s = if is_value { &mut h.value } else { &mut h.name };
                    if end {
                        s.end = v;
                    } else {
                        s.start = v;
                    }
                },
            );
        }
    }
    if nheaders > 0 {
        m.push(format!("{side}.hdr.delete[0]"), move |t| {
            side_headers_mut(t, response).remove(0);
        });
        m.push(format!("{side}.hdr.dup[0]"), move |t| {
            let h = side_headers_mut(t, response)[0];
            side_headers_mut(t, response).insert(0, h);
        });
    }
    if nheaders >= 2 {
        m.push(format!("{side}.hdr.swap[0,1]"), move |t| {
            side_headers_mut(t, response).swap(0, 1);
        });
    }

    // body: Some<->None.
    if side_body(base, response).is_some() {
        m.push(format!("{side}.body=None"), move |t| {
            if response {
                t.response.body = None;
            } else {
                t.request.body = None;
            }
        });
    } else {
        m.push(format!("{side}.body=Some(minimal)"), move |t| {
            if response {
                t.response.body = Some(minimal_body());
            } else {
                t.request.body = Some(minimal_body());
            }
        });
    }

    // body internals (only when present).
    let Some(body) = side_body(base, response) else {
        return;
    };

    // raw.start/end, content_len +-1.
    for (end, label) in [(false, "raw.start"), (true, "raw.end")] {
        m.push_pm1(
            &format!("{side}.body.{label}"),
            move |t| {
                let b = side_body(t, response).unwrap();
                if end { b.raw.end } else { b.raw.start }
            },
            move |t, v| {
                let b = side_body_mut(t, response);
                if end {
                    b.raw.end = v;
                } else {
                    b.raw.start = v;
                }
            },
        );
    }
    m.push_pm1(
        &format!("{side}.body.content_len"),
        move |t| side_body(t, response).unwrap().content_len,
        move |t, v| side_body_mut(t, response).content_len = v,
    );

    // framing cycle (the 2 other variants).
    for fr in FRAMINGS {
        if fr != body.framing {
            m.push(format!("{side}.body.framing={fr:?}"), move |t| {
                side_body_mut(t, response).framing = fr;
            });
        }
    }

    // trailer records: every span endpoint +-1; structural ops.
    let ntrailers = body.trailers.len();
    for ti in 0..ntrailers {
        for (is_value, end, label) in [
            (false, false, "name.start"),
            (false, true, "name.end"),
            (true, false, "value.start"),
            (true, true, "value.end"),
        ] {
            m.push_pm1(
                &format!("{side}.trailer[{ti}].{label}"),
                move |t| {
                    let h = side_body(t, response).unwrap().trailers[ti];
                    let s = if is_value { h.value } else { h.name };
                    if end { s.end } else { s.start }
                },
                move |t, v| {
                    let h = &mut side_body_mut(t, response).trailers[ti];
                    let s = if is_value { &mut h.value } else { &mut h.name };
                    if end {
                        s.end = v;
                    } else {
                        s.start = v;
                    }
                },
            );
        }
    }
    if ntrailers > 0 {
        m.push(format!("{side}.trailer.delete[0]"), move |t| {
            side_body_mut(t, response).trailers.remove(0);
        });
        m.push(format!("{side}.trailer.dup[0]"), move |t| {
            let h = side_body_mut(t, response).trailers[0];
            side_body_mut(t, response).trailers.insert(0, h);
        });
    }
    if ntrailers >= 2 {
        m.push(format!("{side}.trailer.swap[0,1]"), move |t| {
            side_body_mut(t, response).trailers.swap(0, 1);
        });
    }

    // JSON nodes: sampled endpoint +-1, size +-1, kind cycle, and structural
    // delete/duplicate/swap on the sampled indices.
    let Some(json) = &body.json else {
        return;
    };
    let nnodes = json.nodes.len();
    let indices = sampled_node_indices(nnodes);
    for &ni in &indices {
        for (sel, label) in [(0u8, "start"), (1, "end"), (2, "size")] {
            m.push_pm1(
                &format!("{side}.json[{ni}].{label}"),
                move |t| {
                    let n = side_body(t, response).unwrap().json.as_ref().unwrap().nodes[ni];
                    match sel {
                        0 => n.start,
                        1 => n.end,
                        _ => n.size,
                    }
                },
                move |t, v| {
                    let n = &mut side_body_mut(t, response).json.as_mut().unwrap().nodes[ni];
                    match sel {
                        0 => n.start = v,
                        1 => n.end = v,
                        _ => n.size = v,
                    }
                },
            );
        }
        let cur = json.nodes[ni].kind;
        for k in other_kinds(cur) {
            m.push(format!("{side}.json[{ni}].kind={k:?}"), move |t| {
                side_body_mut(t, response).json.as_mut().unwrap().nodes[ni].kind = k;
            });
        }
    }
    // structural node ops on sampled indices.
    for &ni in &indices {
        m.push(format!("{side}.json.delete[{ni}]"), move |t| {
            side_body_mut(t, response)
                .json
                .as_mut()
                .unwrap()
                .nodes
                .remove(ni);
        });
        m.push(format!("{side}.json.dup[{ni}]"), move |t| {
            let n = side_body_mut(t, response).json.as_mut().unwrap().nodes[ni];
            side_body_mut(t, response)
                .json
                .as_mut()
                .unwrap()
                .nodes
                .insert(ni, n);
        });
        if ni + 1 < nnodes {
            m.push(format!("{side}.json.swap[{ni},{}]", ni + 1), move |t| {
                side_body_mut(t, response)
                    .json
                    .as_mut()
                    .unwrap()
                    .nodes
                    .swap(ni, ni + 1);
            });
        }
    }
}

/// Immutable header-vector borrow companion to `side_headers_mut`.
fn side_headers_ref(t: &SpanTable, response: bool) -> &[HeaderSpan] {
    if response {
        &t.response.headers
    } else {
        &t.request.headers
    }
}

/// Reads the request-line / status-line span selected by `response`+`first`
/// (code/method when `first`, else reason/target).
fn line_span(t: &SpanTable, response: bool, first: bool) -> Span {
    match (response, first) {
        (true, true) => t.response.code,
        (true, false) => t.response.reason,
        (false, true) => t.request.method,
        (false, false) => t.request.target,
    }
}

/// Mutable companion to `line_span`.
fn line_span_mut(t: &mut SpanTable, response: bool, first: bool) -> &mut Span {
    match (response, first) {
        (true, true) => &mut t.response.code,
        (true, false) => &mut t.response.reason,
        (false, true) => &mut t.request.method,
        (false, false) => &mut t.request.target,
    }
}

/// Mutable body borrow companion to `side_body`.
fn side_body_mut(t: &mut SpanTable, response: bool) -> &mut BodySpans {
    if response {
        t.response.body.as_mut().unwrap()
    } else {
        t.request.body.as_mut().unwrap()
    }
}

#[test]
fn part1_generic_mutator() {
    let corpus = discover_corpus();
    assert!(!corpus.is_empty(), "fixture corpus is empty");

    let mut total = 0u64;
    for name in &corpus {
        let (sent, recv, table) = parsed(name);
        assert!(
            validate(&sent, &recv, &table).is_ok(),
            "{name}: honest table must validate Ok"
        );

        // version +-1 (top-level field, applies to every fixture).
        let mut m = Mutations::new(&table);
        m.push_pm1("version", |t| t.version as u32, |t, v| t.version = v as u16);
        collect_side_mutations(&mut m, false);
        collect_side_mutations(&mut m, true);

        for (label, cand) in &m.items {
            match validate_outcome(&sent, &recv, cand) {
                Ok(false) => total += 1,
                Ok(true) => {
                    panic!(
                        "{name}: SOUNDNESS — mutation `{label}` validated Ok (forged table accepted)"
                    )
                }
                Err(()) => {
                    panic!("{name}: ROBUSTNESS — mutation `{label}` panicked the validator")
                }
            }
        }
    }
    eprintln!(
        "part1_generic_mutator: {total} rejecting mutations over {} fixtures",
        corpus.len()
    );
    assert!(total > 1000, "expected a substantial sweep, got {total}");
}

/// Rule G1 (the ONLY sanctioned acceptance): dropping a JSON claim
/// (`body.json: Some -> None`) downgrades the body to opaque and MUST still
/// validate `Ok`. Exercised over every fixture side that carries a JSON claim.
#[test]
fn part1_g1_drop_json_claim() {
    let corpus = discover_corpus();
    let mut dropped = 0u64;
    for name in &corpus {
        let (sent, recv, table) = parsed(name);
        for response in [false, true] {
            let has_json = side_body(&table, response)
                .and_then(|b| b.json.as_ref())
                .is_some();
            if !has_json {
                continue;
            }
            let m = mutated(&table, |t| {
                side_body_mut(t, response).json = None;
            });
            assert_ne!(&m, &table, "{name}: drop-json must change the table");
            match validate_outcome(&sent, &recv, &m) {
                Ok(true) => dropped += 1,
                Ok(false) => panic!(
                    "{name}: G1 VIOLATION — dropping JSON claim ({}) was rejected",
                    if response { "response" } else { "request" }
                ),
                Err(()) => panic!("{name}: G1 — dropping JSON claim panicked the validator"),
            }
        }
    }
    eprintln!("part1_g1_drop_json_claim: {dropped} JSON claims dropped and re-accepted");
    assert!(
        dropped > 0,
        "expected at least one JSON-bearing fixture side"
    );
}

// === PART 2: named targeted attacks ===
//
// Two attack shapes:
//   * crafted-INVALID-bytes — the wire bytes themselves are illegal; the host
//     `parse_transcript` MUST refuse them (a forged table cannot then exist).
//   * valid-bytes-forged-table — honest bytes parse fine; we tamper the table
//     and assert the honest table passes while the tamper is rejected by
//     `validate`.
// Each test is commented with the rule (group A-G / B8 dup / C1-C5 framing /
// D1-D3 response framing / E chunk walk / F JSON) it probes.

// --- small table-builder helpers (mirror v2_soundness_probe style) ---

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

/// The honest request table for [`GET_SENT`].
fn get_request() -> RequestSpans {
    RequestSpans {
        method: sp(0, 3),
        target: sp(4, 6),
        head_end: 28,
        headers: vec![hdr(17, 21, 23, 24)],
        body: None,
    }
}

/// Builds `200 OK` + `Content-Length` over `content`, with the head pinned so
/// every span is exact. Returns `(recv, table)` for the honest JSON `nodes`;
/// the caller may tamper the returned table.
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

/// Replaces the response body's JSON node list in place.
fn set_nodes(table: &mut SpanTable, nodes: Vec<JsonNode>) {
    table.response.body.as_mut().unwrap().json = Some(JsonSpans { nodes });
}

/// Asserts the honest table validates and the tampered one does not.
fn honest_then_tamper(
    sent: &[u8],
    recv: &[u8],
    honest: &SpanTable,
    tampered: &SpanTable,
    msg: &str,
) {
    assert!(
        validate(sent, recv, honest).is_ok(),
        "harness: honest table for `{msg}` must validate"
    );
    assert_ne!(honest, tampered, "tamper for `{msg}` must change the table");
    assert!(
        validate(sent, recv, tampered).is_err(),
        "FORGERY: tampered table for `{msg}` was accepted"
    );
}

// ===========================================================================
// Group: request smuggling / framing (rules B8, C1, C3, C5).
// ===========================================================================

#[test]
fn attack_dup_content_length_rejected() {
    // Rule B8 (dup Content-Length): two Content-Length lines are a classic
    // request-smuggling vector; the host parser must reject the bytes.
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\nhi";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "duplicate Content-Length must not parse"
    );
}

#[test]
fn attack_smuggled_second_request_after_cl_body() {
    // Rule C3/coverage: a valid CL:2 body "hi" then a whole second request
    // smuggled after it. The honest message ends at head_end+2; any body claim
    // must satisfy head_end+len == buf.len(), so the trailing request cannot be
    // hidden. Forge a table claiming only the first body.
    let mut recv = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi".to_vec();
    let head_end = (recv.len() - 2) as u32;
    recv.extend_from_slice(b"GET /evil HTTP/1.1\r\nHost: x\r\n\r\n");
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 15),
        head_end,
        headers: vec![hdr(17, 31, 33, 34)],
        body: Some(BodySpans {
            framing: Framing::ContentLength,
            raw: sp(head_end, head_end + 2),
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
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: smuggled second request after CL body"
    );
}

#[test]
fn attack_cl_and_te_both_present() {
    // Rule C1: Content-Length AND Transfer-Encoding present together is the
    // root smuggling ambiguity and must be rejected by the parser.
    let recv =
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "CL + TE both present must not parse"
    );
}

#[test]
fn attack_dup_host_rejected() {
    // Rule B8 (dup Host): two Host lines must be rejected (ambiguous authority).
    let sent = b"GET /a HTTP/1.1\r\nHost: x\r\nHost: y\r\n\r\n";
    let recv = b"HTTP/1.1 204 No Content\r\n\r\n";
    assert!(
        parse_transcript(sent, recv).is_err(),
        "duplicate Host must not parse"
    );
}

#[test]
fn attack_cl_value_plus_sign() {
    // Rule C3: Content-Length grammar is bare DIGITs; a leading '+' is illegal.
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: +5\r\n\r\nhello";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "Content-Length `+5` must not parse"
    );
}

#[test]
fn attack_cl_value_hex() {
    // Rule C3: `0x5` is not a decimal Content-Length; must be rejected.
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 0x5\r\n\r\nhello";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "Content-Length `0x5` must not parse"
    );
}

#[test]
fn attack_cl_value_embedded_space() {
    // Rule C3 (the " 5" case): a *leading* SP is OWS and is trimmed away
    // (`Content-Length:  5` is the valid value `5`), so a space can only break
    // the grammar when it survives the trim — i.e. embedded between non-OWS
    // bytes. `5 5` is that faithful realization: after OWS-trim the value is
    // `5 5`, which is not bare DIGITs and must be rejected. (Documents that
    // leading-OWS " 5" is NOT an attack surface — the trim canonicalizes it.)
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5 5\r\n\r\nhello";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "Content-Length `5 5` must not parse"
    );
}

#[test]
fn attack_cl_value_comma_list() {
    // Rule C3: a comma-separated list `5,5` (header-folding smuggling trick) is
    // not a single decimal value; must be rejected.
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5,5\r\n\r\nhello";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "Content-Length `5,5` must not parse"
    );
}

// ===========================================================================
// Group: header line well-formedness (rules B4/B5/B6).
// ===========================================================================

#[test]
fn attack_obs_fold_continuation() {
    // Rule B5: an obs-fold (a continuation line starting with SP/HTAB) is
    // forbidden; the header value may not be line-folded.
    let recv = b"HTTP/1.1 200 OK\r\nX-Long: a\r\n b\r\nContent-Length: 0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "obs-fold continuation must not parse"
    );
}

#[test]
fn attack_bare_lf_line_ending() {
    // Rule B (strict CRLF): a bare LF terminating a header line (no CR) must be
    // rejected — lenient LF handling is a smuggling desync vector.
    let recv = b"HTTP/1.1 200 OK\nContent-Length: 0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "bare-LF line ending must not parse"
    );
}

#[test]
fn attack_space_before_colon() {
    // Rule B4: whitespace between the field name and `:` is forbidden
    // (RFC 9112 deprecation / smuggling vector).
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length : 0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "space before colon must not parse"
    );
}

// ===========================================================================
// Group: chunked-body wire grammar (rule group E).
// ===========================================================================

#[test]
fn attack_chunk_size_leading_space() {
    // Rule E: a chunk-size line with a leading space (" 5") is not a bare hex
    // size; must be rejected.
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n 5\r\nhello\r\n0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "chunk-size ` 5` must not parse"
    );
}

#[test]
fn attack_chunk_size_overlong_hex() {
    // Rule E: a 17-hex-digit chunk size overflows the 2^30 cap (and a u64); the
    // chunk walk must reject before reading data.
    let recv =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10000000000000000\r\nx\r\n0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "17-hexdigit chunk size must not parse"
    );
}

#[test]
fn attack_chunk_missing_crlf_after_data() {
    // Rule E: each chunk's data must be followed by a strict CRLF; omitting it
    // (running data straight into the next size) must be rejected.
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "chunk missing CRLF after data must not parse"
    );
}

#[test]
fn attack_chunk_cr_in_extension() {
    // Rule E: a bare CR inside a chunk extension (before the terminating CRLF)
    // is illegal control injection; must be rejected.
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;a\rb\r\nhello\r\n0\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "CR in chunk extension must not parse"
    );
}

#[test]
fn attack_bytes_after_terminal_zero_chunk() {
    // Rule E/coverage: trailing bytes after the terminal 0-chunk + CRLFCRLF are
    // a smuggled message; must be rejected.
    let recv =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\nSMUGGLED";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "bytes after terminal 0-chunk must not parse"
    );
}

#[test]
fn attack_trailer_named_content_length() {
    // Rule E/B8: a chunked trailer named Content-Length is forbidden
    // (trailer-smuggling); must be rejected.
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nContent-Length: 3\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "trailer named Content-Length must not parse"
    );
}

// ===========================================================================
// Group: framing-tag / coverage forgeries (rules C/D, forged tables).
// ===========================================================================

#[test]
fn attack_close_framing_with_cl_header() {
    // Rule C4/D2: Close framing is legal only when neither CL nor TE is present.
    // Bytes carry CL:5; honest framing is ContentLength. Forge a table claiming
    // Close over the same bytes.
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    let head_end = 38u32;
    let mk = |framing: Framing| SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response: ResponseSpans {
            code: sp(9, 12),
            reason: sp(13, 15),
            head_end,
            headers: vec![hdr(17, 31, 33, 34)],
            body: Some(BodySpans {
                framing,
                raw: sp(head_end, head_end + 5),
                content_len: 5,
                trailers: vec![],
                json: None,
            }),
        },
    };
    honest_then_tamper(
        GET_SENT,
        recv,
        &mk(Framing::ContentLength),
        &mk(Framing::Close),
        "Close framing claimed while CL header present",
    );
}

#[test]
fn attack_close_raw_stops_before_buffer_end() {
    // Rule D2/coverage: Close-delimited body must extend to the end of the
    // buffer. Forge a raw span (and content_len) that stops one byte early,
    // hiding a trailing byte.
    let recv = b"HTTP/1.1 200 OK\r\nX-Mark: 1\r\n\r\nhello";
    let head_end = 30u32;
    let end = recv.len() as u32;
    let mk = |raw_end: u32, content_len: u32| SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response: ResponseSpans {
            code: sp(9, 12),
            reason: sp(13, 15),
            head_end,
            headers: vec![hdr(17, 23, 25, 26)],
            body: Some(BodySpans {
                framing: Framing::Close,
                raw: sp(head_end, raw_end),
                content_len,
                trailers: vec![],
                json: None,
            }),
        },
    };
    honest_then_tamper(
        GET_SENT,
        recv,
        &mk(end, end - head_end),
        &mk(end - 1, end - head_end - 1),
        "Close raw span stopping before buffer end",
    );
}

#[test]
fn attack_head_response_carrying_body_bytes() {
    // Rule D3: a HEAD response never has a body. Bytes carry CL:5 + "hello";
    // honest table has body: None and the trailing bytes make even the honest
    // parse impossible — so this is a pure crafted-bytes rejection: a HEAD
    // response with body bytes present cannot be covered.
    let sent = b"HEAD /a HTTP/1.1\r\nHost: x\r\n\r\n";
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    // Forge a body claim anyway (D3 says HEAD => no body, so head_end must equal
    // buf.len(); the 5 trailing bytes are uncovered).
    let request = RequestSpans {
        method: sp(0, 4),
        target: sp(5, 7),
        head_end: 28,
        headers: vec![hdr(18, 22, 24, 25)],
        body: None,
    };
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 15),
        head_end: 38,
        headers: vec![hdr(17, 31, 33, 34)],
        body: Some(BodySpans {
            framing: Framing::ContentLength,
            raw: sp(38, 43),
            content_len: 5,
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
        "FORGERY: HEAD response carrying body bytes"
    );
}

#[test]
fn attack_204_with_body_bytes() {
    // Rule D3: a 204 response never has a body; trailing body bytes after the
    // head cannot be covered and must be rejected.
    let recv = b"HTTP/1.1 204 No Content\r\n\r\noops";
    let response = ResponseSpans {
        code: sp(9, 12),
        reason: sp(13, 23),
        head_end: 27,
        headers: vec![],
        body: Some(BodySpans {
            framing: Framing::Close,
            raw: sp(27, 31),
            content_len: 4,
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
        "FORGERY: 204 with body bytes"
    );
}

// ===========================================================================
// Group: status-line forgeries (rule D1).
// ===========================================================================

#[test]
fn attack_status_099() {
    // Rule D1: the first status digit must be 1..=5; `099` is out of range.
    let recv = b"HTTP/1.1 099 Weird\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "status 099 must not parse"
    );
}

#[test]
fn attack_status_600() {
    // Rule D1: `600` has a leading digit > 5; out of range.
    let recv = b"HTTP/1.1 600 Weird\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "status 600 must not parse"
    );
}

#[test]
fn attack_status_non_digit() {
    // Rule D1: the status code is exactly 3 DIGITs; `2a0` contains a non-digit.
    let recv = b"HTTP/1.1 2a0 Weird\r\n\r\n";
    assert!(
        parse_transcript(GET_SENT, recv).is_err(),
        "status 2a0 must not parse"
    );
}

#[test]
fn attack_reason_span_swallows_crlf() {
    // Rule D1: the reason span is charset-checked (no CR/LF). Bytes are a clean
    // `200 OK`; forge a reason span extended over the terminating CR/LF.
    let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    let honest = SpanTable {
        version: FORMAT_VERSION,
        request: get_request(),
        response: ResponseSpans {
            code: sp(9, 12),
            reason: sp(13, 15),
            head_end: recv.len() as u32,
            headers: vec![hdr(17, 31, 33, 34)],
            body: None,
        },
    };
    let mut tampered = honest.clone();
    tampered.response.reason = sp(13, 17); // swallow the CRLF after "OK"
    honest_then_tamper(
        GET_SENT,
        recv,
        &honest,
        &tampered,
        "reason span swallowing the CRLF",
    );
}

// ===========================================================================
// Group: JSON-claim forgeries over binary / opaque bodies (rule F + G).
// ===========================================================================

#[test]
fn attack_conjured_json_over_binary_bytes() {
    // Rule F: a gzip-ish/binary body claimed as JSON must be rejected (the
    // first byte 0x1f is not a JSON value); the SAME bytes claimed opaque
    // (json:None) must be ACCEPTED (rule G — the sanctioned opaque freedom).
    let content: Vec<u8> = vec![0x1f, 0x8b, 0x08, 0x00, 0x00, 0xff, 0x01, 0x02];
    let (recv, mut forged) = cl_json(&content, vec![node(JsonKind::Object, 0, 8, 1)]);
    assert!(
        validate(GET_SENT, &recv, &forged).is_err(),
        "FORGERY: conjured JSON over binary bytes"
    );
    // Same bytes, opaque claim: accepted.
    forged.response.body.as_mut().unwrap().json = None;
    assert!(
        validate(GET_SENT, &recv, &forged).is_ok(),
        "G: opaque claim over binary bytes must be accepted"
    );
}

#[test]
fn attack_te_chunked_uppercase_claimed_close() {
    // Rule C2/D2: `Transfer-Encoding: CHUNKED` (uppercase) is chunked
    // (case-insensitive token); honest framing is Chunked. Forge a table
    // claiming Close. Build the honest table by parse, then tamper the tag.
    let recv = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: CHUNKED\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    let honest = parse_transcript(GET_SENT, recv).expect("uppercase chunked must parse");
    assert_eq!(
        honest.response.body.as_ref().unwrap().framing,
        Framing::Chunked,
        "uppercase CHUNKED must derive Chunked framing"
    );
    let mut tampered = honest.clone();
    tampered.response.body.as_mut().unwrap().framing = Framing::Close;
    honest_then_tamper(
        GET_SENT,
        recv,
        &honest,
        &tampered,
        "uppercase CHUNKED claimed as Close",
    );
}

// ===========================================================================
// Group: JSON structural forgeries (rule group F, forged node trees).
// ===========================================================================

#[test]
fn attack_json_two_roots() {
    // Rule F1/coverage: `{} {}` is two values; only the first can be the root,
    // and the trailing `{}` is uncovered. Claiming the first as root must fail
    // (a second root value is hidden).
    let (recv, table) = cl_json(b"{} {}", vec![node(JsonKind::Object, 0, 2, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: {{}} {{}} second root hidden"
    );
}

#[test]
fn attack_json_trailing_comma_object() {
    // Rule F: `{"a":1,}` has a trailing comma — not valid RFC 8259; the object
    // walk must reject it however the nodes are claimed.
    let content = br#"{"a":1,}"#;
    let nodes = vec![
        node(JsonKind::Object, 0, 8, 3),
        node(JsonKind::Key, 2, 3, 1),
        node(JsonKind::Number, 5, 6, 1),
    ];
    let (recv, table) = cl_json(content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: trailing comma object"
    );
}

#[test]
fn attack_json_comment_in_gap() {
    // Rule F: JSON has no comments; a `/*c*/` in an inter-element gap is not
    // whitespace and must be rejected. `[1/*c*/,2]` claimed as a 2-element array.
    let content = b"[1/*c*/,2]";
    let nodes = vec![
        node(JsonKind::Array, 0, content.len() as u32, 3),
        node(JsonKind::Number, 1, 2, 1),
        node(JsonKind::Number, 7, 8, 1),
    ];
    let (recv, table) = cl_json(content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: comment in JSON gap"
    );
}

#[test]
fn attack_json_number_span_truncated_inside_digits() {
    // Rule F: a Number span must be the maximal lexeme. Body `123`; claiming
    // `12` (span [0,2)) leaves a trailing `3` glued to the number and must fail.
    let (recv, table) = cl_json(b"123", vec![node(JsonKind::Number, 0, 2, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: number span `12` inside `123`"
    );
}

#[test]
fn attack_json_number_span_excludes_exponent() {
    // Rule F: a Number span must include its exponent. Body `1e3`; claiming
    // span [0,1) (`1`) drops `e3`, which is then glued garbage; must fail.
    let (recv, table) = cl_json(b"1e3", vec![node(JsonKind::Number, 0, 1, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: number span excludes exponent"
    );
}

#[test]
fn attack_json_string_end_inside_unicode_escape() {
    // Rule F: a String span end may not split a `\uXXXX` escape. Build content
    // `"A"` (a literal backslash-u-0041); the content runs [1,7). Claim end
    // mid-escape ([1,3)) and assert rejection (honest [1,7) must pass).
    let content: Vec<u8> = {
        let mut v = vec![b'"'];
        v.push(0x5C); // backslash
        v.extend_from_slice(b"u0041");
        v.push(b'"');
        v
    };
    assert_eq!(content.len(), 8); // " \ u 0 0 4 1 "
    let (recv, mut table) = cl_json(&content, vec![node(JsonKind::String, 1, 7, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_ok(),
        "honest \\u escape string must validate"
    );
    set_nodes(&mut table, vec![node(JsonKind::String, 1, 3, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: string end inside \\uXXXX escape"
    );
}

#[test]
fn attack_json_literal_true_wrong_case() {
    // Rule F: literals are lowercase. `True` is not the literal `true`; a Bool
    // node over it must be rejected.
    let (recv, table) = cl_json(b"True", vec![node(JsonKind::Bool, 0, 4, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: literal `True`"
    );
}

#[test]
fn attack_json_literal_null_wrong_case() {
    // Rule F: `NULL` is not the literal `null`; a Null node over it must fail.
    let (recv, table) = cl_json(b"NULL", vec![node(JsonKind::Null, 0, 4, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: literal `NULL`"
    );
}

#[test]
fn attack_json_quoted_true_claimed_bool() {
    // Rule F: `"true"` is a String, not a Bool. A Bool node claimed over the
    // quoted lexeme must be rejected (the first byte is `"`).
    let (recv, table) = cl_json(br#""true""#, vec![node(JsonKind::Bool, 1, 5, 1)]);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: quoted `\"true\"` claimed Bool"
    );
}

#[test]
fn attack_json_duplicate_keys_literal() {
    // Rule F: duplicate object keys are forbidden. `{"a":1,"a":2}` has two
    // literally-equal keys; must be rejected.
    let content = br#"{"a":1,"a":2}"#;
    let nodes = vec![
        node(JsonKind::Object, 0, 13, 5),
        node(JsonKind::Key, 2, 3, 1),
        node(JsonKind::Number, 5, 6, 1),
        node(JsonKind::Key, 8, 9, 1),
        node(JsonKind::Number, 11, 12, 1),
    ];
    let (recv, table) = cl_json(content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: duplicate literal keys"
    );
}

#[test]
fn attack_json_duplicate_keys_escaped_alias() {
    // Rule F: duplicate keys that DECODE equal via an escape alias. The 2nd key
    // is `a` spelled `a`. The literal backslash-u sequence is built at
    // RUNTIME (byte 0x5C followed by b"u0061") to dodge source-mangling. The
    // decoded-key comparison must catch the alias and reject the duplicate.
    let content: Vec<u8> = {
        let mut v = Vec::new();
        v.extend_from_slice(br#"{"a":1,""#);
        v.push(0x5C); // literal backslash
        v.extend_from_slice(b"u0061"); // -> decodes to 'a'
        v.extend_from_slice(br#"":2}"#);
        v
    };
    // { 0 " 1 a 2 " 3 : 4 1 5 , 6 " 7 [key2 content 8..14] " 14 : 15 2 16 } 17
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
        "FORGERY: duplicate keys via \\u0061 escape alias"
    );
}

#[test]
fn attack_json_grandchild_reparented_as_child() {
    // Rule F (subtree sizes / pre-order): re-parent a grandchild as a direct
    // child by tampering the container `size`. Body `[[1]]`: honest is
    // Array[0,5] size3, Array[1,4] size2, Number[2,3] size1. Forge the outer
    // array's size to 2 (claiming it directly contains only the inner array and
    // dropping the grandchild from its subtree). The size re-derivation rejects.
    let content = b"[[1]]";
    let honest = vec![
        node(JsonKind::Array, 0, 5, 3),
        node(JsonKind::Array, 1, 4, 2),
        node(JsonKind::Number, 2, 3, 1),
    ];
    let (recv, mut table) = cl_json(content, honest);
    assert!(
        validate(GET_SENT, &recv, &table).is_ok(),
        "honest [[1]] must validate"
    );
    set_nodes(
        &mut table,
        vec![
            node(JsonKind::Array, 0, 5, 2), // size lies: drops the grandchild
            node(JsonKind::Array, 1, 4, 2),
            node(JsonKind::Number, 2, 3, 1),
        ],
    );
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: grandchild re-parented via size"
    );
}

#[test]
fn attack_json_array_elements_reordered() {
    // Rule F (pre-order = document order): array element records must be in
    // document order. Body `[1,2]`; reorder the two element nodes and assert
    // rejection (honest order passes).
    let content = b"[1,2]";
    let honest = vec![
        node(JsonKind::Array, 0, 5, 3),
        node(JsonKind::Number, 1, 2, 1),
        node(JsonKind::Number, 3, 4, 1),
    ];
    let (recv, mut table) = cl_json(content, honest);
    assert!(
        validate(GET_SENT, &recv, &table).is_ok(),
        "honest [1,2] must validate"
    );
    set_nodes(
        &mut table,
        vec![
            node(JsonKind::Array, 0, 5, 3),
            node(JsonKind::Number, 3, 4, 1), // element 2 claimed first
            node(JsonKind::Number, 1, 2, 1),
        ],
    );
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: array elements reordered"
    );
}

#[test]
fn attack_json_depth_129_rejected() {
    // Rule A/F (depth cap 128): a 129-deep nested array exceeds the documented
    // nesting cap and must be rejected. Built by loop: `[`*129 + `1` + `]`*129.
    let depth = 129usize;
    let mut content = Vec::new();
    content.extend(std::iter::repeat_n(b'[', depth));
    content.push(b'1');
    content.extend(std::iter::repeat_n(b']', depth));
    // Build an honest-shaped pre-order node list: 129 arrays then the number,
    // each array's subtree size = (remaining arrays below) + 1 (number) + 1.
    let mut nodes = Vec::new();
    for i in 0..depth {
        let span_start = i as u32;
        let span_end = (content.len() - i) as u32;
        // subtree size: arrays below it (depth-1-i) + this array + the number
        let size = (depth - i) as u32 + 1;
        nodes.push(node(JsonKind::Array, span_start, span_end, size));
    }
    nodes.push(node(JsonKind::Number, depth as u32, depth as u32 + 1, 1));
    let (recv, table) = cl_json(&content, nodes);
    assert!(
        validate(GET_SENT, &recv, &table).is_err(),
        "FORGERY: depth-129 document accepted (cap is 128)"
    );
}
