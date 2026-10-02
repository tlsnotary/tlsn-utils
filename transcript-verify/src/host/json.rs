//! Host-side span-emitting JSON pass over a decoded body.

use alloc::vec::Vec;

use crate::{
    error::Error,
    spans::{JsonKind, JsonNode},
    validate::json::{MAX_JSON_DEPTH, json_err, scan_number, scan_string, skip_jws, validate_json},
};

/// Maximum content length, mirroring the validator driver's 2^30-byte
/// coordinate limit. Inputs under this limit make every `usize`-to-`u32`
/// position cast below lossless.
const MAX_CONTENT_LEN: usize = 1 << 30;

/// One open container during the emit walk.
struct Frame {
    /// Index of the container's node in the table under construction.
    node_idx: usize,
    /// `true` for an object frame, `false` for an array frame.
    is_object: bool,
}

/// Emits the pre-order JSON node table for `content` (a decoded body), in
/// decoded-body coordinates.
///
/// Explicit-stack recursive descent sharing the validator's
/// [`scan_string`] / [`scan_number`] scanners — so host
/// and guest agree on the grammar by construction. Nodes are pushed
/// pre-order; container `end` and `size` are patched at container close.
/// The walk mirrors the checker's, so the two visit nodes in the same
/// order.
///
/// Guarantee by construction: before returning, the emitted table is run
/// through the validator's own
/// [`validate_json`]; a rejection is
/// returned as that error instead of `Ok`. Emit-accepted is therefore a
/// subset of validate-accepted unconditionally — in particular duplicate
/// object keys, non-UTF-8 bodies, and every other checker policy are
/// inherited exactly rather than reimplemented here.
///
/// An `Err` means `content` is not JSON the validator would accept (the
/// error is the validator's own [`Error::Json`]/[`Error::Table`] — plus
/// [`Error::TooLarge`] for bodies over the 2^30-byte coordinate limit);
/// callers fall back to an opaque (no-JSON) body claim rather than failing
/// the host parse.
pub(crate) fn emit(content: &[u8]) -> Result<Vec<JsonNode>, Error> {
    if content.len() > MAX_CONTENT_LEN {
        return Err(Error::TooLarge { len: content.len() });
    }

    let mut nodes: Vec<JsonNode> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut p = skip_jws(content, 0);

    'value: loop {
        // === a value starts at `p`; its first byte determines the kind ===
        let Some(&b) = content.get(p) else {
            return Err(json_err(p, "expected a JSON value"));
        };
        match b {
            b'{' | b'[' => {
                // Same cap semantics as the checker (rule F5): with 127
                // frames already open, a 128th container is rejected.
                if stack.len() >= MAX_JSON_DEPTH {
                    return Err(Error::Table {
                        reason: "JSON nesting depth exceeds 127",
                    });
                }
                let is_object = b == b'{';
                stack.push(Frame {
                    node_idx: nodes.len(),
                    is_object,
                });
                nodes.push(JsonNode {
                    kind: if is_object {
                        JsonKind::Object
                    } else {
                        JsonKind::Array
                    },
                    start: p as u32,
                    // Patched by `close_top` when the container closes.
                    end: 0,
                    size: 0,
                });
                p = skip_jws(content, p + 1);
                let closer = if is_object { b'}' } else { b']' };
                if content.get(p) == Some(&closer) {
                    // Empty container: close immediately, then cascade
                    // below.
                    p += 1;
                    close_top(&mut nodes, &mut stack, p);
                } else if is_object {
                    // A non-empty object interior opens with a key.
                    p = emit_member_key(content, &mut nodes, p)?;
                    continue 'value;
                } else {
                    // Array: the first element value starts here.
                    continue 'value;
                }
            }
            b'"' => {
                let end = scan_string(content, p)?;
                // Content span between the quotes; `end >= p + 2`, so the
                // subtraction cannot underflow.
                nodes.push(JsonNode {
                    kind: JsonKind::String,
                    start: (p + 1) as u32,
                    end: (end - 1) as u32,
                    size: 1,
                });
                p = end;
            }
            b'-' | b'0'..=b'9' => {
                let end = scan_number(content, p)?;
                nodes.push(JsonNode {
                    kind: JsonKind::Number,
                    start: p as u32,
                    end: end as u32,
                    size: 1,
                });
                p = end;
            }
            b't' | b'f' | b'n' => {
                let (lit, kind): (&[u8], JsonKind) = match b {
                    b't' => (b"true", JsonKind::Bool),
                    b'f' => (b"false", JsonKind::Bool),
                    _ => (b"null", JsonKind::Null),
                };
                // `p < content.len() <= 2^30`: the add cannot overflow.
                let end = p + lit.len();
                if content.get(p..end) != Some(lit) {
                    return Err(json_err(p, "invalid literal"));
                }
                nodes.push(JsonNode {
                    kind,
                    start: p as u32,
                    end: end as u32,
                    size: 1,
                });
                p = end;
            }
            _ => return Err(json_err(p, "unexpected byte at value position")),
        }

        // === after a completed value: separator / closer cascade ===
        loop {
            let Some(frame) = stack.last() else {
                // The root value is complete; only whitespace may follow.
                p = skip_jws(content, p);
                if p != content.len() {
                    return Err(json_err(p, "data after root value"));
                }
                break 'value;
            };
            let is_object = frame.is_object;
            p = skip_jws(content, p);
            let Some(&b) = content.get(p) else {
                return Err(json_err(p, "unterminated container"));
            };
            if b == b',' {
                p = skip_jws(content, p + 1);
                if is_object {
                    p = emit_member_key(content, &mut nodes, p)?;
                }
                // A value must follow the comma.
                continue 'value;
            }
            let closer = if is_object { b'}' } else { b']' };
            if b == closer {
                p += 1;
                close_top(&mut nodes, &mut stack, p);
                // Cascade: the parent may close at this position too.
                continue;
            }
            return Err(json_err(p, "expected comma or closing bracket"));
        }
    }

    // GUARANTEE BY CONSTRUCTION: run the checker on our own output, so
    // emit-accepted ⊆ validate-accepted holds unconditionally. This is
    // where checker-only policies (duplicate keys, whole-body UTF-8)
    // reject documents the grammar walk above accepted.
    validate_json(content, &nodes)?;
    Ok(nodes)
}

