use std::ops::Range;

use pest::{Parser, iterators::Pair as PestPair};

use super::types::{self, JsonValue, KeyValue};
use crate::{ParseError, Store, View};

#[derive(pest_derive::Parser)]
#[grammar = "json/json.pest"]
struct JsonParser;

/// Parse a JSON value.
///
/// # Example
///
/// ```
/// use spansy::json::parse;
///
/// // Parse borrowed
/// let value = parse(b"{\"foo\": 42}").unwrap();
/// assert_eq!(value.get("foo").unwrap(), "42");
/// ```
pub fn parse<S: Store>(src: impl Into<View<S>>) -> Result<JsonValue<S>, ParseError> {
    let view: View<S, str> = src.into().try_into()?;
    let data = view.as_str();

    let value = JsonParser::parse(Rule::value, &data)
        .map_err(ParseError::from_pest)?
        .next()
        .ok_or_else(|| ParseError("no json value is present in source".to_string()))?;

    let consumed = value.as_str().len();
    if consumed != data.len() {
        let (_, trailing) = data.split_at(consumed);
        let mut chars = trailing.chars();
        let preview: String = chars.by_ref().take(50).collect();
        let suffix = if chars.next().is_some() { "..." } else { "" };
        return Err(ParseError(format!(
            "trailing characters are present in source: \"{preview}{suffix}\""
        )));
    }

    Ok(JsonValue::from_pair(&view, &data, value))
}

/// Helper to get the range of a string within the data.
fn get_range(data: &str, s: &str) -> Range<usize> {
    let start = s.as_ptr() as usize - data.as_ptr() as usize;
    start..start + s.len()
}

impl<S: Store> JsonValue<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        match pair.as_rule() {
            Rule::object => Self::Object(types::Object::from_pair(view, data, pair)),
            Rule::array => Self::Array(types::Array::from_pair(view, data, pair)),
            Rule::string => Self::String(types::String::from_pair(view, data, pair)),
            Rule::number => Self::Number(types::Number::from_pair(view, data, pair)),
            Rule::bool => Self::Bool(types::Bool::from_pair(view, data, pair)),
            Rule::null => Self::Null(types::Null::from_pair(view, data, pair)),
            rule => unreachable!("unexpected matched rule: {:?}", rule),
        }
    }
}

impl<S: Store> types::JsonKey<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::string));
        let range = get_range(data, pair.as_str());
        Self {
            view: view.select(range).expect("range should be valid"),
        }
    }
}

impl<S: Store> types::Number<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::number));
        let range = get_range(data, pair.as_str());
        Self {
            view: view.select(range).expect("range should be valid"),
        }
    }
}

impl<S: Store> types::Bool<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::bool));
        let range = get_range(data, pair.as_str());
        Self {
            view: view.select(range).expect("range should be valid"),
        }
    }
}

impl<S: Store> types::Null<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::null));
        let range = get_range(data, pair.as_str());
        Self {
            view: view.select(range).expect("range should be valid"),
        }
    }
}

impl<S: Store> types::String<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::string));
        let range = get_range(data, pair.as_str());
        Self {
            view: view.select(range).expect("range should be valid"),
        }
    }
}

impl<S: Store> KeyValue<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::pair));

        let range = get_range(data, pair.as_str().trim_end());

        let mut pairs = pair.into_inner();

        let key = pairs.next().expect("key should be present");
        let value = pairs.next().expect("value should be present");

        Self {
            view: view.select(range).expect("range should be valid"),
            key: types::JsonKey::from_pair(view, data, key),
            value: JsonValue::from_pair(view, data, value),
        }
    }
}

impl<S: Store> types::Object<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::object));

        let range = get_range(data, pair.as_str());

        Self {
            view: view.select(range).expect("range should be valid"),
            elems: pair
                .into_inner()
                .map(|pair| KeyValue::from_pair(view, data, pair))
                .collect(),
        }
    }
}

