//! Differential suite: the verified JSON view exposed by `validate` is
//! checked node-by-node against `serde_json` over the fixture corpus.
//!
//! For every fixture body the host claims as JSON ([`parse_transcript`]
//! emitted `json` spans), the decoded content is parsed independently with
//! `serde_json` and the two trees are walked in lockstep: kinds must
//! correspond everywhere, containers must agree on element/member counts,
//! and leaves must agree on values wherever the two crates expose the same
//! representation. Documented representation differences are compared
//! structurally instead of by value:
//!
//! - We expose raw string bytes (escapes undecoded); serde decodes escapes.
//!   Strings and keys whose raw bytes contain `\` are kind-checked (and
//!   counted), never value-compared.
//! - Our object iteration order is document order; serde's default `Map` is not
//!   order-preserving. Objects are compared as key sets with per-key subtree
//!   recursion, never by order.
//! - Numbers are type-compared always; values are compared as exact-`f64`
//!   parses of the SAME lexeme (both parsers are correctly rounded, so equality
//!   is exact — no epsilon), plus an exact integer cross-check when both sides
//!   parse the lexeme as an integer. Lexemes like `1e999` are valid for us but
//!   unrepresentable for serde — serde rejects the whole document, so they
//!   cannot appear inside an accepted `Value`.
//! - serde_json rejects two corpus documents outright: `syn_unicode` (a
//!   lone-surrogate `\uD800` escape, valid RFC 8259 *grammar* which we accept
//!   raw) and `syn_deep_128` (serde's stock recursion limit admits at most 127
//!   nested containers; our documented cap is 128). For those the rejection
//!   itself is asserted and our tree is kind-walked instead.

use std::{collections::BTreeSet, fs, path::PathBuf};

use serde_json::Value;
use transcript_verify::{Body, JsonKind, JsonValue, parse_transcript, validate};

/// The expected JSON claim for one side of a fixture.
#[derive(Clone, Copy)]
enum Claim {
    /// No JSON claim: the side has no body at all, or the host must claim
    /// it opaque (non-JSON `Content-Type`, or no `Content-Type`).
    Opaque,
    /// A JSON claim. `serde_rejects` is `Some(why)` when stock `serde_json`
    /// must reject the decoded document; the rejection is then asserted and
    /// our tree is kind-walked instead of value-compared.
    Json { serde_rejects: Option<&'static str> },
}

impl Claim {
    fn is_json(self) -> bool {
        matches!(self, Claim::Json { .. })
    }
}

/// One fixture pair and its expected per-side JSON claims.
struct Fixture {
    name: &'static str,
    request: Claim,
    response: Claim,
}

const OPAQUE: Claim = Claim::Opaque;
const JSON: Claim = Claim::Json {
    serde_rejects: None,
};

const fn fixture(name: &'static str, request: Claim, response: Claim) -> Fixture {
    Fixture {
        name,
        request,
        response,
    }
}

/// The complete corpus (see `fixtures/README.md`), with the expected JSON
/// claim per side. `corpus_is_fully_covered` asserts this list and the
/// on-disk corpus never drift apart.
const FIXTURES: &[Fixture] = &[
    // Real captures.
    fixture("pokeapi_ditto", OPAQUE, JSON),
    fixture("swapi_films", OPAQUE, JSON),
    fixture("httpbingo_get", OPAQUE, JSON),
    fixture("postman_post_json", JSON, JSON),
    fixture("postman_post_form", OPAQUE, JSON),
    fixture("httpbingo_post_multipart", OPAQUE, JSON),
    fixture("httpbingo_xml", OPAQUE, OPAQUE),
    fixture("httpbingo_png", OPAQUE, OPAQUE),
    fixture("postman_204", OPAQUE, OPAQUE),
    fixture("example_html", OPAQUE, OPAQUE),
    fixture("example_head", OPAQUE, OPAQUE),
    fixture("github_head", OPAQUE, OPAQUE),
    fixture("jsonplaceholder_post", JSON, JSON),
    fixture("github_zen", OPAQUE, OPAQUE),
    fixture("icanhazip", OPAQUE, OPAQUE),
    // Synthetic fixtures.
    fixture("syn_close_delim", OPAQUE, JSON),
    fixture("syn_trailers", OPAQUE, JSON),
    fixture("syn_chunked_request", JSON, JSON),
    fixture("syn_cl_zero", OPAQUE, JSON),
    fixture("syn_empty_reason", OPAQUE, OPAQUE),
    fixture("syn_no_reason", OPAQUE, OPAQUE),
    fixture("syn_empty_header_value", OPAQUE, JSON),
    fixture("syn_dup_set_cookie", OPAQUE, JSON),
    fixture("syn_obs_text", OPAQUE, OPAQUE),
    fixture("syn_chunk_split_token", OPAQUE, JSON),
    fixture("syn_root_number", OPAQUE, JSON),
    fixture("syn_root_string", OPAQUE, JSON),
    fixture("syn_root_bool", OPAQUE, JSON),
    fixture("syn_root_null", OPAQUE, JSON),
    fixture("syn_empty_containers", OPAQUE, JSON),
    fixture(
        "syn_unicode",
        OPAQUE,
        Claim::Json {
            serde_rejects: Some("lone-surrogate \\uD800 escape"),
        },
    ),
    fixture("syn_deep_127", OPAQUE, JSON),
    fixture(
        "syn_deep_128",
        OPAQUE,
        Claim::Json {
            serde_rejects: Some(
                "serde_json's recursion limit admits at most 127 nested containers",
            ),
        },
    ),
];

// === fixture loading (self-contained) ===

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn fixture_path(name: &str, ext: &str) -> PathBuf {
    let mut path = fixtures_root();
    if name.starts_with("syn_") {
        path.push("synthetic");
    }
    path.push(format!("{name}.{ext}.bin"));
    path
}

/// Loads a fixture pair as exact wire bytes.
fn load_fixture(name: &str) -> (Vec<u8>, Vec<u8>) {
    let read = |ext: &str| {
        let path = fixture_path(name, ext);
        fs::read(&path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
    };
    (read("sent"), read("recv"))
}

// === the differential walk ===

/// A short kind descriptor for a serde value (full `Debug` of a big subtree
/// would drown the assertion message).
fn serde_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "Null",
        Value::Bool(_) => "Bool",
        Value::Number(_) => "Number",
        Value::String(_) => "String",
        Value::Array(_) => "Array",
        Value::Object(_) => "Object",
    }
}

