# transcript-verify

Sound validation of pre-parsed HTTP/JSON transcript span tables, designed for
zkVM guests. A transcript is a `sent` buffer holding one HTTP/1.1 request and a
`recv` buffer holding one response; the goal is to inspect it inside a zkVM,
where full parsing (branching, backtracking, dynamic allocation) is
prohibitively expensive.

## The advice pattern

Outside the VM, `parse_transcript` parses the transcript once and emits a flat,
serializable **span table** — byte ranges plus type tags (request-line parts,
header spans, body framing, a pre-order tree of typed JSON nodes) that act as a
parse witness. Inside the VM, `validate(sent, recv, &table)` re-derives every
fact from the bytes in a single linear cursor walk and equality-checks it
against the table, so the table never steers parsing and a forged or tampered
table can never validate: for fixed bytes, at most one semantically distinct
table is accepted. On success the guest gets a zero-copy `Transcript` accessor
(method, target, status, headers by name, JSON values by path) whose every
answer is backed by a verified span — no searching, no dynamic tree
construction, and exactly one pre-sized allocation for de-chunking.

## Non-goals (v1)

- HTTP/2, HTTP/3, and HTTP/1.0
- Multiple exchanges per buffer (keep-alive); the table format is versioned so
  this can come later
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
- JSON nesting depth > 128
