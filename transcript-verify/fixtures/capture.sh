#!/usr/bin/env bash
# fixtures/capture.sh — capture REAL raw HTTP/1.1 transcripts from public APIs.
#
# For each manifest entry this script:
#   1. builds the exact raw request bytes -> <name>.sent.bin
#      (printf with explicit \r\n line endings; POST bodies get a
#      Content-Length computed with `wc -c` over the exact body bytes)
#   2. pipes them through a raw TLS connection:
#        openssl s_client -quiet -connect <host>:443 -servername <host>
#      and stores the raw response bytes verbatim -> <name>.recv.bin
#      (status line + headers + body, chunked framing metadata preserved)
#   3. verifies the capture with verify_transcript.pl and prints a
#      one-line summary (name, status line, framing, sent/recv sizes).
#
# Per-fixture verification fails loudly when:
#   - recv does not start with "HTTP/1.1 " (or the status != expected)
#   - the response head contains a Content-Encoding header (we always send
#     `Accept-Encoding: identity`; a CDN that ignores it gets one retry,
#     after that swap the manifest entry for an alternative host)
#   - recv is empty or larger than 200 KB
#   - the framing is inconsistent (CL != body bytes, broken chunk walk,
#     HEAD/204 with body bytes, ...). Close-delimited responses (no CL,
#     no TE) are accepted and reported as such — see fixtures/README.md.
#
# Every request includes: Host, User-Agent (api.github.com requires one),
# Accept, Accept-Encoding: identity, Connection: close.
#
# Usage:
#   ./capture.sh                 # capture every fixture in the manifest
#   ./capture.sh name [name...]  # recapture selected fixtures only
#
# Re-runnable: overwrites existing .sent.bin/.recv.bin pairs. A fixture
# whose capture fails verification twice is reported, its partial recv is
# deleted, and the script exits non-zero after finishing the others.
#
# Note: macOS has no `timeout`; with_timeout falls back to a polling
# watchdog when neither `timeout` nor `gtimeout` is installed.

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY="$DIR/verify_transcript.pl"
CAPTURE_TIMEOUT="${CAPTURE_TIMEOUT:-30}"
UA="transcript-verify-fixtures/0.1"

[[ -f "$VERIFY" ]] || { echo "FATAL: missing $VERIFY" >&2; exit 1; }
command -v openssl >/dev/null 2>&1 || { echo "FATAL: openssl not found" >&2; exit 1; }

TMPD="$(mktemp -d "${TMPDIR:-/tmp}/txv-capture.XXXXXX")"
trap 'rm -rf "$TMPD"' EXIT

# ---------------------------------------------------------------------------
# Manifest: name|host|method|path|expected-status|content-type|body-kind
#
# body-kind is '' (no body) or a key understood by build_body() below.
# 2026-06-10: httpbin.org was returning 503 from its AWS ELB, so its
# endpoints are split across two equivalents that preserve the dimension
# coverage: httpbingo.org (Go clone, Fly.io) and postman-echo.com (AWS).
# ---------------------------------------------------------------------------
FIXTURES=(
  'pokeapi_ditto|pokeapi.co|GET|/api/v2/pokemon/ditto|200||'
  'swapi_films|swapi.info|GET|/api/films|200||'
  'httpbingo_get|httpbingo.org|GET|/get|200||'
  'postman_post_json|postman-echo.com|POST|/post|200|application/json|json_ditto'
  'postman_post_form|postman-echo.com|POST|/post|200|application/x-www-form-urlencoded|form'
  'httpbingo_post_multipart|httpbingo.org|POST|/post|200|multipart/form-data; boundary=X-FIXTURE-BOUNDARY-7d9f1c|multipart'
  'httpbingo_xml|httpbingo.org|GET|/xml|200||'
  'httpbingo_png|httpbingo.org|GET|/image/png|200||'
  'postman_204|postman-echo.com|GET|/status/204|204||'
  'example_html|example.com|GET|/|200||'
  'example_head|example.com|HEAD|/|200||'
  'github_head|api.github.com|HEAD|/zen|200||'
  'jsonplaceholder_post|jsonplaceholder.typicode.com|POST|/posts|201|application/json|json_post'
  'github_zen|api.github.com|GET|/zen|200||'
  'icanhazip|icanhazip.com|GET|/|200||'
)

