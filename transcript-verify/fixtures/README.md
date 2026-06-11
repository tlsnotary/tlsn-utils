# transcript-verify test fixtures

A corpus of raw HTTP/1.1 transcript pairs used by the `transcript-verify` test
suites. Each fixture is two files of **exact wire bytes**:

- `<name>.sent.bin` — the HTTP/1.1 request (request line + headers + body,
  CRLF line endings, computed `Content-Length` on bodies);
- `<name>.recv.bin` — the raw response **verbatim** (status line + headers +
  body with its original framing; chunk-size lines, chunk CRLFs and trailers
  are preserved byte-for-byte).

Every pair in this directory tree is a **valid** HTTP/1.1 transcript — this is
the must-accept corpus. Invalid/adversarial inputs are constructed in test
code (`tests/adversarial.rs`), not checked in here.

| file | purpose |
|---|---|
| `capture.sh` | manifest-driven live capture of the real fixtures below (re-runnable, overwrites) |
| `gen_synthetic.sh` | deterministic generator for `synthetic/` (re-runs are byte-identical) |
| `verify_transcript.pl` | shared sanity checker both scripts run on every message |
| `*.sent.bin` / `*.recv.bin` | real captures (top level) |
| `synthetic/*.bin` | hand-crafted pairs (generated) |

## Real captures (15 pairs, 8 hosts)

Captured live on **2026-06-10 (13:08–13:15 UTC)** with
`openssl s_client -quiet -connect <host>:443 -servername <host>` reading the
request bytes from `<name>.sent.bin`. Every request sends `Host`,
`User-Agent: transcript-verify-fixtures/0.1`, `Accept: */*`,
`Accept-Encoding: identity`, `Connection: close` (plus `Content-Type` /
computed `Content-Length` on POSTs).

| fixture | host | request | status | response content-type | response framing captured | response body | sent/recv B | exercises |
|---|---|---|---|---|---|---|---|---|
| `pokeapi_ditto` | pokeapi.co | GET /api/v2/pokemon/ditto | 200 | application/json; charset=utf-8 | chunked (1 chunk, 25 564 B decoded) | JSON object, deeply nested | 159 / 26 793 | large chunked JSON response |
| `swapi_films` | swapi.info | GET /api/films | 200 | application/json | chunked (1 chunk, 20 524 B decoded) | JSON **array root**, `\r\n` escapes in strings | 148 / 21 487 | chunked JSON, array root |
| `httpbingo_get` | httpbingo.org | GET /get | 200 | application/json; charset=utf-8 | Content-Length: 672 | JSON object | 145 / 1 000 | CL-framed JSON; lowercase header names |
| `postman_post_json` | postman-echo.com | POST /post | 200 | application/json; charset=utf-8 | Content-Length: 462 | JSON object | 285 / 1 431 | **JSON request body** (request-side CL framing + JSON spans); 3 real `Set-Cookie` response headers |
| `postman_post_form` | postman-echo.com | POST /post | 200 | application/json; charset=utf-8 | Content-Length: 415 | JSON object | 257 / 1 382 | opaque `application/x-www-form-urlencoded` request body |
| `httpbingo_post_multipart` | httpbingo.org | POST /post | 200 | application/json; charset=utf-8 | Content-Length: 1513 | JSON object | 652 / 1 842 | opaque `multipart/form-data` request body (fixed boundary `X-FIXTURE-BOUNDARY-7d9f1c`, two text fields + 64-byte `note.txt` file part) |
| `httpbingo_xml` | httpbingo.org | GET /xml | 200 | application/xml | Content-Length: 522 | XML | 145 / 834 | opaque non-JSON text response |
| `httpbingo_png` | httpbingo.org | GET /image/png | 200 | image/png | chunked (2 chunks, 8 090 B decoded) | PNG, **binary non-UTF-8** | 151 / 8 423 | binary body + multi-chunk framing |
| `postman_204` | postman-echo.com | GET /status/204 | 204 | (none) | none (204) | (no body) | 155 / 884 | 204 no-body status rule |
| `example_html` | example.com | GET / | 200 | text/html | chunked (1 chunk, 559 B decoded) | HTML | 140 / 867 | HTML body; chunked framing |
| `example_head` | example.com | HEAD / | 200 | text/html | none (HEAD; **no CL, no TE**) | (no body) | 141 / 268 | HEAD response with neither framing header |
| `github_head` | api.github.com | HEAD /zen | 200 | text/plain;charset=utf-8 | none (HEAD; `Content-Length: 15` header, zero body bytes) | (no body) | 147 / 1 084 | **HEAD-with-Content-Length** rule |
| `jsonplaceholder_post` | jsonplaceholder.typicode.com | POST /posts | **201** | application/json; charset=utf-8 | Content-Length: 65 | JSON object | 254 / 1 289 | JSON request body → 201 + JSON response |
| `github_zen` | api.github.com | GET /zen | 200 | text/plain;charset=utf-8 | Content-Length: 15 | plain text | 146 / 1 100 | tiny text body, large header set, UA-required API |
| `icanhazip` | icanhazip.com | GET / | 200 | text/plain | Content-Length: 14 | plain text | 142 / 604 | minimal text/plain response |

