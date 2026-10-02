# transcript-verify

Sound validation of pre-parsed HTTP/JSON transcript span tables, designed for
zkVM guests.

A transcript is a `sent` buffer holding exactly one HTTP/1.1 request and a
`recv` buffer holding exactly one response, and the goal is to answer
questions about it inside a zkVM guest — *what was the status code? what is
`user.id` in the response JSON?* Running a full HTTP + JSON parser inside the
VM is prohibitively expensive: parsing is branchy, search-heavy,
allocation-heavy work, and every cycle is proved. This crate instead applies
the **non-deterministic advice** pattern: parse once outside the VM, then
inside the VM only *verify* that parse with a single linear walk — far
cheaper than parsing, and just as trustworthy, because every claim is
re-derived from the bytes and equality-checked.

## The advice pattern

Outside the VM, `parse_transcript` parses the transcript once and emits a
flat, serializable **span table** — byte ranges plus type tags (request-line
parts, header spans, body framing, a pre-order tree of typed JSON nodes) that
act as a parse witness. Inside the VM, `validate(sent, recv, &table)`
re-derives every fact from the bytes in a single linear cursor walk and
equality-checks it against the table, so the table never steers parsing and a
forged or tampered table can never validate. On success the guest gets a
zero-copy `Transcript` accessor (method, target, status, headers by name,
JSON values by path) whose every answer is backed by a verified span — no
searching, no dynamic tree construction, and exactly one pre-sized allocation
for de-chunking.

## Security model

- **The bytes are assumed authenticated upstream.** This crate proves facts
  about a pair of byte strings, not where they came from. Binding
  `sent`/`recv` to a real TLS session (e.g. TLSNotary transcript
  commitments) is the caller's job.
- **The span table is untrusted input.** It may come from a malicious
  prover. The validator's byte walk is fully deterministic; every table
  field is *derived from the bytes, then equality-checked* — never trusted,
  never used to decide where to look next. A wrong span, a hidden header, a
  reordered record, a mis-typed JSON node, or a wrong subtree size all fail
  validation.
- **The accepted parse is canonical.** For fixed buffers, at most one
  semantically distinct table validates. The single deliberate degree of
  freedom is the **JSON-vs-opaque body claim**: a prover may present a JSON
  body as opaque bytes (`json: None`). The claim is consumer-visible —
  `Body::json()` returns `None` for an opaque claim — so a verifier that
  requires JSON must simply require `body.json().is_some()`. The claim is
  deliberately *not* tied to `Content-Type`.
- **Accessor answers are span-backed.** Everything `Transcript` returns is
  a slice of the validated buffers (or of the verified de-chunk of them),
  and `Body::content_to_source` maps decoded-body ranges back to wire-byte
  ranges for selective disclosure.

## How validation works

The whole design rests on one mechanism: a **single deterministic cursor
walks the real bytes left-to-right**, and the table is only ever checked for
*equality* against what the cursor independently derives. The table never
says where something is or what type it has — the bytes decide, and the table
must match. A lie in the table therefore fails a concrete `==`, never slips
through.

**Is the transcript well-formed?** `validate` makes one forward pass per
buffer. For the request: scan a method token from byte 0 → require a single
`SP` → scan the target → require the exact literal `` HTTP/1.1\r\n`` → walk
header lines until the blank line → derive framing → walk the body. Each step
starts exactly where the previous ended. Three things are enforced at once:
*charset gates* (method is `tchar`, target is `0x21..=0x7E`, header values
exclude CR/LF/NUL/DEL), *literal anchors* (`expect_lit` requires exact bytes —
this is where a bad version or a bare `LF` dies), and *total coverage* (the
body must end exactly at `buf.len()`, which is what stops a second message
smuggled after the first). Framing (Content-Length vs chunked vs close) is
**derived from the verified headers**, not read from the table, so a body
cannot be relabelled to grab different bytes.