# --- timeout shim (macOS ships no `timeout`; Homebrew coreutils = gtimeout) -
TIMEOUT_BIN=''
for t in timeout gtimeout; do
  if command -v "$t" >/dev/null 2>&1; then TIMEOUT_BIN="$t"; break; fi
done

# --- exact body bytes for POST fixtures -------------------------------------
build_body() {
  # build_body <kind> <outfile>
  local kind="$1" out="$2"
  case "$kind" in
    '')
      : > "$out"
      ;;
    json_ditto)
      printf '%s' '{"name":"ditto","level":42,"tags":["a","b"],"nested":{"x":1.5e3,"y":null,"z":true}}' > "$out"
      ;;
    form)
      printf '%s' 'name=ditto&level=42&note=hello%20world' > "$out"
      ;;
    json_post)
      printf '%s' '{"title":"foo","body":"bar","userId":1}' > "$out"
      ;;
    multipart)
      local b='X-FIXTURE-BOUNDARY-7d9f1c'
      local ff="$TMPD/file64"
      # deterministic 64-byte file part (4 x 16 bytes)
      printf '%s%s%s%s' '0123456789abcdef' '0123456789abcdef' '0123456789abcdef' '0123456789abcdef' > "$ff"
      local fsz
      fsz="$(wc -c < "$ff" | tr -d '[:space:]')"
      [[ "$fsz" == 64 ]] || { echo "FATAL: multipart file part is $fsz bytes, expected 64" >&2; exit 1; }
      {
        printf -- '--%s\r\n' "$b"
        printf 'Content-Disposition: form-data; name="field1"\r\n'
        printf '\r\n'
        printf 'value-one\r\n'
        printf -- '--%s\r\n' "$b"
        printf 'Content-Disposition: form-data; name="field2"\r\n'
        printf '\r\n'
        printf 'second value with spaces\r\n'
        printf -- '--%s\r\n' "$b"
        printf 'Content-Disposition: form-data; name="file"; filename="note.txt"\r\n'
        printf 'Content-Type: text/plain\r\n'
        printf '\r\n'
        cat "$ff"
        printf '\r\n'
        printf -- '--%s--\r\n' "$b"
      } > "$out"
      ;;
    *)
      echo "FATAL: unknown body kind '$kind'" >&2
      exit 1
      ;;
  esac
}

# --- raw request bytes -------------------------------------------------------
build_request() {
  # build_request <outfile> <method> <path> <host> <content-type> <bodyfile>
  local out="$1" method="$2" path="$3" host="$4" ctype="$5" bodyf="$6"
  local blen
  blen="$(wc -c < "$bodyf" | tr -d '[:space:]')"
  {
    printf '%s %s HTTP/1.1\r\n' "$method" "$path"
    printf 'Host: %s\r\n' "$host"
    printf 'User-Agent: %s\r\n' "$UA"
    printf 'Accept: */*\r\n'
    printf 'Accept-Encoding: identity\r\n'
    printf 'Connection: close\r\n'
    if [[ -n "$ctype" ]]; then
      printf 'Content-Type: %s\r\n' "$ctype"
    fi
    if [[ "$blen" != 0 || "$method" == POST ]]; then
      printf 'Content-Length: %s\r\n' "$blen"
    fi
    printf '\r\n'
    cat "$bodyf"
  } > "$out"
}

