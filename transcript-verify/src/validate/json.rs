//! Lockstep JSON validation and RFC 8259 lexical scanners (rule group F).
//!
//! The scanners are shared with the host's span-emitting JSON pass
//! (`host/json.rs`), so guest and host agree on the lexical grammar by
//! construction.

use alloc::vec::Vec;

use crate::{
    error::Error,
    spans::{JsonKind, JsonNode},
};

/// Maximum container nesting depth (rule F5).
///
/// Cap semantics: the depth of a position is the number of `Object`/`Array`
/// frames open at once. A document whose deepest position has exactly 127
/// open containers validates; opening a 128th is an error. (Matches
/// `serde_json`'s default recursion limit, so any document we accept stays
/// re-parseable downstream — anything deeper is toxic for those consumers.)
///
/// Shared with the host span emitter (`host/json.rs`) so the two agree on
/// the cap by construction.
pub(crate) const MAX_JSON_DEPTH: usize = 127;

/// Shorthand for an [`Error::Json`] at a decoded-body position.
///
/// Positions are always `<= content.len() <= 2^30` when called from
/// [`validate_json`]; the saturation only guards host-side misuse on
/// gigantic buffers. Shared with the host span emitter (`host/json.rs`).
pub(crate) fn json_err(at: usize, reason: &'static str) -> Error {
    Error::Json {
        at: u32::try_from(at).unwrap_or(u32::MAX),
        reason,
    }
}

/// JSON whitespace (RFC 8259 `ws`): space, HTAB, LF, CR.
fn is_jws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// Advances the cursor over any run of JSON whitespace. Shared with the
/// host span emitter (`host/json.rs`).
pub(crate) fn skip_jws(src: &[u8], mut p: usize) -> usize {
    while let Some(&b) = src.get(p) {
        if !is_jws(b) {
            break;
        }
        p += 1;
    }
    p
}

/// One open container during the walk.
struct Frame {
    /// Index of the container's node in the node table.
    node_idx: usize,
    /// `true` for an object frame, `false` for an array frame.
    is_object: bool,
    /// Decoded member keys (objects only), each with the source position of
    /// its content start for duplicate reporting.
    keys: Vec<(Vec<u32>, usize)>,
}

/// Verifies that `nodes` is THE pre-order parse of `content` (rule
/// group F).
///
/// `content` is a decoded body; all node spans are in decoded-body
/// coordinates. Single forward pass with an explicit stack (depth ≤ 127,
/// see [`MAX_JSON_DEPTH`]) and a lockstep node cursor:
///
/// - one linear UTF-8 check of the whole body (F0);
/// - the root span must equal the whitespace-trimmed extent — no hidden
///   prefix/suffix or second value (F1, F10);
/// - the byte at the cursor determines the only admissible node kind (F2);
/// - container interiors are exactly tiled by their children and separators,
///   with whitespace-only gaps (F3/F4);
/// - strings/keys/numbers/literals are checked lexically via the scanners below
///   (F6/F7/F8);
/// - duplicate object keys are rejected, comparing DECODED keys (F9).
///
/// Returns `Ok(())` iff every node verifies and the body is fully covered.
pub(crate) fn validate_json(content: &[u8], nodes: &[JsonNode]) -> Result<(), Error> {
    if nodes.is_empty() {
        return Err(Error::Table {
            reason: "empty JSON node table",
        });
    }
    // Every node owns at least one body byte (rule A). The driver
    // pre-checks this too; kept here for defense in depth.
    if nodes.len() > content.len() {
        return Err(Error::Table {
            reason: "more JSON nodes than body bytes",
        });
    }
    // F0: one linear UTF-8 check of the whole body.
    if let Err(e) = core::str::from_utf8(content) {
        return Err(json_err(e.valid_up_to(), "body is not valid UTF-8"));
    }

    let mut stack: Vec<Frame> = Vec::new();
    // F1: the root starts at the first non-whitespace byte. (Empty content
    // cannot reach here: `nodes.len() >= 1 > content.len()` was rejected.)
    let mut p = skip_jws(content, 0);
    let mut k = 0usize;

    'value: loop {
        // === a value must start at `p`, described by node `k` ===
        let Some(&b) = content.get(p) else {
            return Err(json_err(p, "expected a JSON value"));
        };
        // F2: the byte determines the only admissible kind.
        let kind = match b {
            b'{' => JsonKind::Object,
            b'[' => JsonKind::Array,
            b'"' => JsonKind::String,
            b't' | b'f' => JsonKind::Bool,
            b'n' => JsonKind::Null,
            b'-' | b'0'..=b'9' => JsonKind::Number,
            _ => return Err(json_err(p, "unexpected byte at value position")),
        };
        let Some(node) = nodes.get(k) else {
            return Err(json_err(p, "node table exhausted"));
        };
        if node.kind != kind {
            return Err(json_err(p, "node kind mismatch"));
        }
        // String spans cover the content between the quotes; `p + 1` cannot
        // overflow because `p < content.len()`.
        let claimed_start = if kind == JsonKind::String { p + 1 } else { p };
        if node.start as usize != claimed_start {
            return Err(json_err(p, "node start mismatch"));
        }
        let node_idx = k;
        k += 1;

        if kind == JsonKind::Object || kind == JsonKind::Array {
            // F5: depth cap. With 127 frames already open, a 128th
            // container is rejected; depth 127 itself validates.
            if stack.len() >= MAX_JSON_DEPTH {
                return Err(Error::Table {
                    reason: "JSON nesting depth exceeds 127",
                });
            }
            let is_object = kind == JsonKind::Object;
            stack.push(Frame {
                node_idx,
                is_object,
                keys: Vec::new(),
            });
            p = skip_jws(content, p + 1);
            let closer = if is_object { b'}' } else { b']' };
            if content.get(p) == Some(&closer) {
                // Empty container: close immediately, then cascade below.
                p += 1;
                close_frame(nodes, &mut stack, p, k)?;
            } else if is_object {
                // F3: a non-empty object interior must open with a key.
                (p, k) = member_key(content, nodes, &mut stack, p, k)?;
                continue 'value;
            } else {
                // Array: the element value starts here.
                continue 'value;
            }
        } else {
            // Scalar kinds: derive the end lexically (F6/F7/F8) and
            // equality-check the claimed end. `end` is one past the lexeme
            // (strings: one past the closing quote).
            let end = match b {
                b'"' => scan_string(content, p)?,
                b'-' | b'0'..=b'9' => scan_number(content, p)?,
                _ => {
                    let lit: &[u8] = match b {
                        b't' => b"true",
                        b'f' => b"false",
                        _ => b"null",
                    };
                    let end = p
                        .checked_add(lit.len())
                        .ok_or(json_err(p, "invalid literal"))?;
                    if content.get(p..end) != Some(lit) {
                        return Err(json_err(p, "invalid literal"));
                    }
                    end
                }
            };
            // String spans exclude the closing quote; `end >= p + 2` for
            // strings, so the subtraction cannot underflow.
            let claimed_end = if b == b'"' { end - 1 } else { end };
            if node.end as usize != claimed_end {
                return Err(json_err(p, "node end mismatch"));
            }
            if node.size != 1 {
                return Err(json_err(p, "leaf node size mismatch"));
            }
            p = end;
        }

        // === after a completed value: separator / closer cascade (F3/F4) ===
        loop {
            let Some(frame) = stack.last() else {
                // The root value is complete. F1/F10: only whitespace may
                // follow, and the node table must be exactly consumed.
                p = skip_jws(content, p);
                if p != content.len() {
                    return Err(json_err(p, "data after root value"));
                }
                if k != nodes.len() {
                    return Err(json_err(p, "table has extra JSON nodes"));
                }
                return Ok(());
            };
            let is_object = frame.is_object;
            p = skip_jws(content, p);
            let Some(&b) = content.get(p) else {
                return Err(json_err(p, "unterminated container"));
            };
            if b == b',' {
                p = skip_jws(content, p + 1);
                if is_object {
                    (p, k) = member_key(content, nodes, &mut stack, p, k)?;
                }
                // A value must follow the comma (so `[1,]` and `[1,,2]`
                // fail at the value position).
                continue 'value;
            }
            let closer = if is_object { b'}' } else { b']' };
            if b == closer {
                p += 1;
                close_frame(nodes, &mut stack, p, k)?;
                // Cascade: the parent may close at this position too.
                continue;
            }
            return Err(json_err(p, "expected comma or closing bracket"));
        }
    }
}

