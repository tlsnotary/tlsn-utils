#!/usr/bin/env bash
# fixtures/gen_synthetic.sh — deterministic hand-crafted HTTP/1.1 transcript pairs.
#
# Generates fixtures/synthetic/<name>.sent.bin / <name>.recv.bin covering
# valid-but-rare shapes the public internet will not reliably serve:
# close-delimited responses, chunked trailers, chunked REQUEST bodies,
# Content-Length: 0, empty/missing reason phrases, empty header values,
# duplicate Set-Cookie, obs-text header bytes, chunk boundaries splitting
# JSON tokens, JSON scalar roots, empty containers, unicode escapes with a
# lone surrogate, and 127-deep JSON nesting (the maximum accepted depth).
#
# Every pair is a VALID HTTP/1.1 transcript (the must-accept corpus —
# invalid/adversarial cases belong in test code, not here). All sizes
# (Content-Length, chunk sizes) are computed with `wc -c`/printf %x over
# the exact body bytes — never hand-counted. Line endings are explicit
# \r\n via printf. Deterministic: re-running reproduces identical bytes
# (fixed Date header, no randomness), and existing files are overwritten.
#
# Each generated message is verified with verify_transcript.pl (starts
# with HTTP/1.1, CL == actual body bytes, full chunk-walk consistency),
# plus targeted byte-level assertions for the tricky fixtures.

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$DIR/synthetic"
VERIFY="$DIR/verify_transcript.pl"

[[ -f "$VERIFY" ]] || { echo "FATAL: missing $VERIFY" >&2; exit 1; }
mkdir -p "$OUT"

# Fixed values keep re-runs byte-identical.
HOST='synthetic.test'
DATE='Wed, 10 Jun 2026 00:00:00 GMT'
UA='transcript-verify-fixtures/0.1'

# --- helpers -----------------------------------------------------------------

slen() {
  # byte length of a string
  printf '%s' "$1" | wc -c | tr -d '[:space:]'
}

chunk() {
  # emit one HTTP chunk (lowercase hex size computed from the exact bytes)
  local n
  n="$(slen "$1")"
  printf '%x\r\n' "$n"
  printf '%s' "$1"
  printf '\r\n'
}

request_get() {
  # request_get <path> [accept]
  local path="$1" accept="${2:-*/*}"
  printf 'GET %s HTTP/1.1\r\n' "$path"
  printf 'Host: %s\r\n' "$HOST"
  printf 'User-Agent: %s\r\n' "$UA"
  printf 'Accept: %s\r\n' "$accept"
  printf 'Connection: close\r\n'
  printf '\r\n'
}

request_post_cl() {
  # request_post_cl <path> <content-type> <body-string>   (CL computed)
  local path="$1" ctype="$2" body="$3"
  printf 'POST %s HTTP/1.1\r\n' "$path"
  printf 'Host: %s\r\n' "$HOST"
  printf 'User-Agent: %s\r\n' "$UA"
  printf 'Accept: application/json\r\n'
  printf 'Content-Type: %s\r\n' "$ctype"
  printf 'Content-Length: %s\r\n' "$(slen "$body")"
  printf 'Connection: close\r\n'
  printf '\r\n'
  printf '%s' "$body"
}

cl_response() {
  # cl_response <content-type> <status-line> <body-string> [extra-header]...
  # Content-Length is computed from the exact body bytes.
  local ctype="$1" sline="$2" body="$3"
  shift 3
  printf '%s\r\n' "$sline"
  printf 'Date: %s\r\n' "$DATE"
  printf 'Server: synthetic/0.1\r\n'
  printf 'Content-Type: %s\r\n' "$ctype"
  printf 'Content-Length: %s\r\n' "$(slen "$body")"
  local h
  for h in "$@"; do
    printf '%s\r\n' "$h"
  done
  printf 'Connection: close\r\n'
  printf '\r\n'
  printf '%s' "$body"
}

chunked_response() {
  # chunked_response <content-type> <trailer-line-or-''> <chunk-string>...
  local ctype="$1" trailer="$2"
  shift 2
  printf 'HTTP/1.1 200 OK\r\n'
  printf 'Date: %s\r\n' "$DATE"
  printf 'Server: synthetic/0.1\r\n'
  printf 'Content-Type: %s\r\n' "$ctype"
  printf 'Transfer-Encoding: chunked\r\n'
  if [[ -n "$trailer" ]]; then
    printf 'Trailer: %s\r\n' "${trailer%%:*}"
  fi
  printf 'Connection: close\r\n'
  printf '\r\n'
  local c
  for c in "$@"; do
    chunk "$c"
  done
  printf '0\r\n'
  if [[ -n "$trailer" ]]; then
    printf '%s\r\n' "$trailer"
  fi
  printf '\r\n'
}