/// Recursively compares our verified `JsonValue` tree against serde's
/// `Value` tree, bumping `nodes` for every one of OUR nodes visited (values
/// and keys).
fn compare_value(ctx: &str, ours: JsonValue<'_>, theirs: &Value, path: &str, nodes: &mut u64) {
    *nodes += 1;
    match (ours.kind(), theirs) {
        (JsonKind::Null, Value::Null) => {
            assert!(ours.is_null(), "{ctx}: is_null() false for Null at {path}");
        }
        (JsonKind::Bool, Value::Bool(theirs_bool)) => {
            assert_eq!(
                ours.as_bool(),
                Some(*theirs_bool),
                "{ctx}: bool mismatch at {path}"
            );
        }
        (JsonKind::Number, Value::Number(theirs_num)) => {
            let lexeme = ours
                .as_number_str()
                .unwrap_or_else(|| panic!("{ctx}: number lexeme not UTF-8 at {path}"));
            // Both sides parse the SAME lexeme text and both conversions
            // are correctly rounded, so equality is exact — never epsilon.
            // The guards cannot mask a divergence inside an accepted
            // document: stock serde `Number::as_f64` is always `Some`, and
            // an out-of-range lexeme (`1e999`) makes serde reject the whole
            // document instead.
            if let (Some(theirs_f64), Ok(ours_f64)) = (theirs_num.as_f64(), lexeme.parse::<f64>()) {
                assert_eq!(
                    ours_f64, theirs_f64,
                    "{ctx}: f64 number mismatch at {path} (lexeme {lexeme:?})"
                );
            }
            // Exact integer cross-check when both sides parse the lexeme
            // as an integer (stronger than the f64 rule for 2^53..2^64).
            if let (Some(theirs_u64), Ok(ours_u64)) = (theirs_num.as_u64(), lexeme.parse::<u64>()) {
                assert_eq!(
                    ours_u64, theirs_u64,
                    "{ctx}: u64 number mismatch at {path} (lexeme {lexeme:?})"
                );
            }
            if let (Some(theirs_i64), Ok(ours_i64)) = (theirs_num.as_i64(), lexeme.parse::<i64>()) {
                assert_eq!(
                    ours_i64, theirs_i64,
                    "{ctx}: i64 number mismatch at {path} (lexeme {lexeme:?})"
                );
            }
        }
        (JsonKind::String, Value::String(theirs_str)) => {
            let raw = ours
                .as_str()
                .unwrap_or_else(|| panic!("{ctx}: string content not UTF-8 at {path}"));
            // Escape-bearing strings are exposed raw by us but decoded by
            // serde (escape decoding is a documented non-goal) — kind
            // correspondence is the contract there.
            if !raw.contains('\\') {
                assert_eq!(raw, theirs_str, "{ctx}: string mismatch at {path}");
            }
        }
        (JsonKind::Array, Value::Array(theirs_elems)) => {
            let len = ours.array_len().expect("kind is Array") as usize;
            assert_eq!(
                len,
                theirs_elems.len(),
                "{ctx}: array length mismatch at {path}"
            );
            let elems = ours.iter_array().expect("kind is Array");
            for (idx, (our_elem, their_elem)) in elems.zip(theirs_elems).enumerate() {
                compare_value(ctx, our_elem, their_elem, &format!("{path}.{idx}"), nodes);
            }
        }
        (JsonKind::Object, Value::Object(theirs_map)) => {
            let members: Vec<_> = ours.iter_object().expect("kind is Object").collect();
            assert_eq!(
                members.len(),
                theirs_map.len(),
                "{ctx}: object member-count mismatch at {path}"
            );
            // Compare key SETS plus per-key subtrees. Our iteration order
            // is document order; serde's map order is an implementation
            // detail — never compare orders.
            for (key, our_member) in members {
                *nodes += 1; // the key node
                let raw_key = key.as_str();
                if raw_key.contains('\\') {
                    // Escape-bearing key: serde's map is keyed by the
                    // DECODED string, so a raw-bytes lookup would be wrong.
                    // Member-count equality plus duplicate-key rejection
                    // (decoded comparison, rule F9) keep this honest; the
                    // member's subtree is still kind-walked and counted.
                    kind_walk(ctx, our_member, nodes);
                    continue;
                }
                let their_member = theirs_map.get(raw_key).unwrap_or_else(|| {
                    panic!("{ctx}: key {raw_key:?} missing from serde object at {path}")
                });
                compare_value(
                    ctx,
                    our_member,
                    their_member,
                    &format!("{path}.{raw_key}"),
                    nodes,
                );
            }
        }
        (kind, theirs) => panic!(
            "{ctx}: kind mismatch at {path}: ours {kind:?} vs serde {}",
            serde_kind(theirs)
        ),
    }
}