/// Verifies one object-member key at `p` (the opening quote) against node
/// `k`, records its decoded form in the top frame for duplicate detection,
/// and consumes the following colon (F3/F9).
///
/// Returns the cursor at the member value's first byte and the next node
/// index. The caller must hold an open object frame on `stack`.
fn member_key(
    content: &[u8],
    nodes: &[JsonNode],
    stack: &mut [Frame],
    p: usize,
    k: usize,
) -> Result<(usize, usize), Error> {
    if content.get(p) != Some(&b'"') {
        return Err(json_err(p, "expected object key"));
    }
    let Some(node) = nodes.get(k) else {
        return Err(json_err(p, "node table exhausted"));
    };
    if node.kind != JsonKind::Key {
        return Err(json_err(p, "node kind mismatch"));
    }
    if node.start as usize != p + 1 {
        return Err(json_err(p, "node start mismatch"));
    }
    let end = scan_string(content, p)?;
    if node.end as usize != end - 1 {
        return Err(json_err(p, "node end mismatch"));
    }
    if node.size != 1 {
        return Err(json_err(p, "leaf node size mismatch"));
    }
    // The content span was derived from the scan, so it is in bounds; the
    // fallback cannot trigger.
    let raw = content.get(p + 1..end - 1).unwrap_or(&[]);
    let Some(frame) = stack.last_mut() else {
        // Unreachable by construction (both call sites hold an open object
        // frame); fail closed rather than skip duplicate tracking.
        return Err(Error::Table {
            reason: "internal: member key without open object",
        });
    };
    frame.keys.push((decode_key(raw), p + 1));
    let mut q = skip_jws(content, end);
    if content.get(q) != Some(&b':') {
        return Err(json_err(q, "expected colon"));
    }
    q = skip_jws(content, q + 1);
    Ok((q, k + 1))
}

/// Pops the top frame at a container close and finishes its checks: `p` is
/// the cursor one past the closer, `k` the next unconsumed node index.
///
/// Equality-checks the container node's `end` against the cursor and its
/// `size` against the nodes actually consumed in the subtree (F3/F4), then
/// rejects duplicate decoded keys via sort + adjacent compare (F9).
fn close_frame(
    nodes: &[JsonNode],
    stack: &mut Vec<Frame>,
    p: usize,
    k: usize,
) -> Result<(), Error> {
    let Some(mut frame) = stack.pop() else {
        // Unreachable by construction (callers close only open frames);
        // fail closed.
        return Err(Error::Table {
            reason: "internal: close without open container",
        });
    };
    let Some(node) = nodes.get(frame.node_idx) else {
        // Unreachable: the index was in bounds when the frame was pushed.
        return Err(Error::Table {
            reason: "internal: frame node out of range",
        });
    };
    // `p >= 1` here (the closer was just consumed); report at the closer.
    let at = p.saturating_sub(1);
    if node.end as usize != p {
        return Err(json_err(at, "container end mismatch"));
    }
    if node.size as usize != k.saturating_sub(frame.node_idx) {
        return Err(json_err(at, "container size mismatch"));
    }
    if frame.keys.len() > 1 {
        // Sorting orders equal keys adjacently; the tuple's second field
        // (content position) breaks ties, so the reported position is the
        // LATER duplicate in document order.
        frame.keys.sort_unstable();
        for pair in frame.keys.windows(2) {
            if let [(a, _), (b, pos)] = pair
                && a == b
            {
                return Err(json_err(*pos, "duplicate object key"));
            }
        }
    }
    Ok(())
}

/// Decodes a scan-validated key's content bytes to a `u32` code-unit
/// sequence for duplicate comparison ONLY (F9). Never exposed.
///
/// Rules: raw UTF-8 characters decode to their code points; simple escapes
/// to their code points; `\uXXXX` to its 16-bit value, EXCEPT that an
/// adjacent high+low surrogate escape pair (`D800..=DBFF` then
/// `DC00..=DFFF`) combines to the supplementary code point. Lone surrogate
/// escapes keep their raw value — sound because F0-validated raw text never
/// produces surrogate code points, so a lone surrogate can never collide
/// with raw text.
///
/// `raw` was validated by [`scan_string`] over UTF-8-checked content, so
/// every escape is complete and the bytes are valid UTF-8; the impossible
/// branches below bail out early, which at worst causes a spurious
/// duplicate REJECTION (fail closed), never an acceptance.
fn decode_key(raw: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(&b) = raw.get(i) {
        if b == b'\\' {
            match raw.get(i + 1) {
                Some(b'u') => {
                    let Some(hi) = hex4(raw, i + 2) else {
                        break;
                    };
                    i += 6;
                    if (0xD800..=0xDBFF).contains(&hi)
                        && raw.get(i) == Some(&b'\\')
                        && raw.get(i + 1) == Some(&b'u')
                        && let Some(lo) = hex4(raw, i + 2)
                        && (0xDC00..=0xDFFF).contains(&lo)
                    {
                        // Surrogate pair: combine. `hi - 0xD800 <= 0x3FF`,
                        // so the shift and adds stay far below u32::MAX.
                        out.push(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00));
                        i += 6;
                    } else {
                        out.push(hi);
                    }
                }
                Some(&e) => {
                    out.push(match e {
                        b'b' => 0x08,
                        b'f' => 0x0C,
                        b'n' => 0x0A,
                        b'r' => 0x0D,
                        b't' => 0x09,
                        // `"`, `\`, `/` map to themselves.
                        other => u32::from(other),
                    });
                    i += 2;
                }
                None => break,
            }
        } else if b < 0x80 {
            out.push(u32::from(b));
            i += 1;
        } else {
            // Multi-byte UTF-8 character. `i` is always a character
            // boundary: escapes are ASCII and characters advance whole.
            let Ok(s) = core::str::from_utf8(raw) else {
                break;
            };
            let Some(c) = s.get(i..).and_then(|t| t.chars().next()) else {
                break;
            };
            out.push(c as u32);
            i += c.len_utf8();
        }
    }
    out
}