deep_body() {
  # deep_body <depth> -> [[[...1...]]] nested exactly <depth> levels
  local d="$1" i open='' close=''
  for (( i = 0; i < d; i++ )); do
    open+='['
    close+=']'
  done
  printf '%s1%s' "$open" "$close"
}

# --- byte-level assertion helpers --------------------------------------------

assert_contains() {
  # assert_contains <file> <raw-byte-needle>
  perl -e 'open my $f, "<:raw", $ARGV[0] or die "$ARGV[0]: $!"; local $/; my $b = <$f>;
           exit(index($b, $ARGV[1]) >= 0 ? 0 : 1)' "$1" "$2" \
    || { echo "ASSERT FAILED: $1 does not contain needle: $(printf '%q' "$2")" >&2; exit 1; }
}

assert_starts_with() {
  # assert_starts_with <file> <raw-byte-prefix>
  perl -e 'open my $f, "<:raw", $ARGV[0] or die "$ARGV[0]: $!"; local $/; my $b = <$f>;
           exit(index($b, $ARGV[1]) == 0 ? 0 : 1)' "$1" "$2" \
    || { echo "ASSERT FAILED: $1 does not start with: $(printf '%q' "$2")" >&2; exit 1; }
}

assert_count() {
  # assert_count <file> <raw-byte-needle> <expected-occurrences>
  local n
  n="$(perl -e 'open my $f, "<:raw", $ARGV[0] or die "$ARGV[0]: $!"; local $/; my $b = <$f>;
                my ($c, $p) = (0, 0);
                while (($p = index($b, $ARGV[1], $p)) >= 0) { $c++; $p++ }
                print $c' "$1" "$2")"
  [[ "$n" == "$3" ]] \
    || { echo "ASSERT FAILED: $1 contains $(printf '%q' "$2") $n times, expected $3" >&2; exit 1; }
}

# ==============================================================================
# Fixture generation
# ==============================================================================

# --- syn_close_delim: response framed only by connection close ----------------
# No Content-Length, no Transfer-Encoding; body runs to EOF.
request_get '/api/status' 'application/json' > "$OUT/syn_close_delim.sent.bin"
{
  printf 'HTTP/1.1 200 OK\r\n'
  printf 'Date: %s\r\n' "$DATE"
  printf 'Server: synthetic/0.1\r\n'
  printf 'Content-Type: application/json\r\n'
  printf 'Connection: close\r\n'
  printf '\r\n'
  printf '%s' '{"ok":true}'
} > "$OUT/syn_close_delim.recv.bin"

# --- syn_trailers: chunked response with a trailer after the 0-chunk ----------
request_get '/api/data' 'application/json' > "$OUT/syn_trailers.sent.bin"
chunked_response 'application/json' 'X-Checksum: abc123' \
  '{"data":"chunk' 'ed","n":7}' > "$OUT/syn_trailers.recv.bin"
assert_contains "$OUT/syn_trailers.recv.bin" $'e\r\n{"data":"chunk\r\na\r\ned","n":7}\r\n0\r\nX-Checksum: abc123\r\n\r\n'

# --- syn_chunked_request: POST with a chunked request body --------------------
# Chunk boundary falls mid-token, inside the string lexeme "a" of {"a":[1,2,3]}.
{
  printf 'POST /api/items HTTP/1.1\r\n'
  printf 'Host: %s\r\n' "$HOST"
  printf 'User-Agent: %s\r\n' "$UA"
  printf 'Accept: application/json\r\n'
  printf 'Content-Type: application/json\r\n'
  printf 'Transfer-Encoding: chunked\r\n'
  printf 'Connection: close\r\n'
  printf '\r\n'
  chunk '{"a'
  chunk '":[1,2,3]}'
  printf '0\r\n\r\n'
} > "$OUT/syn_chunked_request.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' '{"created":true,"id":7}' \
  > "$OUT/syn_chunked_request.recv.bin"