/// Emits the [`JsonKind::Key`] node for one object-member key at `p` (the
/// opening quote) and consumes the following colon.
///
/// Returns the cursor at the member value's first byte. Content span
/// semantics match the checker: the quotes are excluded.
fn emit_member_key(content: &[u8], nodes: &mut Vec<JsonNode>, p: usize) -> Result<usize, Error> {
    if content.get(p) != Some(&b'"') {
        return Err(json_err(p, "expected object key"));
    }
    let end = scan_string(content, p)?;
    // `end >= p + 2`: neither arithmetic step can wrap.
    nodes.push(JsonNode {
        kind: JsonKind::Key,
        start: (p + 1) as u32,
        end: (end - 1) as u32,
        size: 1,
    });
    let mut q = skip_jws(content, end);
    if content.get(q) != Some(&b':') {
        return Err(json_err(q, "expected colon"));
    }
    q = skip_jws(content, q + 1);
    Ok(q)
}

/// Pops the top frame at a container close and patches its node: `end` is
/// `p` (one past the closer) and `size` is the number of nodes emitted
/// since (and including) the container's own.
///
/// Infallible by construction: both call sites hold a nonempty stack, and
/// the frame's node index is in bounds because the node is pushed before
/// its frame. Were either invariant ever broken, the node would keep its
/// `end = 0` / `size = 0` placeholders and the final [`validate_json`]
/// self-check in [`emit`] would reject — never a wrong `Ok`.
fn close_top(nodes: &mut [JsonNode], stack: &mut Vec<Frame>, p: usize) {
    let count = nodes.len();
    if let Some(frame) = stack.pop()
        && let Some(node) = nodes.get_mut(frame.node_idx)
    {
        node.end = p as u32;
        node.size = count.saturating_sub(frame.node_idx) as u32;
    }
}