**HTTP/1.1 only.** The version is a byte-exact literal match against
`HTTP/1.1` — not a parse of major/minor numbers — so `HTTP/1.0`, `http/1.1`,
and `HTTP/2.0` all fail at that anchor. This is deliberate: HTTP/2 and /3 are
*binary* protocols (no `GET /path HTTP/1.1\r\n` exists on their wire), so they
could never reach a text parser; and HTTP/1.0 frames bodies differently
(close-delimited by default, no chunked), so accepting it and applying 1.1
framing rules would be subtly unsound. Rejecting the exact literal is safer
than mis-handling it. (`spansy`/`httparse` accept 1.0; the host's self-check
turns that into an upfront error so no doomed table reaches the guest.)

**Does a header value belong to its name?** The binding is *positional, by
construction*. For each line the scanner reads: a run of `tchar` name bytes →
the `:` that must immediately follow (no space before it) → optional
whitespace → the value run up to `CR` → trailing-whitespace trim → the
required `CRLF`. The value is literally "the bytes after this name's colon, up
to this line's CRLF" — it is never matched to a name by searching, so a value
cannot drift to a different header. The table's claimed name/value spans must
then *equal* what the scan derived, and records are consumed in **lockstep**
with the lines (line *i* ↔ record *i*, with the counts required equal). So an
inserted, dropped, reordered, or one-byte-shifted header span all fail.

**Does a JSON value belong to its key?** Same principle over the JSON grammar.
The byte cursor walks the document while the pre-order node array is consumed
in lockstep. Inside an object the loop is rigid: require `"` (the node must be
kind `Key`, span equal to the scanned string) → require `:` → the *next* node
is this key's value and must start exactly at the cursor sitting just past the
colon. So a key and value are bound because the value is whatever
grammatically follows *this* key's colon. Three further checks lock it in: the
**kind is byte-forced** (the first byte — `{`, `[`, `"`, `t`/`f`, `n`, digit —
determines the only admissible `kind`); **scalar extents are equality-checked**
against a scanner's derived end (so `"12"` can't be claimed inside the bytes
`123` — the scanner returns the maximal lexeme and a short claim fails the
`==`); and **containers verify on close** (`node.end == cursor` and
`node.size == nodes consumed in the subtree`), which prevents re-parenting a
child or smuggling extra nodes. Per-object duplicate keys are rejected by
*decoded* comparison, so a key and its escaped alias — the same character
written literally in one and as a `\uXXXX` escape in the other — still
collide.

## Usage

Host side (default features; add `serde` to ship the table across the VM
boundary):

```rust
use transcript_verify::parse_transcript;

// `sent` / `recv` are the exact wire bytes of one request / one response.
let table = parse_transcript(&sent, &recv)?;

// With the `serde` feature, `SpanTable` derives Serialize/Deserialize, so
// any serde format can carry it to the guest. It is advice, not a trusted
// input — tampering with it in transit just makes `validate` fail.
let advice = postcard::to_allocvec(&table)?;
```

Guest side (`no_std` + `alloc`; build with `--no-default-features`, plus
`serde` for deserialization):

```rust
use transcript_verify::validate;

let table: SpanTable = postcard::from_bytes(&advice)?;
let transcript = validate(&sent, &recv, &table)?;

assert_eq!(transcript.request().method(), "GET");
assert_eq!(transcript.response().status(), 200);

// Header lookup is ASCII case-insensitive; duplicates iterable in order.
let content_type = transcript.response().header("content-type");

// JSON access by spansy-style dot path, zero-copy, O(path length).
let body = transcript.response().body().expect("response has a body");
let json = body.json().expect("this verifier requires a JSON body");
let name = json.get("species.name").and_then(|v| v.as_str());
let level = json.get("stats.0.base_stat").and_then(|v| v.as_number_str());

// Selective disclosure: map any decoded-body range back to wire bytes.
let span = json.get("species.name").expect("present").span();
let wire_spans = body.content_to_source(span.start..span.end);
```

A complete, compiling end-to-end example lives in the crate-level docs
(`cargo doc --open`).

## Feature flags

| feature | default | description |
|---|---|---|
| `std` | yes | standard-library support |
| `parse` | yes | host-side `parse_transcript` (implies `std`; pulls in `spansy`) |
| `serde` | no | `Serialize`/`Deserialize` derives on the wire-format types (`SpanTable` and friends) |