Request bodies used by the POST fixtures:

- `postman_post_json`: `{"name":"ditto","level":42,"tags":["a","b"],"nested":{"x":1.5e3,"y":null,"z":true}}` (CL 83)
- `postman_post_form`: `name=ditto&level=42&note=hello%20world` (CL 38)
- `httpbingo_post_multipart`: two text fields + one 64-byte text file part, fixed boundary (CL 413)
- `jsonplaceholder_post`: `{"title":"foo","body":"bar","userId":1}` (CL 39)

### Substitutions & captured-reality notes (2026-06-10)

- **httpbin.org was down** (its AWS ELB answered `503 Service Temporarily
  Unavailable` on every request), so its endpoints were split across two
  equivalents, preserving the dimension coverage and adding an 8th host:
  **httpbingo.org** (Go httpbin clone on Fly.io — `/get`, `/post`, `/xml`,
  `/image/png`) and **postman-echo.com** (AWS — `/post`, `/status/204`).
- **example.com no longer serves Content-Length HTML**: it now sits behind
  Cloudflare and answers GET with `Transfer-Encoding: chunked` and HEAD with
  *neither* CL nor TE. The planned "HEAD with Content-Length" case moved to
  `github_head` (`Content-Length: 15`, zero body bytes); `example_head`
  remains as the rarer header-only HEAD shape.
- **No real endpoint returned close-delimited framing** (everything sent CL
  or chunked despite `Connection: close`), so that framing is covered by
  `synthetic/syn_close_delim`.
- Cloudflare-fronted hosts (pokeapi, swapi, example) buffered their chunked
  responses into a **single large chunk**; `httpbingo_png` has 2 chunks, and
  multi-chunk/trailer/split-token shapes are covered by the synthetics.
- postman-echo.com responses carry **three `Set-Cookie` headers** (duplicate
  names in the wild); icanhazip sends one.
- No response carried `Content-Encoding` (identity was honored everywhere);
  no retries were needed in the final run. All 15/15 verified on capture.
- `github_zen` returns a random aphorism per request — re-capturing changes
  its body bytes (all suites run offline over the checked-in bytes).

## Synthetic fixtures (18 pairs, `synthetic/`)

Deterministic, generated by `gen_synthetic.sh` (fixed `Date`, no randomness —
re-runs are byte-identical; verified via md5 across runs). Host is always
`synthetic.test`. All requests are GET with `Host` (plus `User-Agent`,
`Accept`, `Connection: close`) unless stated. All responses are
`HTTP/1.1 200 OK` with correct computed framing unless stated.