#[cfg(test)]
mod tests {
    use alloc::{
        string::{String, ToString},
        vec,
        vec::Vec,
    };

    use super::*;
    use crate::validate::http::walk_chunks;

    fn node(kind: JsonKind, start: usize, end: usize, size: usize) -> JsonNode {
        JsonNode {
            kind,
            start: start as u32,
            end: end as u32,
            size: size as u32,
        }
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

    /// Builds `[[[...]]]` with the given number of nested containers.
    fn nested_arrays(depth: usize) -> Vec<u8> {
        let mut v = vec![b'['; depth];
        v.extend(core::iter::repeat_n(b']', depth));
        v
    }

    fn assert_json_err_at<T: core::fmt::Debug>(result: Result<T, Error>, at: usize, want: &str) {
        match result {
            Err(Error::Json { at: got_at, reason }) => {
                assert_eq!((got_at, reason), (at as u32, want));
            }
            other => panic!("expected Json {{ at: {at}, {want:?} }}, got {other:?}"),
        }
    }

    // === 1. hand-expectation tests: exact node arrays ===

    #[test]
    fn exact_nodes_scalar_roots() {
        assert_eq!(emit(b"42").unwrap(), vec![node(JsonKind::Number, 0, 2, 1)]);
        assert_eq!(
            emit(br#""hi""#).unwrap(),
            vec![node(JsonKind::String, 1, 3, 1)]
        );
        assert_eq!(emit(b"true").unwrap(), vec![node(JsonKind::Bool, 0, 4, 1)]);
        assert_eq!(emit(b"false").unwrap(), vec![node(JsonKind::Bool, 0, 5, 1)]);
        assert_eq!(emit(b"null").unwrap(), vec![node(JsonKind::Null, 0, 4, 1)]);
        assert_eq!(
            emit(b"-1.5e3").unwrap(),
            vec![node(JsonKind::Number, 0, 6, 1)]
        );
    }

    #[test]
    fn exact_nodes_empty_containers() {
        assert_eq!(emit(b"{}").unwrap(), vec![node(JsonKind::Object, 0, 2, 1)]);
        assert_eq!(emit(b"[]").unwrap(), vec![node(JsonKind::Array, 0, 2, 1)]);
        assert_eq!(emit(b"{ }").unwrap(), vec![node(JsonKind::Object, 0, 3, 1)]);
        assert_eq!(
            emit(b"[\t\r\n]").unwrap(),
            vec![node(JsonKind::Array, 0, 5, 1)]
        );
    }

    #[test]
    fn exact_nodes_simple_object() {
        assert_eq!(
            emit(br#"{"a":1}"#).unwrap(),
            vec![
                node(JsonKind::Object, 0, 7, 3),
                node(JsonKind::Key, 2, 3, 1),
                node(JsonKind::Number, 5, 6, 1),
            ]
        );
    }

    #[test]
    fn exact_nodes_nested_mixed() {
        assert_eq!(
            emit(br#"[1,[2,3],{"k":"v"}]"#).unwrap(),
            vec![
                node(JsonKind::Array, 0, 19, 8),
                node(JsonKind::Number, 1, 2, 1),
                node(JsonKind::Array, 3, 8, 3),
                node(JsonKind::Number, 4, 5, 1),
                node(JsonKind::Number, 6, 7, 1),
                node(JsonKind::Object, 9, 18, 3),
                node(JsonKind::Key, 11, 12, 1),
                node(JsonKind::String, 15, 16, 1),
            ]
        );
    }

    #[test]
    fn exact_nodes_whitespace_padded() {
        assert_eq!(
            emit(b" \t\r\n42 \n").unwrap(),
            vec![node(JsonKind::Number, 4, 6, 1)]
        );
        assert_eq!(
            emit(b" {} ").unwrap(),
            vec![node(JsonKind::Object, 1, 3, 1)]
        );
        // Same expectations as the checker's whitespace test.
        assert_eq!(
            emit(br#"{ "a" : [ 1 , 2 ] }"#).unwrap(),
            vec![
                node(JsonKind::Object, 0, 19, 5),
                node(JsonKind::Key, 3, 4, 1),
                node(JsonKind::Array, 8, 17, 3),
                node(JsonKind::Number, 10, 11, 1),
                node(JsonKind::Number, 14, 15, 1),
            ]
        );
        assert_eq!(
            emit(b"[ true ,\tnull ]").unwrap(),
            vec![
                node(JsonKind::Array, 0, 15, 3),
                node(JsonKind::Bool, 2, 6, 1),
                node(JsonKind::Null, 9, 13, 1),
            ]
        );
    }

    #[test]
    fn exact_nodes_escapes() {
        // The 6-byte document `"a\nb"` (backslash-n escape, built at
        // runtime): the content span covers the raw escape bytes.
        let doc = vec![b'"', b'a', b'\\', b'n', b'b', b'"'];
        assert_eq!(emit(&doc).unwrap(), vec![node(JsonKind::String, 1, 5, 1)]);
        // The 8-byte document `"A"` ("A").
        assert_eq!(
            emit(&quoted_uesc("0041")).unwrap(),
            vec![node(JsonKind::String, 1, 7, 1)]
        );
    }

    #[test]
    fn exact_nodes_empty_string_key_and_value() {
        assert_eq!(
            emit(br#""""#).unwrap(),
            vec![node(JsonKind::String, 1, 1, 1)]
        );
        assert_eq!(
            emit(br#"{"":""}"#).unwrap(),
            vec![
                node(JsonKind::Object, 0, 7, 3),
                node(JsonKind::Key, 2, 2, 1),
                node(JsonKind::String, 5, 5, 1),
            ]
        );
    }

    // === 2. round-trip property over a tricky corpus ===

    /// Documents both `serde_json` (default config) and `emit` accept; used
    /// by the round-trip AND differential suites.
    fn agree_ok_corpus() -> Vec<Vec<u8>> {
        let mut docs: Vec<Vec<u8>> = [
            // Every scalar form.
            "0",
            "-0",
            "42",
            "-7",
            "0.5",
            "1e0",
            "1E+9",
            "-1.5e-10",
            "123456789012345678901234567890",
            "true",
            "false",
            "null",
            r#""""#,
            r#""x""#,
            r#""hello world""#,
            // Containers, nested both ways.
            "{}",
            "[]",
            "[[]]",
            "[[[],[]],[]]",
            r#"{"a":{}}"#,
            r#"{"a":[1,[2,[3]],{"b":{"c":null}}]}"#,
            r#"[1,2,3,true,false,null,"s",{"k":"v"},[]]"#,
            // Whitespace variants.
            " 42 ",
            "\r\n[ 1 ,\t2 ]\n",
            "{ }",
            "[ ]",
            r#" { "a" : [ true , null ] , "b" : { } } "#,
            // Empty and near-colliding keys (case-sensitive, no dups).
            r#"{"":0}"#,
            r#"{"a b":1,"A":2,"a":3}"#,
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();

        // One string holding every simple escape, built at runtime.
        let mut all_escapes = vec![b'"'];
        for e in [b'"', b'\\', b'/', b'b', b'f', b'n', b'r', b't'] {
            all_escapes.push(b'\\');
            all_escapes.push(e);
        }
        all_escapes.push(b'"');
        docs.push(all_escapes);

        // Unicode escapes: BMP, mixed-case hex, and a surrogate PAIR.
        docs.push(quoted_uesc("0041"));
        docs.push(quoted_uesc("AbCd"));
        docs.push(quoted_uesc("ffff"));
        let mut pair = vec![b'"'];
        pair.extend_from_slice(&uesc("D83D"));
        pair.extend_from_slice(&uesc("DE00"));
        pair.push(b'"');
        docs.push(pair);

        // Raw multi-byte UTF-8 (é, 😀) composed at runtime.
        let mut uni = String::from(r#"{"k"#);
        uni.push(char::from_u32(0x00E9).unwrap());
        uni.push_str(r#"":"w"#);
        uni.push(char::from_u32(0x1F600).unwrap());
        uni.push_str(r#"rld"}"#);
        docs.push(uni.into_bytes());

        // Deep nesting just inside serde_json's default recursion limit.
        docs.push(nested_arrays(127));
        docs
    }

    /// Documents only `emit` accepts (the documented serde divergences).
    fn emit_only_ok_corpus() -> Vec<Vec<u8>> {
        vec![
            // Lone surrogate escapes: grammar-level check, never decoded.
            quoted_uesc("D800"),
            quoted_uesc("DFFF"),
        ]
    }

    #[test]
    fn roundtrip_tricky_corpus() {
        let mut corpus = agree_ok_corpus();
        corpus.extend(emit_only_ok_corpus());
        for doc in corpus {
            let label = String::from_utf8_lossy(&doc).to_string();
            let nodes = emit(&doc).unwrap_or_else(|e| panic!("emit({label:?}) failed: {e:?}"));
            assert!(!nodes.is_empty(), "{label:?}");
            // Trivially true given emit's built-in final check; assert
            // anyway per spec.
            validate_json(&doc, &nodes)
                .unwrap_or_else(|e| panic!("validate_json({label:?}) failed: {e:?}"));
            // The root spans the JWS-trimmed extent (quotes excluded for a
            // String root), and its subtree is the whole table.
            let jws = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
            let first = doc.iter().position(|b| !jws(b)).unwrap();
            let last = doc.iter().rposition(|b| !jws(b)).unwrap();
            let root = nodes[0];
            let (want_start, want_end) = if root.kind == JsonKind::String {
                (first + 1, last)
            } else {
                (first, last + 1)
            };
            assert_eq!(
                (root.start as usize, root.end as usize),
                (want_start, want_end),
                "root extent of {label:?}"
            );
            assert_eq!(root.size as usize, nodes.len(), "root size of {label:?}");
        }
    }

    #[test]
    fn depth_cap_boundary() {
        // 127 open containers validate; a 128th (and a 129th) is rejected
        // with the checker's exact error.
        assert!(emit(&nested_arrays(127)).is_ok());
        assert_eq!(
            emit(&nested_arrays(128)),
            Err(Error::Table {
                reason: "JSON nesting depth exceeds 127",
            })
        );
        assert_eq!(
            emit(&nested_arrays(129)),
            Err(Error::Table {
                reason: "JSON nesting depth exceeds 127",
            })
        );
    }

    #[test]
    fn emit_rejects_invalid_documents_with_checker_positions() {
        assert_json_err_at(emit(b""), 0, "expected a JSON value");
        assert_json_err_at(emit(b" \t\n"), 3, "expected a JSON value");
        assert_json_err_at(emit(b"{} x"), 3, "data after root value");
        assert_json_err_at(emit(b"{}{}"), 2, "data after root value");
        assert_json_err_at(emit(b"[1,]"), 3, "unexpected byte at value position");
        assert_json_err_at(emit(br#"{'a':1}"#), 1, "expected object key");
        assert_json_err_at(emit(b"-"), 1, "invalid number");
        assert_json_err_at(emit(b"+1"), 0, "unexpected byte at value position");
        assert_json_err_at(emit(b"01"), 1, "data after root value");
        assert_json_err_at(emit(b"1."), 1, "data after root value");
        assert_json_err_at(emit(b"tru"), 0, "invalid literal");
        assert_json_err_at(emit(b"True"), 0, "unexpected byte at value position");
        assert_json_err_at(emit(br#"{"a" 1}"#), 5, "expected colon");
        assert_json_err_at(emit(b"[1 2]"), 3, "expected comma or closing bracket");
        assert_json_err_at(emit(b"[1"), 2, "unterminated container");
        assert_json_err_at(emit(b"{"), 1, "expected object key");
        assert_json_err_at(emit(b"["), 1, "expected a JSON value");
        // Checker-only policies surface through the final self-check.
        assert_json_err_at(emit(br#"{"a":1,"a":2}"#), 8, "duplicate object key");
        assert_json_err_at(emit(b"\"\xFF\""), 1, "body is not valid UTF-8");
    }

    #[test]
    fn emit_never_panics_on_prefixes_and_mutations() {
        let base: &[u8] = br#"{"a":[1,{"b":"x\n"},-2.5e8],"c":null,"d":[true,false,[]]}"#;
        assert!(emit(base).is_ok());
        // Every strict prefix is incomplete, hence rejected — panic-free.
        for i in 0..base.len() {
            assert!(emit(&base[..i]).is_err(), "prefix of length {i}");
        }
        // Single-byte corruption: emit never panics, and anything it still
        // accepts passes the checker.
        for i in 0..base.len() {
            for b in [
                0x00, 0x1F, b'"', b'\\', b'{', b'}', b'[', b']', b',', b':', b'9', b' ', 0xFF,
            ] {
                let mut doc = base.to_vec();
                doc[i] = b;
                if let Ok(nodes) = emit(&doc) {
                    validate_json(&doc, &nodes).unwrap();
                }
            }
        }
    }

    // === 3. differential vs serde_json ===

    fn serde_accepts(doc: &[u8]) -> bool {
        serde_json::from_slice::<serde_json::Value>(doc).is_ok()
    }

    #[test]
    fn differential_agreement_on_valid_documents() {
        for doc in agree_ok_corpus() {
            let label = String::from_utf8_lossy(&doc).to_string();
            assert!(serde_accepts(&doc), "serde rejected {label:?}");
            assert!(emit(&doc).is_ok(), "emit rejected {label:?}");
        }
    }

    #[test]
    fn differential_agreement_on_invalid_documents() {
        let mut docs: Vec<Vec<u8>> = [
            "{} x",
            "[1,]",
            "{'a':1}",
            "+1",
            "01",
            "1.",
            ".5",
            "nan",
            "Infinity",
            "//c",
            "-",
            "",
            " \t ",
            "{",
            "[",
            "[1",
            "[1 2]",
            "[1,,2]",
            "tru",
            "True",
            "NULL",
            "falsey",
            "{}{}",
            "1 2",
            "--1",
            "1e",
            "1e+",
            "0x5",
            ",",
            "]",
            "}",
            ":",
            r#"{"a" 1}"#,
            r#"{"a":}"#,
            r#"{"a":1,}"#,
            r#"{"a":1 "b":2}"#,
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        // Raw control bytes in a string (LF and 0x01).
        docs.push(vec![b'"', b'a', b'\n', b'b', b'"']);
        docs.push(vec![b'"', b'a', 0x01, b'b', b'"']);
        // Truncated / malformed escapes, built at runtime.
        let mut d = vec![b'"', b'\\', b'u', b'0', b'0', b'"'];
        docs.push(d.clone());
        d.truncate(3); // `"\u`
        docs.push(d);
        docs.push(vec![b'"', b'a', b'b', b'c', b'\\']);
        docs.push(quoted_uesc("ZZZZ"));
        docs.push(vec![b'"', b'a', b'\\', b'q', b'"']);
        // Unterminated string.
        docs.push(vec![b'"', b'a', b'b']);

        for doc in docs {
            let label = String::from_utf8_lossy(&doc).to_string();
            assert!(!serde_accepts(&doc), "serde accepted {label:?}");
            assert!(emit(&doc).is_err(), "emit accepted {label:?}");
        }
    }

    #[test]
    fn differential_documented_divergences() {
        // 1. Duplicate keys: serde accepts (last wins), emit rejects — including
        //    escape-alias duplicates serde cannot see.
        let dup = br#"{"a":1,"a":2}"#;
        assert!(serde_accepts(dup));
        assert_json_err_at(emit(dup), 8, "duplicate object key");
        let mut alias = Vec::from(&br#"{"a":1,""#[..]);
        alias.extend_from_slice(&uesc("0061"));
        alias.extend_from_slice(br#"":2}"#);
        assert!(serde_accepts(&alias));
        assert_json_err_at(emit(&alias), 8, "duplicate object key");

        // 2. Lone surrogate escapes: serde rejects (it decodes strings), emit accepts
        //    (grammar-level check per checker policy).
        for hex in ["D800", "DFFF"] {
            let doc = quoted_uesc(hex);
            assert!(!serde_accepts(&doc), "serde accepted lone {hex}");
            assert!(emit(&doc).is_ok(), "emit rejected lone {hex}");
        }

        // 3. Depth: NO divergence. Our cap matches serde_json's default recursion limit
        //    (127), so the two agree at every depth — 127 both accept, 128 and beyond
        //    both reject.
        let d127 = nested_arrays(127);
        assert!(serde_accepts(&d127) && emit(&d127).is_ok());
        let d128 = nested_arrays(128);
        assert!(!serde_accepts(&d128) && emit(&d128).is_err());
        let d129 = nested_arrays(129);
        assert!(!serde_accepts(&d129) && emit(&d129).is_err());
    }

    // === 4. real-fixture bodies ===

    /// Loads a fixture, splits the head at CRLFCRLF, and de-chunks the body
    /// with the validator's own chunk walker.
    fn dechunked_fixture_body(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name);
        let recv = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()));
        let head_end = recv
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("fixture has no CRLFCRLF")
            + 4;
        walk_chunks(&recv, head_end, 0)
            .expect("fixture de-chunk")
            .decoded
    }

    /// Returns `true` if some member key `key` has the string value `value`
    /// anywhere in the table (a Key node is immediately followed by its
    /// value's subtree).
    fn has_string_member(body: &[u8], nodes: &[JsonNode], key: &[u8], value: &[u8]) -> bool {
        let span = |n: &JsonNode| &body[n.start as usize..n.end as usize];
        nodes.windows(2).any(|w| {
            w[0].kind == JsonKind::Key
                && span(&w[0]) == key
                && w[1].kind == JsonKind::String
                && span(&w[1]) == value
        })
    }

    #[test]
    fn fixture_pokeapi_ditto_body_emits_and_validates() {
        let body = dechunked_fixture_body("pokeapi_ditto.recv.bin");
        let nodes = emit(&body).expect("emit pokeapi body");
        validate_json(&body, &nodes).expect("checker accepts pokeapi table");
        assert!(nodes.len() > 1000, "node count {}", nodes.len());
        assert_eq!(nodes[0].kind, JsonKind::Object);
        assert_eq!(nodes[0].size as usize, nodes.len());
        assert!(has_string_member(&body, &nodes, b"name", b"ditto"));
    }

    #[test]
    fn fixture_swapi_films_body_emits_and_validates() {
        let body = dechunked_fixture_body("swapi_films.recv.bin");
        let nodes = emit(&body).expect("emit swapi body");
        validate_json(&body, &nodes).expect("checker accepts swapi table");
        assert!(nodes.len() > 100, "node count {}", nodes.len());
        assert_eq!(nodes[0].kind, JsonKind::Array);
        assert_eq!(nodes[0].size as usize, nodes.len());
        assert!(has_string_member(&body, &nodes, b"title", b"A New Hope"));
    }
}