With `--no-default-features` the crate is **`no_std` + `alloc`**: only
`validate` and the accessors, with `thiserror` as the sole required
dependency. That is the configuration intended for zkVM guests; the crate is
zkVM-agnostic (nothing SP1- or RISC Zero-specific). The only guest
allocations are the chunked-body decode buffer (pre-sized exactly once from
the table) and the JSON walk stack.

## Guarantees & limits

What validation enforces (each fact re-derived from the bytes, then
equality-checked against the table):

- exactly one request in `sent` and one response in `recv`, each consuming
  its whole buffer — no hidden trailing bytes or smuggled second message;
- strict CRLF line discipline and charset checks everywhere (token method
  and header names, printable-ASCII target, no CR/LF/NUL/DEL in header
  values, OWS-canonical value trimming);
- header records in bijection with the header lines on the wire — none
  hidden, missing, duplicated, or reordered; duplicate
  `Content-Length`/`Transfer-Encoding`/`Host` rejected;
- framing re-derived from the verified head (`Content-Length`, `chunked`,
  close-delimited responses, bodyless HEAD/1xx/204/304 responses), the
  chunk walk re-done byte-exactly, and the decoded length pinned so the
  guest pre-allocates exactly once;
- claimed-JSON bodies re-walked against the full RFC 8259 grammar in
  lockstep with the node table: kinds, spans, subtree sizes, UTF-8, and
  duplicate-key rejection (under escape-decoded comparison) all verified.

Non-goals (v1):

- HTTP/2, HTTP/3, and HTTP/1.0
- Multiple exchanges per buffer (keep-alive); the table format is versioned
  so this can come later
- `100 Continue` / `101 Switching Protocols` sequences
- obs-fold (folded headers) and bare-LF line endings — strict CRLF only
- Transfer codings other than `chunked`; non-identity `Content-Encoding`
  (capture transcripts with `Accept-Encoding: identity`; gzip bodies are
  provable only as opaque bytes)
- Multipart body parsing (provable as an opaque body)
- URI and cookie parsing (the request target is an opaque, charset-checked
  span)
- JSON escape decoding (accessors return raw string bytes; decoding may come
  later)
- JSON nesting depth > 127 (matches `serde_json`'s default recursion limit)

## Performance

`cargo bench -p transcript-verify` compares three costs over representative
fixtures, with throughput in transcript bytes: `host_parse`
(`parse_transcript`, off the proving path), `guest_validate` (`validate`
with the table precomputed — the in-VM cost proxy), and `spansy_parse` (a
full structural parse of the same bytes, the in-guest baseline this crate
replaces). Indicatively, on the ~27 KB `pokeapi_ditto` fixture the
validation walk runs ~6× faster than the spansy baseline — while also
verifying the entire JSON node tree, which the baseline does not even
parse — and, unlike a parser, it does so search-free and branch-light,
which is what matters once every cycle is proved.

## Testing

The crate is tested in five layers. (1) Unit tests pin every validator rule
and accessor (223 tests in-crate). (2) A checked-in fixture corpus — 15 live
captures from 8 real hosts plus 18 deterministic synthetics, exact wire
bytes, each pair documented in `fixtures/README.md` — forms the must-accept
set for the integration suites: a host→guest roundtrip suite
(`tests/roundtrip.rs`) and an adversarial suite (`tests/adversarial.rs`)
that mutates accepted tables and bytes and requires every mutation to be
rejected. (3) A differential suite (`tests/differential.rs`) re-parses every
JSON-claimed body with `serde_json` and walks the two trees in lockstep —
kinds, container counts, key sets, and leaf values (>1,400 nodes on the
pokeapi fixture alone) — with the few documented representation differences
(raw vs decoded escapes, serde's stock 127-deep recursion limit, lone
surrogates) asserted explicitly rather than skipped silently. (4) The
criterion benches above keep the performance claim honest. (5) The fixture
capture/generation scripts themselves verify framing invariants on every
byte they emit.