/// Walks our tree alone (used where serde rejected the document and for
/// escape-bearing-key subtrees): every node must answer the accessor for
/// its kind, and every node is counted.
fn kind_walk(ctx: &str, value: JsonValue<'_>, nodes: &mut u64) {
    *nodes += 1;
    match value.kind() {
        JsonKind::Null => assert!(value.is_null(), "{ctx}: is_null() false for Null"),
        JsonKind::Bool => assert!(value.as_bool().is_some(), "{ctx}: as_bool() none for Bool"),
        JsonKind::Number => {
            let lexeme = value.as_number_str().expect("number lexeme is UTF-8");
            assert!(!lexeme.is_empty(), "{ctx}: empty number lexeme");
        }
        JsonKind::String => {
            value.as_str().expect("string content is UTF-8");
        }
        JsonKind::Array => {
            for elem in value.iter_array().expect("kind is Array") {
                kind_walk(ctx, elem, nodes);
            }
        }
        JsonKind::Object => {
            for (key, member) in value.iter_object().expect("kind is Object") {
                *nodes += 1; // the key node
                assert!(
                    core::str::from_utf8(key.as_str().as_bytes()).is_ok(),
                    "{ctx}: key not UTF-8"
                );
                kind_walk(ctx, member, nodes);
            }
        }
        JsonKind::Key => panic!("{ctx}: kind() must never yield Key"),
    }
}

/// Runs the differential comparison for one claimed body; returns the
/// number of OUR nodes visited.
fn compare_body(ctx: &str, body: Body<'_>, serde_rejects: Option<&'static str>) -> u64 {
    let content = body.content().to_vec();
    let ours = body.json().expect("JSON claim presence was asserted");
    let mut nodes = 0;
    match serde_rejects {
        None => {
            let theirs: Value = serde_json::from_slice(&content).unwrap_or_else(|err| {
                panic!("{ctx}: serde_json rejected a body our validator accepted: {err}")
            });
            compare_value(ctx, ours, &theirs, "$", &mut nodes);
        }
        Some(why) => {
            let result = serde_json::from_slice::<Value>(&content);
            let err = result.expect_err(&format!(
                "{ctx}: expected serde_json to reject this document ({why}), but it parsed"
            ));
            eprintln!(
                "differential[{ctx}]: serde_json rejected as expected ({err}); kind-walking our tree"
            );
            kind_walk(ctx, ours, &mut nodes);
        }
    }
    nodes
}