impl<S: Store> types::Array<S> {
    fn from_pair(view: &View<S, str>, data: &str, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::array));

        let range = get_range(data, pair.as_str());

        Self {
            view: view.select(range).expect("range should be valid"),
            elems: pair
                .into_inner()
                .map(|pair| JsonValue::from_pair(view, data, pair))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_spanner() {
        let src =
            br#"{"foo": "bar", "baz": 123, "quux": { "a": "b", "c": "d" }, "arr": [1, 2, 3]}"#;

        let value = parse(src).unwrap();

        assert_eq!(value.get("foo").unwrap(), "bar");
        assert_eq!(value.get("baz").unwrap(), "123");
        assert_eq!(value.get("quux.a").unwrap(), "b");
        assert_eq!(value.get("arr").unwrap(), "[1, 2, 3]");
    }

    #[test]
    fn test_json_borrowed() {
        let src: &[u8] = br#"{"foo": "bar"}"#;

        let value = parse(src).unwrap();

        assert_eq!(value.get("foo").unwrap(), "bar");
        assert!(value.view().indices().len_ranges() <= 1);
    }

    #[test]
    fn test_err_leading_characters() {
        let src = b" {\"foo\": \"bar\"}";
        assert!(parse(src).is_err());
    }

    #[test]
    fn test_err_trailing_characters() {
        let src = b"{\"foo\": \"bar\"} ";
        assert_eq!(
            parse(src).err().unwrap().to_string(),
            "parsing error: trailing characters are present in source: \" \""
        );
    }

    #[test]
    fn test_unicode_strings() {
        let src = r#"{"key": "日本語", "emoji": "🎉"}"#.as_bytes();
        let value = parse(src).unwrap();
        assert_eq!(value.get("key").unwrap(), "日本語");
        assert_eq!(value.get("emoji").unwrap(), "🎉");
    }

    #[test]
    fn test_escaped_characters() {
        let src = br#"{"a": "line1\nline2", "b": "tab\there", "c": "quote\"here"}"#;
        let value = parse(src).unwrap();
        // Spans contain raw content without quotes
        assert_eq!(
            value.get("a").unwrap().view().as_str().as_ref(),
            r#"line1\nline2"#
        );
        assert_eq!(
            value.get("b").unwrap().view().as_str().as_ref(),
            r#"tab\there"#
        );
        assert_eq!(
            value.get("c").unwrap().view().as_str().as_ref(),
            r#"quote\"here"#
        );
    }

    #[test]
    fn test_numbers() {
        let src = br#"{"neg": -42, "float": 3.14, "exp": 1e10, "neg_exp": 2.5e-3}"#;
        let value = parse(src).unwrap();
        assert_eq!(value.get("neg").unwrap(), "-42");
        assert_eq!(value.get("float").unwrap(), "3.14");
        assert_eq!(value.get("exp").unwrap(), "1e10");
        assert_eq!(value.get("neg_exp").unwrap(), "2.5e-3");
    }

    #[test]
    fn test_whitespace_variations() {
        let src = b"{\n\t\"a\"\t:\n1\n,\t\"b\" : 2\n}";
        let value = parse(src).unwrap();
        assert_eq!(value.get("a").unwrap(), "1");
        assert_eq!(value.get("b").unwrap(), "2");
    }

    #[test]
    fn test_empty_structures() {
        let src = br#"{"obj": {}, "arr": [], "nested": {"empty": {}}}"#;
        let value = parse(src).unwrap();
        assert_eq!(value.get("obj").unwrap(), "{}");
        assert_eq!(value.get("arr").unwrap(), "[]");
        assert_eq!(value.get("nested.empty").unwrap(), "{}");
    }

    #[test]
    fn test_deeply_nested() {
        let src = br#"{"a": {"b": {"c": {"d": {"e": {"f": 42}}}}}}"#;
        let value = parse(src).unwrap();
        assert_eq!(value.get("a.b.c.d.e.f").unwrap(), "42");
    }

    #[test]
    fn test_array_access() {
        let src = br#"[1, [2, [3, [4, 5]]]]"#;
        let value = parse(src).unwrap();
        assert_eq!(value.get("0").unwrap(), "1");
        assert_eq!(value.get("1.0").unwrap(), "2");
        assert_eq!(value.get("1.1.0").unwrap(), "3");
        assert_eq!(value.get("1.1.1.0").unwrap(), "4");
        assert_eq!(value.get("1.1.1.1").unwrap(), "5");
    }

    #[test]
    fn test_booleans_and_null() {
        let src = br#"{"t": true, "f": false, "n": null}"#;
        let value = parse(src).unwrap();
        assert_eq!(value.get("t").unwrap(), "true");
        assert_eq!(value.get("f").unwrap(), "false");
        assert_eq!(value.get("n").unwrap(), "null");
    }

    #[test]
    fn test_non_contiguous_json() {
        use crate::http::{BodyContent, parse_response};

        // JSON split across two chunks: {"key": + "value"}
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\
            Content-Type: application/json\r\n\r\n\
            8\r\n{\"key\": \r\n\
            8\r\n\"value\"}\r\n\
            0\r\n\r\n";

        let res = parse_response(src).unwrap();
        let body = res.body.unwrap();

        // Body has 2 chunks
        assert_eq!(body.chunks.as_ref().unwrap().len(), 2);

        let BodyContent::Json(value) = body.content else {
            panic!("body should be json");
        };

        // Verify we can access parsed JSON correctly
        assert_eq!(value.get("key").unwrap(), "value");

        // Verify the full JSON view is non-contiguous (spans 2 chunks)
        let json_view = value.view();
        assert!(json_view.indices().len_ranges() > 1);
    }
}