capture_one() {
  # capture_one <host> <sentfile> <recvfile>
  #
  # The stdin/stdout redirections MUST sit on the openssl command itself:
  # POSIX rewires an asynchronous command's stdin to /dev/null unless the
  # async command carries an explicit stdin redirection of its own.
  local host="$1" sent="$2" recv="$3"
  if [[ -n "$TIMEOUT_BIN" ]]; then
    "$TIMEOUT_BIN" "$CAPTURE_TIMEOUT" \
      openssl s_client -quiet -connect "$host:443" -servername "$host" \
      < "$sent" > "$recv" 2>/dev/null || true
  else
    openssl s_client -quiet -connect "$host:443" -servername "$host" \
      < "$sent" > "$recv" 2>/dev/null &
    local pid=$!
    (
      i=0
      while (( i < CAPTURE_TIMEOUT )); do
        kill -0 "$pid" 2>/dev/null || exit 0
        sleep 1
        i=$((i + 1))
      done
      kill -9 "$pid" 2>/dev/null || true
    ) &
    local wd=$!
    wait "$pid" 2>/dev/null || true
    wait "$wd" 2>/dev/null || true
  fi
}

# --- selection ----------------------------------------------------------------
SELECT=("$@")
want_fixture() {
  local name="$1" w
  (( ${#SELECT[@]} == 0 )) && return 0
  for w in ${SELECT[@]+"${SELECT[@]}"}; do
    [[ "$w" == "$name" ]] && return 0
  done
  return 1
}

# --- main loop -----------------------------------------------------------------
PASS=0
SUMMARY=()
FAILED=()

for entry in "${FIXTURES[@]}"; do
  IFS='|' read -r name host method path expect ctype bodykind <<< "$entry"
  want_fixture "$name" || continue

  sent="$DIR/$name.sent.bin"
  recv="$DIR/$name.recv.bin"
  bodyf="$TMPD/$name.body"

  build_body "$bodykind" "$bodyf"
  build_request "$sent" "$method" "$path" "$host" "$ctype" "$bodyf"

  # self-check the request bytes we just generated
  perl "$VERIFY" "$sent" request > /dev/null \
    || { echo "FATAL: generated request for '$name' is malformed" >&2; exit 1; }

  echo "capturing $name  ($method https://$host$path)"
  info=''
  for attempt in 1 2; do
    capture_one "$host" "$sent" "$recv"
    if info="$(perl "$VERIFY" "$recv" response "$method" "$expect" 2> "$TMPD/verr")"; then
      break
    fi
    info=''
    echo "  attempt $attempt failed: $(cat "$TMPD/verr")" >&2
    [[ "$attempt" == 1 ]] && sleep 2
  done

  if [[ -z "$info" ]]; then
    rm -f "$recv"
    FAILED+=("$name")
    SUMMARY+=("$(printf 'FAIL %-26s capture/verify failed twice; recv removed (see errors above)' "$name")")
    continue
  fi

  IFS=$'\t' read -r sline framing _bodylen <<< "$info"
  ssz="$(wc -c < "$sent" | tr -d '[:space:]')"
  rsz="$(wc -c < "$recv" | tr -d '[:space:]')"
  SUMMARY+=("$(printf 'OK   %-26s %-24s %-46s sent=%-6s recv=%s' "$name" "$sline" "$framing" "$ssz" "$rsz")")
  PASS=$((PASS + 1))
done

TOTAL=$((PASS + ${#FAILED[@]}))
echo
echo "== capture verification summary ($PASS/$TOTAL ok, $(date -u '+%Y-%m-%d %H:%M UTC')) =="
for line in ${SUMMARY[@]+"${SUMMARY[@]}"}; do
  echo "$line"
done

if (( ${#FAILED[@]} > 0 )); then
  echo
  echo "FAILED fixtures: ${FAILED[*]}" >&2
  echo "(httpbin.org alternatives: postman-echo.com, httpbingo.org — edit the manifest and re-run)" >&2
  exit 1
fi
echo "all captured fixtures verified."