assert_contains "$OUT/syn_chunked_request.sent.bin" $'3\r\n{"a\r\na\r\n":[1,2,3]}\r\n0\r\n\r\n'

# --- syn_cl_zero: POST with Content-Length: 0 (no body) ------------------------
request_post_cl '/api/ping' 'application/json' '' > "$OUT/syn_cl_zero.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' '{"pong":true}' \
  > "$OUT/syn_cl_zero.recv.bin"
assert_contains "$OUT/syn_cl_zero.sent.bin" $'Content-Length: 0\r\n'

# --- syn_empty_reason: status line "HTTP/1.1 200 \r\n" (SP, empty reason) -----
request_get '/reason/empty' > "$OUT/syn_empty_reason.sent.bin"
cl_response 'text/plain' 'HTTP/1.1 200 ' 'status line has an empty reason phrase' \
  > "$OUT/syn_empty_reason.recv.bin"
assert_starts_with "$OUT/syn_empty_reason.recv.bin" $'HTTP/1.1 200 \r\nDate:'

# --- syn_no_reason: status line "HTTP/1.1 200\r\n" (no SP, no reason) ---------
request_get '/reason/none' > "$OUT/syn_no_reason.sent.bin"
cl_response 'text/plain' 'HTTP/1.1 200' 'status line has no reason phrase at all' \
  > "$OUT/syn_no_reason.recv.bin"
assert_starts_with "$OUT/syn_no_reason.recv.bin" $'HTTP/1.1 200\r\nDate:'

# --- syn_empty_header_value: empty value, with and without OWS ----------------
request_get '/headers/empty-value' 'application/json' > "$OUT/syn_empty_header_value.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' '{"empty":true}' \
  'X-Empty:' \
  'X-Empty-Ows: ' \
  > "$OUT/syn_empty_header_value.recv.bin"
assert_contains "$OUT/syn_empty_header_value.recv.bin" $'\r\nX-Empty:\r\n'
assert_contains "$OUT/syn_empty_header_value.recv.bin" $'\r\nX-Empty-Ows: \r\n'

# --- syn_dup_set_cookie: two Set-Cookie headers --------------------------------
request_get '/login' 'application/json' > "$OUT/syn_dup_set_cookie.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' '{"login":"ok"}' \
  'Set-Cookie: session=abc123; Path=/; HttpOnly' \
  'Set-Cookie: theme=dark; Max-Age=3600' \
  > "$OUT/syn_dup_set_cookie.recv.bin"
assert_count "$OUT/syn_dup_set_cookie.recv.bin" 'Set-Cookie:' 2

# --- syn_obs_text: raw 0x80+ bytes (UTF-8 é = 0xC3 0xA9) in a header value ----
request_get '/headers/obs-text' > "$OUT/syn_obs_text.sent.bin"
cl_response 'text/plain' 'HTTP/1.1 200 OK' 'the X-Obs header value carries raw obs-text bytes' \
  $'X-Obs: caf\xc3\xa9' \
  > "$OUT/syn_obs_text.recv.bin"
assert_contains "$OUT/syn_obs_text.recv.bin" $'\r\nX-Obs: caf\xc3\xa9\r\n'

# --- syn_chunk_split_token: chunk boundaries inside JSON lexemes ---------------
# boundary 1 splits the number 12345.678 (after "123");
# boundary 2 splits the string "alphabet" (after "alph").
request_get '/api/split' 'application/json' > "$OUT/syn_chunk_split_token.sent.bin"
chunked_response 'application/json' '' \
  '{"price":123' '45.678,"name":"alph' 'abet"}' \
  > "$OUT/syn_chunk_split_token.recv.bin"
assert_contains "$OUT/syn_chunk_split_token.recv.bin" \
  $'c\r\n{"price":123\r\n13\r\n45.678,"name":"alph\r\n6\r\nabet"}\r\n0\r\n\r\n'

# --- syn_root_*: every JSON scalar shape as a root value -----------------------
request_get '/api/root/number' 'application/json' > "$OUT/syn_root_number.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' '42' > "$OUT/syn_root_number.recv.bin"

request_get '/api/root/string' 'application/json' > "$OUT/syn_root_string.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' '"hello"' > "$OUT/syn_root_string.recv.bin"

request_get '/api/root/bool' 'application/json' > "$OUT/syn_root_bool.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' 'true' > "$OUT/syn_root_bool.recv.bin"

request_get '/api/root/null' 'application/json' > "$OUT/syn_root_null.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' 'null' > "$OUT/syn_root_null.recv.bin"