| fixture | request | response content-type | response framing | body | sent/recv B | exercises |
|---|---|---|---|---|---|---|
| `syn_close_delim` | GET /api/status | application/json | **close-delimited** (no CL, no TE; `Connection: close`) | `{"ok":true}` | 139 / 141 | close-delimited response framing |
| `syn_trailers` | GET /api/data | application/json | chunked, 2 chunks + **trailer `X-Checksum: abc123`** after the 0-chunk (declared via `Trailer:`) | `{"data":"chunked","n":7}` | 137 / 238 | chunked trailer section |
| `syn_chunked_request` | **POST /api/items, `Transfer-Encoding: chunked` request body** | application/json | Content-Length: 23 | request `{"a":[1,2,3]}` split `{"a` ∥ `":[1,2,3]}` (boundary **inside the `"a"` string token**); response `{"created":true,"id":7}` | 227 / 173 | chunked request-side framing, mid-token chunk split |
| `syn_cl_zero` | **POST /api/ping, `Content-Length: 0`**, no body | application/json | Content-Length: 13 | response `{"pong":true}` | 189 / 163 | CL=0 request rule |
| `syn_empty_reason` | GET /reason/empty | text/plain | Content-Length: 38; status line `HTTP/1.1 200 ` + CRLF (**SP, empty reason**) | text | 128 / 180 | empty reason phrase |
| `syn_no_reason` | GET /reason/none | text/plain | Content-Length: 39; status line `HTTP/1.1 200` + CRLF (**no SP, no reason**) | text | 127 / 180 | absent reason phrase |
| `syn_empty_header_value` | GET /headers/empty-value | application/json | Content-Length: 14; headers `X-Empty:` and `X-Empty-Ows: ` (empty values, without/with OWS) | `{"empty":true}` | 148 / 189 | empty header values (F1 position pinning) |
| `syn_dup_set_cookie` | GET /login | application/json | Content-Length: 14; **two `Set-Cookie` headers** | `{"login":"ok"}` | 134 / 248 | allowed duplicate headers |
| `syn_obs_text` | GET /headers/obs-text | text/plain | Content-Length: 49; header `X-Obs: café` with **raw 0xC3 0xA9 bytes** in the value | text | 132 / 207 | obs-text (0x80+) header value bytes |
| `syn_chunk_split_token` | GET /api/split | application/json | chunked, 3 chunks: `{"price":123` ∥ `45.678,"name":"alph` ∥ `abet"}` | `{"price":12345.678,"name":"alphabet"}` | 138 / 216 | chunk boundaries **inside a number lexeme and inside a string lexeme** |
| `syn_root_number` | GET /api/root/number | application/json | Content-Length: 2 | `42` | 144 / 151 | number as JSON root |
| `syn_root_string` | GET /api/root/string | application/json | Content-Length: 7 | `"hello"` | 144 / 156 | string as JSON root |
| `syn_root_bool` | GET /api/root/bool | application/json | Content-Length: 4 | `true` | 142 / 153 | bool as JSON root |
| `syn_root_null` | GET /api/root/null | application/json | Content-Length: 4 | `null` | 142 / 153 | null as JSON root |
| `syn_empty_containers` | GET /api/empty | application/json | Content-Length: 45 | `{"obj":{},"arr":[],"str":"","nested":[{},[]]}` | 138 / 195 | empty object/array/string, nested empties |
| `syn_unicode` | GET /api/unicode | application/json | Content-Length: 52 | `{"u":"Aé中😀 via 😀", "lone":"\uD800"}` — real 2/3/4-byte UTF-8 chars **plus the 6-char `\uXXXX` sequences as raw backslash-u text**, including a **lone surrogate `\uD800`** | 140 / 202 | unicode escapes, lone surrogate (accepted per RFC 8259 grammar), multi-byte UTF-8 |
| `syn_deep_127` | GET /api/deep/127 | application/json | Content-Length: 255 | `[`×127 `1` `]`×127 | 141 / 406 | nesting exactly at the 127 depth boundary |
| `syn_deep_128` | GET /api/deep/128 | application/json | Content-Length: 257 | `[`×128 `1` `]`×128 | 141 / 408 | nesting at the 128 depth limit |

In the `syn_unicode` row above the `😀 via …` body is rendered by Markdown;
the actual file bytes after `via ` are the twelve ASCII characters
`\` `u` `D` `8` `3` `D` `\` `u` `D` `E` `0` `0`, and `"lone"`'s value is the
six ASCII characters `\` `u` `D` `8` `0` `0` (verified by byte-level
assertions in the generator).

## Verification performed on every fixture

`verify_transcript.pl` (run automatically by both scripts) rejects a capture
unless:

- it is non-empty (responses ≤ 200 KB) and the head ends with CRLFCRLF, with
  no bare-LF lines;
- responses start with `HTTP/1.1 ` + 3-digit status (and match the expected
  status from the manifest); requests have a `<token> <target> HTTP/1.1` line;
- the response head has **no `Content-Encoding`** header (captures always
  request `Accept-Encoding: identity`);
- no duplicate `Content-Length`/`Transfer-Encoding`, never both at once;
- framing is internally consistent: `Content-Length` equals the actual body
  byte count and the message ends exactly at EOF; chunked bodies pass a full
  chunk walk (hex sizes, exact CRLFs, terminal `0`-chunk, optional trailers)
  that lands exactly on EOF; HEAD/1xx/204/304 responses carry zero body
  bytes; close-delimited responses (no CL/TE) are accepted and recorded.

`gen_synthetic.sh` additionally pins the tricky bytes with raw-byte
assertions (exact chunk-size lines, the `HTTP/1.1 200 ` / `HTTP/1.1 200`
status lines, `X-Empty:` / `X-Empty-Ows: `, the 0xC3 0xA9 obs-text bytes, the
backslash-u sequences, two `Set-Cookie` occurrences, 255/257-byte deep
bodies).

## Regenerating

```sh
fixtures/capture.sh                  # re-capture all real fixtures (live network)
fixtures/capture.sh github_zen ...   # re-capture selected fixtures
fixtures/gen_synthetic.sh            # regenerate synthetic/ (byte-identical)
CAPTURE_TIMEOUT=60 fixtures/capture.sh   # slower networks
```

`capture.sh` exits non-zero (after finishing the rest) if any fixture fails
its checks twice, and deletes that fixture's partial `recv` so no invalid
bytes can be checked in. Re-captures overwrite in place; response bytes from
live hosts naturally differ run to run (Date headers, dynamic bodies), so
only regenerate the real corpus deliberately.