/// Parses, validates, and differentially checks one fixture; returns the
/// (request, response) node counts.
fn run_differential(name: &str) -> (u64, u64) {
    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == name)
        .unwrap_or_else(|| panic!("fixture {name} missing from FIXTURES"));
    let (sent, recv) = load_fixture(name);

    let table = parse_transcript(&sent, &recv)
        .unwrap_or_else(|err| panic!("{name}: parse_transcript failed: {err}"));

    // The host's JSON-vs-opaque claim must match the documented corpus —
    // catches the host silently downgrading a JSON body to opaque (which
    // would otherwise skip the comparison) and vice versa.
    let request_claimed = table
        .request
        .body
        .as_ref()
        .is_some_and(|body| body.json.is_some());
    let response_claimed = table
        .response
        .body
        .as_ref()
        .is_some_and(|body| body.json.is_some());
    assert_eq!(
        request_claimed,
        fixture.request.is_json(),
        "{name}: request-side JSON claim drifted from the documented corpus"
    );
    assert_eq!(
        response_claimed,
        fixture.response.is_json(),
        "{name}: response-side JSON claim drifted from the documented corpus"
    );

    let transcript = validate(&sent, &recv, &table)
        .unwrap_or_else(|err| panic!("{name}: validate failed: {err}"));

    let request_nodes = match fixture.request {
        Claim::Opaque => 0,
        Claim::Json { serde_rejects } => {
            let body = transcript.request().body().expect("claimed body exists");
            compare_body(&format!("{name}/request"), body, serde_rejects)
        }
    };
    let response_nodes = match fixture.response {
        Claim::Opaque => 0,
        Claim::Json { serde_rejects } => {
            let body = transcript.response().body().expect("claimed body exists");
            compare_body(&format!("{name}/response"), body, serde_rejects)
        }
    };

    eprintln!(
        "differential[{name}]: request {request_nodes} nodes, response {response_nodes} nodes \
         compared"
    );
    (request_nodes, response_nodes)
}

// === tests ===

/// The on-disk corpus and the `FIXTURES` table must agree exactly, and
/// every `sent` must have its `recv` (and vice versa) — so no fixture can
/// silently escape the differential.
#[test]
fn corpus_is_fully_covered() {
    let root = fixtures_root();
    let mut sent_names = BTreeSet::new();
    let mut recv_names = BTreeSet::new();
    for dir in [root.clone(), root.join("synthetic")] {
        for entry in fs::read_dir(&dir).expect("fixture dir is readable") {
            let file_name = entry.expect("dir entry").file_name();
            let file_name = file_name.to_str().expect("fixture names are UTF-8");
            if let Some(stem) = file_name.strip_suffix(".sent.bin") {
                sent_names.insert(stem.to_string());
            } else if let Some(stem) = file_name.strip_suffix(".recv.bin") {
                recv_names.insert(stem.to_string());
            }
        }
    }
    assert_eq!(sent_names, recv_names, "unpaired fixture files");
    let listed: BTreeSet<String> = FIXTURES
        .iter()
        .map(|fixture| fixture.name.to_string())
        .collect();
    assert_eq!(
        sent_names, listed,
        "fixture corpus and the differential FIXTURES table drifted apart"
    );
}

/// The headline scale check: the pokeapi response tree is large enough for
/// the differential to be meaningful.
#[test]
fn pokeapi_compares_more_than_1000_nodes() {
    let (_, response_nodes) = run_differential("pokeapi_ditto");
    assert!(
        response_nodes > 1000,
        "expected >1000 compared nodes, got {response_nodes}"
    );
}

macro_rules! differential_tests {
    ($($name:ident),* $(,)?) => {$(
        #[test]
        fn $name() {
            run_differential(stringify!($name));
        }
    )*};
}

differential_tests! {
    pokeapi_ditto,
    swapi_films,
    httpbingo_get,
    postman_post_json,
    postman_post_form,
    httpbingo_post_multipart,
    httpbingo_xml,
    httpbingo_png,
    postman_204,
    example_html,
    example_head,
    github_head,
    jsonplaceholder_post,
    github_zen,
    icanhazip,
    syn_close_delim,
    syn_trailers,
    syn_chunked_request,
    syn_cl_zero,
    syn_empty_reason,
    syn_no_reason,
    syn_empty_header_value,
    syn_dup_set_cookie,
    syn_obs_text,
    syn_chunk_split_token,
    syn_root_number,
    syn_root_string,
    syn_root_bool,
    syn_root_null,
    syn_empty_containers,
    syn_unicode,
    syn_deep_127,
    syn_deep_128,
}