# --- syn_empty_containers: {}, [], "" and nested empties -----------------------
request_get '/api/empty' 'application/json' > "$OUT/syn_empty_containers.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' \
  '{"obj":{},"arr":[],"str":"","nested":[{},[]]}' \
  > "$OUT/syn_empty_containers.recv.bin"

# --- syn_unicode: real multi-byte UTF-8 + \uXXXX escapes + lone surrogate -----
# The body bytes contain the six-char escape sequences \uD83D \uDE00 (a valid
# surrogate pair, as raw backslash-u text) and a LONE \uD800 escape, plus real
# 2-byte (é), 3-byte (中) and 4-byte (😀) UTF-8 characters.
# Composed via printf, with every backslash supplied as a %s argument and the
# real UTF-8 characters as \xNN byte escapes, so the generated body stays
# byte-exact no matter how an editor/tool normalizes this script's encoding:
#   {"u":"A é 中 😀 via \ uD83D \ uDE00", "lone":"\ uD800"}   (spaces not in output)
UNICODE_BODY="$(printf '{"u":"A\xc3\xa9\xe4\xb8\xad\xf0\x9f\x98\x80 via %sD83D%sDE00", "lone":"%sD800"}' '\u' '\u' '\u')"
SURROGATE_PAIR_TEXT="$(printf '%sD83D%sDE00' '\u' '\u')"    # literal 12-char backslash-u pair
LONE_SURROGATE_TEXT="$(printf '"lone":"%sD800"' '\u')"      # literal lone backslash-u escape
request_get '/api/unicode' 'application/json' > "$OUT/syn_unicode.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' "$UNICODE_BODY" \
  > "$OUT/syn_unicode.recv.bin"
assert_contains "$OUT/syn_unicode.recv.bin" "$SURROGATE_PAIR_TEXT"   # surrogate pair as raw text
assert_contains "$OUT/syn_unicode.recv.bin" "$LONE_SURROGATE_TEXT"   # lone surrogate escape
assert_contains "$OUT/syn_unicode.recv.bin" $'\xc3\xa9'          # é  (2-byte UTF-8)
assert_contains "$OUT/syn_unicode.recv.bin" $'\xe4\xb8\xad'      # 中 (3-byte UTF-8)
assert_contains "$OUT/syn_unicode.recv.bin" $'\xf0\x9f\x98\x80'  # 😀 (4-byte UTF-8)

# --- syn_deep_127: nesting exactly at the maximum accepted depth -------------
# 127 matches serde_json's default recursion limit (the validator's cap); a
# 128-deep body is a REJECT case and is covered by the unit/adversarial tests.
DEEP_127="$(deep_body 127)"
[[ "$(slen "$DEEP_127")" == 255 ]] || { echo "ASSERT FAILED: deep_127 body length" >&2; exit 1; }

request_get '/api/deep/127' 'application/json' > "$OUT/syn_deep_127.sent.bin"
cl_response 'application/json' 'HTTP/1.1 200 OK' "$DEEP_127" > "$OUT/syn_deep_127.recv.bin"

# ==============================================================================
# Verification: every pair must be a well-formed HTTP/1.1 transcript
# ==============================================================================

method_of() {
  LC_ALL=C tr -d '\r' < "$1" | head -n 1 | cut -d' ' -f1
}

echo "== synthetic fixture verification ($(date -u '+%Y-%m-%d %H:%M UTC')) =="
COUNT=0
for sent in "$OUT"/*.sent.bin; do
  name="$(basename "$sent" .sent.bin)"
  recv="$OUT/$name.recv.bin"
  [[ -f "$recv" ]] || { echo "FAIL $name: missing recv pair" >&2; exit 1; }

  perl "$VERIFY" "$sent" request > /dev/null
  info="$(perl "$VERIFY" "$recv" response "$(method_of "$sent")")"
  IFS=$'\t' read -r sline framing _bodylen <<< "$info"

  ssz="$(wc -c < "$sent" | tr -d '[:space:]')"
  rsz="$(wc -c < "$recv" | tr -d '[:space:]')"
  printf 'OK   %-26s %-16s %-42s sent=%-5s recv=%s\n' "$name" "$sline" "$framing" "$ssz" "$rsz"
  COUNT=$((COUNT + 1))
done
echo "all $COUNT synthetic pairs verified."