/// Parses exactly 4 HEXDIGs of `raw` at `at`, case-insensitive.
fn hex4(raw: &[u8], at: usize) -> Option<u32> {
    let s = raw.get(at..at.checked_add(4)?)?;
    let mut v = 0u32;
    for &b in s {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        // `v <= 0xFFF` before the last round: no overflow possible.
        v = v * 16 + u32::from(d);
    }
    Some(v)
}

/// Scans one RFC 8259 string starting at `start` and returns the index one
/// past the closing quote.
///
/// Contract:
///
/// - Requires `src[start] == b'"'` (the opening quote); errors if `start` is
///   out of bounds or the byte is not a quote.
/// - Content bytes may be anything except `"`, `\`, and raw control bytes (`<
///   0x20`, rejected — stricter than spansy's grammar). Escapes are `\"` `\\`
///   `\/` `\b` `\f` `\n` `\r` `\t` and `\u` followed by exactly 4 HEXDIG
///   (case-insensitive). Lone surrogates are accepted: this is a grammar-level
///   check and strings are never decoded here. Bytes `>= 0x80` pass through —
///   UTF-8 validity is checked once for the whole body by [`validate_json`].
/// - The string ends at the first unescaped `"`, which forces the boundary: no
///   other end position can validate.
///
/// On success the content span (quotes excluded) is `start + 1..ret - 1`.
/// Errors with [`Error::Json`] on a bad escape, a raw control byte, or an
/// unterminated string.
pub(crate) fn scan_string(src: &[u8], start: usize) -> Result<usize, Error> {
    if src.get(start) != Some(&b'"') {
        return Err(json_err(start, "expected opening quote"));
    }
    let mut p = start + 1;
    loop {
        let Some(&b) = src.get(p) else {
            return Err(json_err(p, "unterminated string"));
        };
        match b {
            b'"' => return Ok(p + 1),
            b'\\' => match src.get(p + 1) {
                Some(b'"') | Some(b'\\') | Some(b'/') | Some(b'b') | Some(b'f') | Some(b'n')
                | Some(b'r') | Some(b't') => p += 2,
                Some(b'u') => {
                    for off in 0..4usize {
                        // `p + 2 + off` cannot overflow: `p < src.len()`.
                        if !src.get(p + 2 + off).is_some_and(|h| h.is_ascii_hexdigit()) {
                            return Err(json_err(p, "invalid unicode escape"));
                        }
                    }
                    p += 6;
                }
                _ => return Err(json_err(p, "invalid escape")),
            },
            0x00..=0x1F => return Err(json_err(p, "control byte in string")),
            _ => p += 1,
        }
    }
}

