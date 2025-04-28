use bytes::Bytes;
use pest::{iterators::Pair as PestPair, Parser};
use types::KeyValue;

use super::types::{self, JsonValue};

use crate::{ParseError, Span};

#[derive(pest_derive::Parser)]
#[grammar = "json/json.pest"]
struct JsonParser;

/// Parse a JSON value from a source string.
pub fn parse_str(src: &str) -> Result<JsonValue, ParseError> {
    let src = Bytes::copy_from_slice(src.as_bytes());

    // # Safety
    // `src` was passed as a string slice, so it is guaranteed to be valid UTF-8.
    let src_str = unsafe { std::str::from_utf8_unchecked(src.as_ref()) };

    let value = JsonParser::parse(Rule::value, src_str)?
        .next()
        .ok_or_else(|| ParseError("no json value is present in source".to_string()))?;

    // Since json.pest grammar prohibits leading characters but allows trailing
    // characters, we prohibit trailing characters here.
    if value.as_str().len() != src.len() {
        return Err(ParseError(
            "trailing characters are present in source".to_string(),
        ));
    }

    Ok(JsonValue::from_pair(src.clone(), value))
}

/// Parse a JSON value from a byte slice.
pub fn parse_slice(src: &[u8]) -> Result<JsonValue, ParseError> {
    let src = Bytes::copy_from_slice(src);
    parse(src)
}

/// Parse a JSON value from source bytes.
pub fn parse(src: Bytes) -> Result<JsonValue, ParseError> {
    let src_str = std::str::from_utf8(&src)?;

    let value = JsonParser::parse(Rule::value, src_str)?
        .next()
        .ok_or_else(|| ParseError("no json value is present in source".to_string()))?;

    // Since json.pest grammar prohibits leading characters but allows trailing
    // characters, we prohibit trailing characters here.
    if value.as_str().len() != src.len() {
        return Err(ParseError(
            "trailing characters are present in source".to_string(),
        ));
    }

    Ok(JsonValue::from_pair(src.clone(), value))
}

macro_rules! impl_from_pair {
    ($ty:ty, $rule:ident) => {
        impl $ty {
            fn from_pair(src: Bytes, pair: PestPair<'_, Rule>) -> Self {
                assert!(matches!(pair.as_rule(), Rule::$rule));

                Self(Span::new_from_str(src, pair.as_str()))
            }
        }
    };
}

impl_from_pair!(types::JsonKey, string);
impl_from_pair!(types::Number, number);
impl_from_pair!(types::Bool, bool);
impl_from_pair!(types::Null, null);
impl_from_pair!(types::String, string);
impl_from_pair!(types::Redacted, redacted);

impl types::KeyValue {
    fn from_pair(src: Bytes, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::pair));

        let span = Span::new_from_str(src.clone(), pair.as_str().trim_end());

        let mut pairs = pair.into_inner();

        let key = pairs.next().expect("key is present");
        let value = pairs.next().expect("value is present");

        Self {
            span,
            key: types::JsonKey::from_pair(src.clone(), key),
            value: types::JsonValue::from_pair(src.clone(), value),
        }
    }
}

impl types::Object {
    fn from_pair(src: Bytes, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::object));

        Self {
            span: Span::new_from_str(src.clone(), pair.as_str()),
            elems: pair
                .into_inner()
                .map(|pair| KeyValue::from_pair(src.clone(), pair))
                .collect(),
        }
    }
}

impl types::Array {
    fn from_pair(src: Bytes, pair: PestPair<'_, Rule>) -> Self {
        assert!(matches!(pair.as_rule(), Rule::array));

        Self {
            span: Span::new_from_str(src.clone(), pair.as_str()),
            elems: pair
                .into_inner()
                .map(|pair| types::JsonValue::from_pair(src.clone(), pair))
                .collect(),
        }
    }
}

impl types::JsonValue {
    fn from_pair(src: Bytes, pair: PestPair<'_, Rule>) -> Self {
        match pair.as_rule() {
            Rule::object => Self::Object(types::Object::from_pair(src, pair)),
            Rule::array => Self::Array(types::Array::from_pair(src, pair)),
            Rule::string => Self::String(types::String::from_pair(src, pair)),
            Rule::redacted => Self::Redacted(types::Redacted::from_pair(src, pair)),
            Rule::number => Self::Number(types::Number::from_pair(src, pair)),
            Rule::bool => Self::Bool(types::Bool::from_pair(src, pair)),
            Rule::null => Self::Null(types::Null::from_pair(src, pair)),
            rule => unreachable!("unexpected matched rule: {:?}", rule),
        }
    }
}
#[cfg(test)]
mod tests {
    use crate::Spanned;

    use super::*;

    #[test]
    fn test_json_spanner() {
        let src = r#"{"foo": "bar", "baz": 123, "quux": { "a": "b", "c": "d" }, "arr": [1, 2, 3], "r_one": \0, "r_many": \0\0\0\0 }"#;

        let value = parse_str(src).unwrap();

        assert_eq!(value.get("foo").unwrap().span(), "bar");
        assert_eq!(value.get("baz").unwrap().span(), "123");
        assert_eq!(value.get("quux.a").unwrap().span(), "b");
        assert_eq!(value.get("arr").unwrap().span(), "[1, 2, 3]");
        assert_eq!(value.get("r_one").unwrap().span(), r#"\0"#);
        assert_eq!(value.get("r_many").unwrap().span(), r#"\0\0\0\0"#);
    }

    #[test]
    fn test_err_leading_characters() {
        let src = " {\"foo\": \"bar\"}";
        assert!(parse_str(src).is_err());
    }

    #[test]
    fn test_err_trailing_characters() {
        let src = "{\"foo\": \"bar\"} ";
        assert_eq!(
            parse_str(src).err().unwrap().to_string(),
            "parsing error: trailing characters are present in source"
        );
    }

    #[test]
    fn test_redaction_in_key() {
        let src = r#"{"\0": "bar"}"#;
        assert!(parse_str(src).is_err());
    }

    #[test]
    fn test_redaction_in_value() {
        let src = r#"{"foo": 1\0\0\0}"#;
        assert!(parse_str(src).is_err());
    }
}