/// Scans one RFC 8259 number starting at `start` and returns the index one
/// past the end of the lexeme.
///
/// Contract:
///
/// - Requires `src[start]` to be `-` or a DIGIT.
/// - Matches the longest prefix of `src[start..]` conforming to the full RFC
///   8259 grammar `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` — no leading
///   `+`, no leading zeros, and the fraction/exponent parts require at least
///   one digit (so for `1.x` or `1e+` the lexeme is just `1`).
/// - Errors with [`Error::Json`] iff no nonempty prefix matches (e.g. a lone
///   `-`).
///
/// Maximality is NOT enforced here: callers ([`validate_json`]'s tiling
/// rule and the host emitter) must require the byte at the returned index
/// to be JSON whitespace, `,`, `}`, `]`, or end of input, which pins the
/// lexeme end (rule F7).
pub(crate) fn scan_number(src: &[u8], start: usize) -> Result<usize, Error> {
    let mut p = start;
    if src.get(p) == Some(&b'-') {
        p += 1;
    }
    // Integer part: `0` or `[1-9][0-9]*` — required.
    match src.get(p) {
        Some(b'0') => p += 1,
        Some(b'1'..=b'9') => {
            p += 1;
            while src.get(p).is_some_and(u8::is_ascii_digit) {
                p += 1;
            }
        }
        _ => return Err(json_err(p, "invalid number")),
    }
    // Optional fraction: `.` followed by at least one digit; otherwise the
    // lexeme ends before the dot.
    if src.get(p) == Some(&b'.') {
        let mut q = p + 1;
        while src.get(q).is_some_and(u8::is_ascii_digit) {
            q += 1;
        }
        if q > p + 1 {
            p = q;
        }
    }
    // Optional exponent: `e|E`, optional sign, at least one digit;
    // otherwise the lexeme ends before the `e`.
    if matches!(src.get(p), Some(b'e') | Some(b'E')) {
        let mut q = p + 1;
        if matches!(src.get(q), Some(b'+') | Some(b'-')) {
            q += 1;
        }
        let digits_start = q;
        while src.get(q).is_some_and(u8::is_ascii_digit) {
            q += 1;
        }
        if q > digits_start {
            p = q;
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use super::*;

    fn node(kind: JsonKind, start: usize, end: usize, size: usize) -> JsonNode {
        JsonNode {
            kind,
            start: start as u32,
            end: end as u32,
            size: size as u32,
        }
    }

    fn assert_json_err<T: core::fmt::Debug>(result: Result<T, Error>, want: &str) {
        match result {
            Err(Error::Json { reason, .. }) => assert_eq!(reason, want),
            other => panic!("expected Json {{ {want:?} }}, got {other:?}"),
        }
    }

    fn assert_json_err_at<T: core::fmt::Debug>(result: Result<T, Error>, at: usize, want: &str) {
        match result {
            Err(Error::Json { at: got_at, reason }) => {
                assert_eq!((got_at, reason), (at as u32, want));
            }
            other => panic!("expected Json {{ at: {at}, {want:?} }}, got {other:?}"),
        }
    }

    fn assert_table_err<T: core::fmt::Debug>(result: Result<T, Error>, want: &str) {
        match result {
            Err(Error::Table { reason }) => assert_eq!(reason, want),
            other => panic!("expected Table {{ {want:?} }}, got {other:?}"),
        }
    }

    // === scan_string ===

    #[test]
    fn string_simple_and_empty() {
        assert_eq!(scan_string(b"\"\"", 0), Ok(2));
        assert_eq!(scan_string(b"\"abc\"", 0), Ok(5));
        // Not anchored at 0.
        assert_eq!(scan_string(b"xx\"a\"yy", 2), Ok(5));
        // Ends at the FIRST unescaped quote.
        assert_eq!(scan_string(b"\"a\"b\"", 0), Ok(3));
    }

    #[test]
    fn string_requires_opening_quote() {
        assert_json_err(scan_string(b"x", 0), "expected opening quote");
        assert_json_err(scan_string(b"", 0), "expected opening quote");
        // Start out of bounds.
        assert_json_err_at(scan_string(b"\"a\"", 9), 9, "expected opening quote");
    }

    #[test]
    fn string_every_simple_escape() {
        for e in [b'"', b'\\', b'/', b'b', b'f', b'n', b'r', b't'] {
            let src = [b'"', b'a', b'\\', e, b'b', b'"'];
            assert_eq!(scan_string(&src, 0), Ok(6), "escape {:?}", e as char);
        }
    }

    #[test]
    fn string_escaped_quote_is_not_a_terminator() {
        assert_eq!(scan_string(br#""a\"b""#, 0), Ok(6));
        // Escaped backslash then a real quote terminates.
        assert_eq!(scan_string(br#""a\\""#, 0), Ok(5));
    }

    /// Returns the source bytes `\uXXXX` for `hex` = `"XXXX"`, built at
    /// runtime so the test source stays free of escape-in-escape puzzles.
    fn uesc(hex: &str) -> Vec<u8> {
        let mut v = vec![b'\\', b'u'];
        v.extend_from_slice(hex.as_bytes());
        v
    }

    /// Returns `"\uXXXX"` (a quoted string holding one unicode escape).
    fn quoted_uesc(hex: &str) -> Vec<u8> {
        let mut v = vec![b'"'];
        v.extend_from_slice(&uesc(hex));
        v.push(b'"');
        v
    }

    #[test]
    fn string_unicode_escapes() {
        assert_eq!(scan_string(&quoted_uesc("0041"), 0), Ok(8));
        // HEXDIG is case-insensitive.
        assert_eq!(scan_string(&quoted_uesc("AbCd"), 0), Ok(8));
        assert_eq!(scan_string(&quoted_uesc("ffff"), 0), Ok(8));
        // Lone surrogates are accepted (grammar-level check; RFC 8259).
        assert_eq!(scan_string(&quoted_uesc("D800"), 0), Ok(8));
        assert_eq!(scan_string(&quoted_uesc("DFFF"), 0), Ok(8));
    }

    #[test]
    fn string_truncated_unicode_escape() {
        // The closing quote lands inside the 4 HEXDIGs.
        assert_json_err(scan_string(br#""\u00""#, 0), "invalid unicode escape");
        assert_json_err(scan_string(br#""\u""#, 0), "invalid unicode escape");
        // Truncated by end of input.
        assert_json_err(scan_string(br#""\u00"#, 0), "invalid unicode escape");
        assert_json_err(scan_string(br#""\u"#, 0), "invalid unicode escape");
        // Non-hex digit inside.
        assert_json_err(scan_string(br#""\u00G0""#, 0), "invalid unicode escape");
        assert_json_err(scan_string(br#""\uzzzz""#, 0), "invalid unicode escape");
    }

    #[test]
    fn string_bad_escapes() {
        assert_json_err(scan_string(br#""a\qb""#, 0), "invalid escape");
        assert_json_err(scan_string(br#""a\x00""#, 0), "invalid escape");
        // Capital-letter variants of valid escapes are invalid.
        assert_json_err(scan_string(br#""a\N""#, 0), "invalid escape");
        // Trailing backslash at end of input.
        assert_json_err(scan_string(br#""abc\"#, 0), "invalid escape");
    }

    #[test]
    fn string_raw_control_bytes() {
        assert_json_err_at(scan_string(b"\"a\x00b\"", 0), 2, "control byte in string");
        assert_json_err_at(scan_string(b"\"a\x1Fb\"", 0), 2, "control byte in string");
        // Raw tab and newline are control bytes too.
        assert_json_err(scan_string(b"\"a\tb\"", 0), "control byte in string");
        assert_json_err(scan_string(b"\"a\nb\"", 0), "control byte in string");
        // 0x20 (SP) is the first allowed raw byte.
        assert_eq!(scan_string(b"\"a b\"", 0), Ok(5));
    }

    #[test]
    fn string_unterminated() {
        assert_json_err_at(scan_string(b"\"abc", 0), 4, "unterminated string");
        assert_json_err(scan_string(b"\"", 0), "unterminated string");
    }

    #[test]
    fn string_high_bytes_pass_through() {
        // Multi-byte UTF-8 ("é" = C3 A9) passes; the scanner is byte-level.
        assert_eq!(scan_string("\"é\"".as_bytes(), 0), Ok(4));
        // Even invalid UTF-8 passes HERE: whole-body UTF-8 is checked by
        // validate_json, not the scanner.
        assert_eq!(scan_string(b"\"\xFF\"", 0), Ok(3));
    }

    // === scan_number ===

    #[test]
    fn number_valid_matrix() {
        for (src, len) in [
            ("0", 1),
            ("-0", 2),
            ("1", 1),
            ("42", 2),
            ("1.5", 3),
            ("-1.5e10", 7),
            ("1e+9", 4),
            ("1E-9", 4),
            ("0.1", 3),
            ("0e0", 3),
            ("-123.456E+789", 13),
        ] {
            assert_eq!(scan_number(src.as_bytes(), 0), Ok(len), "{src:?}");
        }
        // Not anchored at 0.
        assert_eq!(scan_number(b"[-1.5]", 1), Ok(5));
    }

    #[test]
    fn number_invalid_at_start() {
        for src in ["+1", ".5", "-", "e5", "", "-x", "x", "\"1\""] {
            assert_json_err(scan_number(src.as_bytes(), 0), "invalid number");
        }
        // Start out of bounds.
        assert_json_err(scan_number(b"1", 9), "invalid number");
    }

    #[test]
    fn number_longest_valid_prefix() {
        for (src, len) in [
            ("12e", 2), // exponent without digits
            ("12e ", 2),
            ("1.", 1),  // fraction without digits
            ("012", 1), // leading zero: lexeme is just `0`
            ("1.5.2", 3),
            ("1e+", 1),
            ("1e-", 1),
            ("1ee", 1),
            ("0x5", 1),
            ("1.2e3.4", 5),
            ("-0.5E-2x", 7),
            ("42,", 2),
        ] {
            assert_eq!(scan_number(src.as_bytes(), 0), Ok(len), "{src:?}");
        }
    }

    // === validate_json: roots and coverage (F1/F10) ===

    #[test]
    fn scalar_roots_with_exact_spans() {
        validate_json(b"42", &[node(JsonKind::Number, 0, 2, 1)]).unwrap();
        validate_json(b"\"hi\"", &[node(JsonKind::String, 1, 3, 1)]).unwrap();
        validate_json(b"true", &[node(JsonKind::Bool, 0, 4, 1)]).unwrap();
        validate_json(b"false", &[node(JsonKind::Bool, 0, 5, 1)]).unwrap();
        validate_json(b"null", &[node(JsonKind::Null, 0, 4, 1)]).unwrap();
    }

    #[test]
    fn root_with_surrounding_whitespace() {
        validate_json(b" \t\r\n42 \n", &[node(JsonKind::Number, 4, 6, 1)]).unwrap();
        validate_json(b" {} ", &[node(JsonKind::Object, 1, 3, 1)]).unwrap();
    }

    #[test]
    fn root_span_off_by_one_rejected() {
        let content = b" 42 ";
        validate_json(content, &[node(JsonKind::Number, 1, 3, 1)]).unwrap();
        for (start, end, want) in [
            (0, 3, "node start mismatch"),
            (2, 3, "node start mismatch"),
            (1, 2, "node end mismatch"),
            (1, 4, "node end mismatch"),
        ] {
            assert_json_err(
                validate_json(content, &[node(JsonKind::Number, start, end, 1)]),
                want,
            );
        }
    }

    #[test]
    fn residue_after_root_rejected() {
        assert_json_err_at(
            validate_json(b"{} x", &[node(JsonKind::Object, 0, 2, 1)]),
            3,
            "data after root value",
        );
        assert_json_err_at(
            validate_json(b"{}{}", &[node(JsonKind::Object, 0, 2, 1)]),
            2,
            "data after root value",
        );
        assert_json_err_at(
            validate_json(b"42 7", &[node(JsonKind::Number, 0, 2, 1)]),
            3,
            "data after root value",
        );
    }

    #[test]
    fn all_whitespace_or_empty_rejected() {
        assert_json_err_at(
            validate_json(b"  \t\n", &[node(JsonKind::Null, 0, 4, 1)]),
            4,
            "expected a JSON value",
        );
        // Empty content trips the node/byte cap (a node owns >= 1 byte).
        assert_table_err(
            validate_json(b"", &[node(JsonKind::Null, 0, 0, 1)]),
            "more JSON nodes than body bytes",
        );
    }

    #[test]
    fn node_table_well_formedness() {
        assert_table_err(validate_json(b"{}", &[]), "empty JSON node table");
        let n = node(JsonKind::Number, 0, 1, 1);
        assert_table_err(
            validate_json(b"1", &[n, n]),
            "more JSON nodes than body bytes",
        );
    }

    #[test]
    fn invalid_utf8_body_rejected() {
        assert_json_err_at(
            validate_json(b"\"\xFF\"", &[node(JsonKind::String, 1, 2, 1)]),
            1,
            "body is not valid UTF-8",
        );
        assert_json_err_at(
            validate_json(b"\xFF", &[node(JsonKind::Null, 0, 1, 1)]),
            0,
            "body is not valid UTF-8",
        );
    }

    // === validate_json: containers and tiling (F3/F4) ===

    #[test]
    fn empty_containers() {
        validate_json(b"{}", &[node(JsonKind::Object, 0, 2, 1)]).unwrap();
        validate_json(b"[]", &[node(JsonKind::Array, 0, 2, 1)]).unwrap();
        validate_json(b"[ ]", &[node(JsonKind::Array, 0, 3, 1)]).unwrap();
        validate_json(b"{\t\n}", &[node(JsonKind::Object, 0, 4, 1)]).unwrap();
    }

    fn simple_object_nodes() -> Vec<JsonNode> {
        vec![
            node(JsonKind::Object, 0, 7, 3),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 5, 6, 1),
        ]
    }

    #[test]
    fn simple_object() {
        validate_json(br#"{"a":1}"#, &simple_object_nodes()).unwrap();
    }

    /// Nested document exercising every kind; same shape as the
    /// transcript-accessor fixture.
    const DOC1: &[u8] = br#"{"a": [1, {"b": "x"}, true], "c": null, "d": 1.5e3}"#;

    fn doc1_nodes() -> Vec<JsonNode> {
        vec![
            node(JsonKind::Object, 0, 51, 12),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Array, 6, 27, 6),
            node(JsonKind::Number, 7, 8, 1),
            node(JsonKind::Object, 10, 20, 3),
            node(JsonKind::Key, 12, 13, 1),
            node(JsonKind::String, 17, 18, 1),
            node(JsonKind::Bool, 22, 26, 1),
            node(JsonKind::Key, 30, 31, 1),
            node(JsonKind::Null, 34, 38, 1),
            node(JsonKind::Key, 41, 42, 1),
            node(JsonKind::Number, 45, 50, 1),
        ]
    }

    #[test]
    fn nested_documents() {
        validate_json(DOC1, &doc1_nodes()).unwrap();
        // Multi-node subtrees on both sides of sibling boundaries.
        let doc2 = br#"[[1,2],{"k":[3]},"s"]"#;
        let nodes = vec![
            node(JsonKind::Array, 0, 21, 9),
            node(JsonKind::Array, 1, 6, 3),
            node(JsonKind::Number, 2, 3, 1),
            node(JsonKind::Number, 4, 5, 1),
            node(JsonKind::Object, 7, 16, 4),
            node(JsonKind::Key, 9, 10, 1),
            node(JsonKind::Array, 12, 15, 2),
            node(JsonKind::Number, 13, 14, 1),
            node(JsonKind::String, 18, 19, 1),
        ];
        validate_json(doc2, &nodes).unwrap();
    }

    #[test]
    fn whitespace_variants_inside_containers() {
        let content = br#"{ "a" : [ 1 , 2 ] }"#;
        let nodes = vec![
            node(JsonKind::Object, 0, 19, 5),
            node(JsonKind::Key, 3, 4, 1),
            node(JsonKind::Array, 8, 17, 3),
            node(JsonKind::Number, 10, 11, 1),
            node(JsonKind::Number, 14, 15, 1),
        ];
        validate_json(content, &nodes).unwrap();

        let content = b"{\n\t\"a\"\r\n:\t1\n}";
        let nodes = vec![
            node(JsonKind::Object, 0, 13, 3),
            node(JsonKind::Key, 4, 5, 1),
            node(JsonKind::Number, 10, 11, 1),
        ];
        validate_json(content, &nodes).unwrap();
    }

    #[test]
    fn comment_in_gap_rejected() {
        let nodes = vec![
            node(JsonKind::Array, 0, 10, 2),
            node(JsonKind::Number, 1, 2, 1),
        ];
        assert_json_err_at(
            validate_json(b"[1 /*c*/ ]", &nodes),
            3,
            "expected comma or closing bracket",
        );
        // A comment before the root is an invalid value byte.
        assert_json_err_at(
            validate_json(b"/* */42", &[node(JsonKind::Number, 5, 7, 1)]),
            0,
            "unexpected byte at value position",
        );
        // Non-JWS control whitespace (vertical tab) is not a gap byte.
        let nodes = vec![
            node(JsonKind::Array, 0, 4, 2),
            node(JsonKind::Number, 1, 2, 1),
        ];
        assert_json_err_at(
            validate_json(b"[1\x0B]", &nodes),
            2,
            "expected comma or closing bracket",
        );
    }

    #[test]
    fn trailing_and_double_commas_rejected() {
        let nodes = vec![
            node(JsonKind::Array, 0, 4, 2),
            node(JsonKind::Number, 1, 2, 1),
        ];
        assert_json_err_at(
            validate_json(b"[1,]", &nodes),
            3,
            "unexpected byte at value position",
        );
        let nodes = vec![
            node(JsonKind::Object, 0, 8, 3),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 5, 6, 1),
        ];
        assert_json_err_at(
            validate_json(br#"{"a":1,}"#, &nodes),
            7,
            "expected object key",
        );
        let nodes = vec![
            node(JsonKind::Array, 0, 6, 3),
            node(JsonKind::Number, 1, 2, 1),
            node(JsonKind::Number, 4, 5, 1),
        ];
        assert_json_err_at(
            validate_json(b"[1,,2]", &nodes),
            3,
            "unexpected byte at value position",
        );
    }

    #[test]
    fn missing_colon_or_comma_rejected() {
        assert_json_err_at(
            validate_json(br#"{"a" 1}"#, &simple_object_nodes()),
            5,
            "expected colon",
        );
        assert_json_err_at(
            validate_json(br#"{"a"1}"#, &simple_object_nodes()),
            4,
            "expected colon",
        );
        let nodes = vec![
            node(JsonKind::Array, 0, 5, 3),
            node(JsonKind::Number, 1, 2, 1),
            node(JsonKind::Number, 3, 4, 1),
        ];
        assert_json_err_at(
            validate_json(b"[1 2]", &nodes),
            3,
            "expected comma or closing bracket",
        );
        let nodes = vec![
            node(JsonKind::Object, 0, 13, 5),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 5, 6, 1),
            node(JsonKind::Key, 8, 9, 1),
            node(JsonKind::Number, 11, 12, 1),
        ];
        assert_json_err_at(
            validate_json(br#"{"a":1 "b":2}"#, &nodes),
            7,
            "expected comma or closing bracket",
        );
    }

    // === validate_json: kind anchoring (F2) and literals (F8) ===

    #[test]
    fn kind_relabels_rejected() {
        // Number claimed String.
        assert_json_err_at(
            validate_json(b"12", &[node(JsonKind::String, 0, 2, 1)]),
            0,
            "node kind mismatch",
        );
        // Object claimed Array and vice versa.
        assert_json_err(
            validate_json(b"{}", &[node(JsonKind::Array, 0, 2, 1)]),
            "node kind mismatch",
        );
        assert_json_err(
            validate_json(b"[]", &[node(JsonKind::Object, 0, 2, 1)]),
            "node kind mismatch",
        );
        // Quoted "true" claimed Bool: the quote admits only String.
        assert_json_err(
            validate_json(b"\"true\"", &[node(JsonKind::Bool, 0, 6, 1)]),
            "node kind mismatch",
        );
        // Bare literal claimed String.
        assert_json_err(
            validate_json(b"true", &[node(JsonKind::String, 1, 4, 1)]),
            "node kind mismatch",
        );
        // Wrong-case literals admit no kind at all.
        assert_json_err_at(
            validate_json(b"True", &[node(JsonKind::Bool, 0, 4, 1)]),
            0,
            "unexpected byte at value position",
        );
        assert_json_err(
            validate_json(b"NULL", &[node(JsonKind::Null, 0, 4, 1)]),
            "unexpected byte at value position",
        );
    }

    #[test]
    fn literal_bytes_must_be_exact() {
        assert_json_err_at(
            validate_json(b"tru", &[node(JsonKind::Bool, 0, 3, 1)]),
            0,
            "invalid literal",
        );
        assert_json_err(
            validate_json(b"fals", &[node(JsonKind::Bool, 0, 4, 1)]),
            "invalid literal",
        );
        assert_json_err(
            validate_json(b"nulL", &[node(JsonKind::Null, 0, 4, 1)]),
            "invalid literal",
        );
        // A correct literal with residue fails coverage, not the literal.
        assert_json_err_at(
            validate_json(b"falsey", &[node(JsonKind::Bool, 0, 5, 1)]),
            5,
            "data after root value",
        );
    }

    // === validate_json: span/size tampering ===

    #[test]
    fn number_span_tampering_rejected() {
        // End short of the scanned lexeme ("12" claimed inside "123").
        assert_json_err_at(
            validate_json(b"123", &[node(JsonKind::Number, 0, 2, 1)]),
            0,
            "node end mismatch",
        );
        assert_json_err(
            validate_json(b"123 ", &[node(JsonKind::Number, 0, 4, 1)]),
            "node end mismatch",
        );
        // Span excluding the exponent.
        assert_json_err(
            validate_json(b"1.5e3", &[node(JsonKind::Number, 0, 3, 1)]),
            "node end mismatch",
        );
        assert_json_err(
            validate_json(b"123", &[node(JsonKind::Number, 1, 3, 1)]),
            "node start mismatch",
        );
    }

    #[test]
    fn string_span_tampering_rejected() {
        // Content "aAb" (escape built at runtime).
        let mut content = vec![b'"', b'a'];
        content.extend_from_slice(&uesc("0041"));
        content.extend_from_slice(b"b\"");
        assert_eq!(content.len(), 10);
        validate_json(&content, &[node(JsonKind::String, 1, 9, 1)]).unwrap();
        // End inside the \uXXXX escape.
        assert_json_err(
            validate_json(&content, &[node(JsonKind::String, 1, 5, 1)]),
            "node end mismatch",
        );
        assert_json_err(
            validate_json(&content, &[node(JsonKind::String, 1, 8, 1)]),
            "node end mismatch",
        );
        // Span including a quote on either side.
        assert_json_err(
            validate_json(&content, &[node(JsonKind::String, 0, 9, 1)]),
            "node start mismatch",
        );
        assert_json_err(
            validate_json(&content, &[node(JsonKind::String, 1, 10, 1)]),
            "node end mismatch",
        );
    }

    #[test]
    fn container_end_and_size_tampering_rejected() {
        let content = br#"{"a":1}"#;
        for (end, size, want) in [
            (6, 3, "container end mismatch"),
            (8, 3, "container end mismatch"),
            (7, 2, "container size mismatch"),
            (7, 4, "container size mismatch"),
        ] {
            let mut nodes = simple_object_nodes();
            nodes[0] = node(JsonKind::Object, 0, end, size);
            assert_json_err(validate_json(content, &nodes), want);
        }
        // Empty container claiming size 0.
        assert_json_err_at(
            validate_json(b"{}", &[node(JsonKind::Object, 0, 2, 0)]),
            1,
            "container size mismatch",
        );
        // Array variants.
        let arr = |end, size| {
            vec![
                node(JsonKind::Array, 0, end, size),
                node(JsonKind::Number, 1, 2, 1),
            ]
        };
        assert_json_err(validate_json(b"[1]", &arr(2, 2)), "container end mismatch");
        assert_json_err(validate_json(b"[1]", &arr(3, 1)), "container size mismatch");
        assert_json_err(validate_json(b"[1]", &arr(3, 3)), "container size mismatch");
    }

    #[test]
    fn leaf_size_tampering_rejected() {
        assert_json_err(
            validate_json(b"42", &[node(JsonKind::Number, 0, 2, 0)]),
            "leaf node size mismatch",
        );
        assert_json_err(
            validate_json(b"42", &[node(JsonKind::Number, 0, 2, 2)]),
            "leaf node size mismatch",
        );
        let mut nodes = simple_object_nodes();
        nodes[1] = node(JsonKind::Key, 2, 3, 2);
        assert_json_err_at(
            validate_json(br#"{"a":1}"#, &nodes),
            1,
            "leaf node size mismatch",
        );
    }

    #[test]
    fn node_insert_delete_reorder_rejected() {
        let content = br#"{"a":1}"#;
        let [obj, key, num] = [
            node(JsonKind::Object, 0, 7, 3),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 5, 6, 1),
        ];
        // Key record deleted: the value node shows up at the key position.
        assert_json_err_at(validate_json(content, &[obj, num]), 1, "node kind mismatch");
        // Value record deleted: table exhausted at the value position.
        assert_json_err_at(
            validate_json(content, &[obj, key]),
            5,
            "node table exhausted",
        );
        // Key record duplicated (insertion).
        assert_json_err_at(
            validate_json(content, &[obj, key, key, num]),
            5,
            "node kind mismatch",
        );
        // Extra trailing record.
        assert_json_err_at(
            validate_json(content, &[obj, key, num, node(JsonKind::Null, 0, 4, 1)]),
            7,
            "table has extra JSON nodes",
        );
        // Key/value records swapped.
        assert_json_err_at(
            validate_json(content, &[obj, num, key]),
            1,
            "node kind mismatch",
        );
        // Sibling leaves reordered in an array: spans betray the swap.
        let content = b"[1,true]";
        let [arr, one, tru] = [
            node(JsonKind::Array, 0, 8, 3),
            node(JsonKind::Number, 1, 2, 1),
            node(JsonKind::Bool, 3, 7, 1),
        ];
        validate_json(content, &[arr, one, tru]).unwrap();
        assert_json_err_at(
            validate_json(content, &[arr, tru, one]),
            1,
            "node kind mismatch",
        );
    }

    #[test]
    fn key_value_role_confusion_rejected() {
        // Key node where a value is expected.
        let nodes = vec![node(JsonKind::Array, 0, 3, 2), node(JsonKind::Key, 1, 2, 1)];
        assert_json_err_at(validate_json(b"[1]", &nodes), 1, "node kind mismatch");
        // Key node as the root value.
        assert_json_err(
            validate_json(b"\"a\"", &[node(JsonKind::Key, 1, 2, 1)]),
            "node kind mismatch",
        );
        // String (value) node where a key is expected.
        let mut nodes = simple_object_nodes();
        nodes[1] = node(JsonKind::String, 2, 3, 1);
        assert_json_err_at(
            validate_json(br#"{"a":1}"#, &nodes),
            1,
            "node kind mismatch",
        );
    }

    #[test]
    fn truncated_documents_rejected() {
        assert_json_err_at(
            validate_json(
                b"[1",
                &[
                    node(JsonKind::Array, 0, 2, 2),
                    node(JsonKind::Number, 1, 2, 1),
                ],
            ),
            2,
            "unterminated container",
        );
        assert_json_err_at(
            validate_json(b"[", &[node(JsonKind::Array, 0, 1, 1)]),
            1,
            "expected a JSON value",
        );
        assert_json_err_at(
            validate_json(b"{", &[node(JsonKind::Object, 0, 1, 1)]),
            1,
            "expected object key",
        );
        let partial = vec![
            node(JsonKind::Object, 0, 4, 2),
            node(JsonKind::Key, 2, 3, 1),
        ];
        assert_json_err_at(validate_json(br#"{"a""#, &partial), 4, "expected colon");
        let partial = vec![
            node(JsonKind::Object, 0, 5, 2),
            node(JsonKind::Key, 2, 3, 1),
        ];
        assert_json_err_at(
            validate_json(br#"{"a":"#, &partial),
            5,
            "expected a JSON value",
        );
        assert_json_err_at(
            validate_json(
                b"[1,",
                &[
                    node(JsonKind::Array, 0, 3, 2),
                    node(JsonKind::Number, 1, 2, 1),
                ],
            ),
            3,
            "expected a JSON value",
        );
        assert_json_err_at(
            validate_json(br#"{"a":1"#, &simple_object_nodes()),
            6,
            "unterminated container",
        );
    }

    #[test]
    fn number_maximality_via_tiling() {
        // In a container, a number must run to a separator/closer.
        let nodes = vec![
            node(JsonKind::Array, 0, 5, 2),
            node(JsonKind::Number, 1, 3, 1),
        ];
        assert_json_err_at(
            validate_json(b"[12x]", &nodes),
            3,
            "expected comma or closing bracket",
        );
        // At the root, residue after the maximal lexeme fails coverage.
        assert_json_err_at(
            validate_json(b"12x", &[node(JsonKind::Number, 0, 2, 1)]),
            2,
            "data after root value",
        );
        // `[1.5e]`: the maximal lexeme is `1.5`, then tiling rejects `e`.
        let nodes = vec![
            node(JsonKind::Array, 0, 6, 2),
            node(JsonKind::Number, 1, 4, 1),
        ];
        assert_json_err_at(
            validate_json(b"[1.5e]", &nodes),
            4,
            "expected comma or closing bracket",
        );
    }

    #[test]
    fn multibyte_strings_accepted() {
        let content = "\"wörld\"".as_bytes();
        assert_eq!(content.len(), 8);
        validate_json(content, &[node(JsonKind::String, 1, 7, 1)]).unwrap();
        let content = "[\"😀\"]".as_bytes();
        assert_eq!(content.len(), 8);
        let nodes = vec![
            node(JsonKind::Array, 0, 8, 2),
            node(JsonKind::String, 2, 6, 1),
        ];
        validate_json(content, &nodes).unwrap();
    }

    // === validate_json: depth cap (F5) ===

    /// Builds `[[[...]]]` of the given container depth with its honest node
    /// table.
    fn nested_arrays(depth: usize) -> (Vec<u8>, Vec<JsonNode>) {
        let mut content = Vec::new();
        let mut nodes = Vec::new();
        for i in 0..depth {
            content.push(b'[');
            nodes.push(node(JsonKind::Array, i, 2 * depth - i, depth - i));
        }
        content.extend(core::iter::repeat_n(b']', depth));
        (content, nodes)
    }

    #[test]
    fn depth_cap_boundary() {
        // Pinned cap semantics (MAX_JSON_DEPTH = 127): a document whose
        // deepest position has up to 127 simultaneously open containers
        // validates; opening a 128th is an error. (Matches serde_json's
        // default recursion limit.)
        let (content, nodes) = nested_arrays(127);
        validate_json(&content, &nodes).unwrap();
        let (content, nodes) = nested_arrays(128);
        assert_table_err(
            validate_json(&content, &nodes),
            "JSON nesting depth exceeds 127",
        );
        let (content, nodes) = nested_arrays(129);
        assert_table_err(
            validate_json(&content, &nodes),
            "JSON nesting depth exceeds 127",
        );
    }

    // === validate_json: duplicate keys (F9) ===

    /// Builds `{"K1":1,"K2":2}` from raw key content bytes, with its honest
    /// node table.
    fn two_key_object(k1: &[u8], k2: &[u8]) -> (Vec<u8>, Vec<JsonNode>) {
        let mut c = Vec::from(&b"{\""[..]);
        let k1_start = c.len();
        c.extend_from_slice(k1);
        let k1_end = c.len();
        c.extend_from_slice(b"\":1,\"");
        let k2_start = c.len();
        c.extend_from_slice(k2);
        let k2_end = c.len();
        c.extend_from_slice(b"\":2}");
        let nodes = vec![
            node(JsonKind::Object, 0, c.len(), 5),
            node(JsonKind::Key, k1_start, k1_end, 1),
            node(JsonKind::Number, k1_end + 2, k1_end + 3, 1),
            node(JsonKind::Key, k2_start, k2_end, 1),
            node(JsonKind::Number, k2_end + 2, k2_end + 3, 1),
        ];
        (c, nodes)
    }

    /// Asserts rejection with the duplicate reported at the SECOND key's
    /// content start.
    fn assert_dup(c: &[u8], n: &[JsonNode]) {
        let at = n[3].start as usize;
        assert_json_err_at(validate_json(c, n), at, "duplicate object key");
    }

    #[test]
    fn duplicate_raw_keys_rejected() {
        let (c, n) = two_key_object(b"a", b"a");
        assert_dup(&c, &n);
        let (c, n) = two_key_object(b"", b"");
        assert_dup(&c, &n);
    }

    #[test]
    fn duplicate_keys_via_escape_aliases_rejected() {
        // Raw "a" vs the u-0061 escape.
        let (c, n) = two_key_object(b"a", &uesc("0061"));
        assert_dup(&c, &n);
        // Raw "é" vs the u-00e9 escape.
        let (c, n) = two_key_object("é".as_bytes(), &uesc("00e9"));
        assert_dup(&c, &n);
        // Raw "😀" vs the surrogate-pair escape u-D83D u-DE00.
        let mut pair = uesc("D83D");
        pair.extend_from_slice(&uesc("DE00"));
        let (c, n) = two_key_object("😀".as_bytes(), &pair);
        assert_dup(&c, &n);
        // The simple tab escape vs the u-0009 escape.
        let (c, n) = two_key_object(b"\\t", &uesc("0009"));
        assert_dup(&c, &n);
        // The escaped-quote escape vs the u-0022 escape.
        let (c, n) = two_key_object(b"\\\"", &uesc("0022"));
        assert_dup(&c, &n);
    }

    #[test]
    fn lone_surrogate_escape_comparison() {
        // Lone surrogates keep their raw value: u-D800 then raw "A"
        // equals u-D800 then escaped u-0041.
        let mut k1 = uesc("D800");
        k1.push(b'A');
        let mut k2 = uesc("D800");
        k2.extend_from_slice(&uesc("0041"));
        let (c, n) = two_key_object(&k1, &k2);
        assert_dup(&c, &n);
        // ...but distinct lone surrogates are distinct keys.
        let (c, n) = two_key_object(&uesc("D800"), &uesc("DC00"));
        validate_json(&c, &n).unwrap();
        // A high+low pair COMBINES, so it differs from the lone high.
        let mut pair = uesc("D800");
        pair.extend_from_slice(&uesc("DC00"));
        let (c, n) = two_key_object(&uesc("D800"), &pair);
        validate_json(&c, &n).unwrap();
        // Low-then-high does NOT combine: distinct from the combined pair.
        let mut reversed = uesc("DC00");
        reversed.extend_from_slice(&uesc("D800"));
        let (c, n) = two_key_object(&pair, &reversed);
        validate_json(&c, &n).unwrap();
    }

    #[test]
    fn distinct_keys_accepted() {
        // Key comparison is case-sensitive.
        let (c, n) = two_key_object(b"a", b"A");
        validate_json(&c, &n).unwrap();
        let (c, n) = two_key_object(b"a", b"ab");
        validate_json(&c, &n).unwrap();
        // A single empty key is fine; duplicates of it are not (above).
        let nodes = vec![
            node(JsonKind::Object, 0, 6, 3),
            node(JsonKind::Key, 2, 2, 1),
            node(JsonKind::Number, 4, 5, 1),
        ];
        validate_json(br#"{"":1}"#, &nodes).unwrap();
    }

    #[test]
    fn same_keys_at_different_nesting_accepted() {
        // Parent and child objects may reuse a key.
        let nodes = vec![
            node(JsonKind::Object, 0, 13, 5),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Object, 5, 12, 3),
            node(JsonKind::Key, 7, 8, 1),
            node(JsonKind::Number, 10, 11, 1),
        ];
        validate_json(br#"{"a":{"a":1}}"#, &nodes).unwrap();
        // Sibling objects in an array may too.
        let nodes = vec![
            node(JsonKind::Array, 0, 17, 7),
            node(JsonKind::Object, 1, 8, 3),
            node(JsonKind::Key, 3, 4, 1),
            node(JsonKind::Number, 6, 7, 1),
            node(JsonKind::Object, 9, 16, 3),
            node(JsonKind::Key, 11, 12, 1),
            node(JsonKind::Number, 14, 15, 1),
        ];
        validate_json(br#"[{"a":1},{"a":2}]"#, &nodes).unwrap();
    }

    #[test]
    fn duplicate_detected_non_adjacent() {
        // Sorting catches duplicates separated by another member; the
        // reported position is the LATER occurrence in document order.
        let content = br#"{"a":1,"b":2,"a":3}"#;
        let nodes = vec![
            node(JsonKind::Object, 0, 19, 7),
            node(JsonKind::Key, 2, 3, 1),
            node(JsonKind::Number, 5, 6, 1),
            node(JsonKind::Key, 8, 9, 1),
            node(JsonKind::Number, 11, 12, 1),
            node(JsonKind::Key, 14, 15, 1),
            node(JsonKind::Number, 17, 18, 1),
        ];
        assert_json_err_at(validate_json(content, &nodes), 14, "duplicate object key");
    }

    // === end-to-end through the public validate() ===

    #[test]
    fn end_to_end_json_body_through_public_validate() {
        use crate::{
            spans::{
                BodySpans, FORMAT_VERSION, Framing, HeaderSpan, JsonSpans, RequestSpans,
                ResponseSpans, Span, SpanTable,
            },
            validate::validate,
        };

        let sent = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        let recv = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        let table = SpanTable {
            version: FORMAT_VERSION,
            request: RequestSpans {
                method: Span::new(0, 3),
                target: Span::new(4, 6),
                head_end: 28,
                headers: vec![HeaderSpan {
                    name: Span::new(17, 21),
                    value: Span::new(23, 24),
                }],
                body: None,
            },
            response: ResponseSpans {
                code: Span::new(9, 12),
                reason: Span::new(13, 15),
                head_end: 39,
                headers: vec![HeaderSpan {
                    name: Span::new(17, 31),
                    value: Span::new(33, 35),
                }],
                body: Some(BodySpans {
                    framing: Framing::ContentLength,
                    raw: Span::new(39, 50),
                    content_len: 11,
                    trailers: vec![],
                    json: Some(JsonSpans {
                        nodes: vec![
                            node(JsonKind::Object, 0, 11, 3),
                            node(JsonKind::Key, 2, 4, 1),
                            node(JsonKind::Bool, 6, 10, 1),
                        ],
                    }),
                }),
            },
        };

        let transcript = validate(sent, recv, &table).unwrap();
        assert_eq!(transcript.request().method(), "GET");
        assert_eq!(transcript.response().status(), 200);
        let body = transcript.response().body().unwrap();
        assert_eq!(body.content(), b"{\"ok\":true}");
        let ok = body.json().unwrap().get("ok").unwrap();
        assert_eq!(ok.kind(), JsonKind::Bool);
        assert_eq!(ok.as_bool(), Some(true));

        // One tampered node fails the WHOLE validation.
        let mut bad = table.clone();
        if let Some(body) = bad.response.body.as_mut()
            && let Some(json) = body.json.as_mut()
        {
            json.nodes[2] = node(JsonKind::Bool, 6, 9, 1);
        }
        assert!(matches!(
            validate(sent, recv, &bad),
            Err(Error::Json {
                reason: "node end mismatch",
                ..
            })
        ));
    }
}
