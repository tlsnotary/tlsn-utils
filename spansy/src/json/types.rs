use std::ops::{Index, Range};

use rangeset::{Difference, RangeSet, ToRangeSet};

use crate::{Span, Spanned};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A JSON value.
pub enum JsonValue {
    /// A null value.
    Null(Null),
    /// A boolean value.
    Bool(Bool),
    /// A number value.
    Number(Number),
    /// A string value.
    String(String),
    /// A redacted value.
    Redacted(Redacted),
    /// An array value.
    Array(Array),
    /// An object value.
    Object(Object),
}

impl JsonValue {
    /// Returns the span corresponding to the value.
    pub fn into_span(self) -> Span<str> {
        match self {
            JsonValue::Null(v) => v.0,
            JsonValue::Bool(v) => v.0,
            JsonValue::Number(v) => v.0,
            JsonValue::String(v) => v.0,
            JsonValue::Redacted(v) => v.0,
            JsonValue::Array(v) => v.span,
            JsonValue::Object(v) => v.span,
        }
    }

    /// Shifts the span range by the given offset.
    pub fn offset(&mut self, offset: usize) {
        match self {
            JsonValue::Null(v) => v.0.offset(offset),
            JsonValue::Bool(v) => v.0.offset(offset),
            JsonValue::Number(v) => v.0.offset(offset),
            JsonValue::String(v) => v.0.offset(offset),
            JsonValue::Redacted(v) => v.0.offset(offset),
            JsonValue::Array(v) => {
                v.span.offset(offset);
                v.elems.iter_mut().for_each(|v| v.offset(offset))
            }
            JsonValue::Object(v) => {
                v.span.offset(offset);
                v.elems.iter_mut().for_each(|kv| {
                    kv.span.offset(offset);
                    kv.key.offset(offset);
                    kv.value.offset(offset);
                })
            }
        }
    }
}

impl JsonValue {
    /// Get a reference to the value using the given path.
    ///
    /// # Example
    ///
    /// ```
    /// use spansy::json::parse_str;
    /// use spansy::Spanned;
    ///
    /// let src = "{\"foo\": {\"bar\": [42, 14]}}";
    ///
    /// let value = parse_str(src).unwrap();
    ///
    /// assert_eq!(value.get("foo.bar.1").unwrap().span(), "14");
    /// ```
    pub fn get(&self, path: &str) -> Option<&JsonValue> {
        match self {
            JsonValue::Null(_) => None,
            JsonValue::Bool(_) => None,
            JsonValue::Number(_) => None,
            JsonValue::String(_) => None,
            JsonValue::Redacted(_) => None,
            JsonValue::Array(v) => v.get(path),
            JsonValue::Object(v) => v.get(path),
        }
    }
}

impl Spanned<str> for JsonValue {
    fn span(&self) -> &Span<str> {
        match self {
            JsonValue::Null(v) => v.span(),
            JsonValue::Bool(v) => v.span(),
            JsonValue::Number(v) => v.span(),
            JsonValue::String(v) => v.span(),
            JsonValue::Redacted(v) => v.span(),
            JsonValue::Array(v) => v.span(),
            JsonValue::Object(v) => v.span(),
        }
    }
}

impl AsRef<str> for JsonValue {
    fn as_ref(&self) -> &str {
        match self {
            JsonValue::Null(v) => v.as_ref(),
            JsonValue::Bool(v) => v.as_ref(),
            JsonValue::Number(v) => v.as_ref(),
            JsonValue::String(v) => v.as_ref(),
            JsonValue::Redacted(v) => v.as_ref(),
            JsonValue::Array(v) => v.as_ref(),
            JsonValue::Object(v) => v.as_ref(),
        }
    }
}

impl AsRef<[u8]> for JsonValue {
    fn as_ref(&self) -> &[u8] {
        match self {
            JsonValue::Null(v) => v.as_ref(),
            JsonValue::Bool(v) => v.as_ref(),
            JsonValue::Number(v) => v.as_ref(),
            JsonValue::String(v) => v.as_ref(),
            JsonValue::Redacted(v) => v.as_ref(),
            JsonValue::Array(v) => v.as_ref(),
            JsonValue::Object(v) => v.as_ref(),
        }
    }
}

impl AsRef<RangeSet<usize>> for JsonValue {
    fn as_ref(&self) -> &RangeSet<usize> {
        match self {
            JsonValue::Null(v) => v.as_ref(),
            JsonValue::Bool(v) => v.as_ref(),
            JsonValue::Number(v) => v.as_ref(),
            JsonValue::String(v) => v.as_ref(),
            JsonValue::Redacted(v) => v.as_ref(),
            JsonValue::Array(v) => v.as_ref(),
            JsonValue::Object(v) => v.as_ref(),
        }
    }
}

impl ToRangeSet<usize> for JsonValue {
    fn to_range_set(&self) -> RangeSet<usize> {
        match self {
            JsonValue::Null(v) => v.to_range_set(),
            JsonValue::Bool(v) => v.to_range_set(),
            JsonValue::Number(v) => v.to_range_set(),
            JsonValue::String(v) => v.to_range_set(),
            JsonValue::Redacted(v) => v.to_range_set(),
            JsonValue::Array(v) => v.to_range_set(),
            JsonValue::Object(v) => v.to_range_set(),
        }
    }
}

impl PartialEq<str> for JsonValue {
    fn eq(&self, other: &str) -> bool {
        match self {
            JsonValue::Null(v) => v == other,
            JsonValue::Bool(v) => v == other,
            JsonValue::Number(v) => v == other,
            JsonValue::String(v) => v == other,
            JsonValue::Redacted(v) => v == other,
            JsonValue::Array(v) => v == other,
            JsonValue::Object(v) => v == other,
        }
    }
}

impl PartialEq<JsonValue> for str {
    fn eq(&self, other: &JsonValue) -> bool {
        other == self
    }
}

impl PartialEq<&str> for JsonValue {
    fn eq(&self, other: &&str) -> bool {
        match self {
            JsonValue::Null(v) => v == other,
            JsonValue::Bool(v) => v == other,
            JsonValue::Number(v) => v == other,
            JsonValue::String(v) => v == other,
            JsonValue::Redacted(v) => v == other,
            JsonValue::Array(v) => v == other,
            JsonValue::Object(v) => v == other,
        }
    }
}

impl PartialEq<JsonValue> for &str {
    fn eq(&self, other: &JsonValue) -> bool {
        other == self
    }
}

/// A key value pair in a JSON object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyValue {
    pub(crate) span: Span<str>,

    /// The key of the pair.
    pub key: JsonKey,
    /// The value of the pair.
    pub value: JsonValue,
}

impl KeyValue {
    /// Returns the indices of the key value pair, excluding the value.
    pub fn without_value(&self) -> RangeSet<usize> {
        self.span.indices.difference(&self.value.span().indices)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A key in a JSON object.
pub struct JsonKey(pub(crate) Span<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A null value.
pub struct Null(pub(crate) Span<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A boolean value.
pub struct Bool(pub(crate) Span<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A number value.
pub struct Number(pub(crate) Span<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A JSON string value.
///
/// This span does not capture the quotation marks around the string.
pub struct String(pub(crate) Span<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A redacted value.
pub struct Redacted(pub(crate) Span<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// An array value.
pub struct Array {
    pub(crate) span: Span<str>,
    /// The elements of the array.
    pub elems: Vec<JsonValue>,
}

impl Array {
    /// Get a reference to the value using the given path.
    pub fn get(&self, path: &str) -> Option<&JsonValue> {
        let mut path_iter = path.split('.');

        let key = path_iter.next()?;
        let idx = key.parse::<usize>().ok()?;

        let value = self.elems.get(idx)?;

        if path_iter.next().is_some() {
            value.get(&path[key.len() + 1..])
        } else {
            Some(value)
        }
    }

    /// Returns the indices of the array, excluding the values and separators.
    pub fn without_values(&self) -> RangeSet<usize> {
        let start = self
            .span
            .indices
            .min()
            .expect("array has at least brackets");
        let end = self
            .span
            .indices
            .max()
            .expect("array has at least brackets");

        RangeSet::from([start..start + 1, end..end + 1])
    }
}

impl Index<usize> for Array {
    type Output = JsonValue;

    /// Returns the value at the given index of the array.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    fn index(&self, index: usize) -> &Self::Output {
        self.elems.get(index).expect("index is in bounds")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// A JSON object value.
pub struct Object {
    pub(crate) span: Span<str>,
    /// The key value pairs of the object.
    pub elems: Vec<KeyValue>,
}

impl Object {
    /// Get a reference to the value using the given path.
    pub fn get(&self, path: &str) -> Option<&JsonValue> {
        let mut path_iter = path.split('.');

        let key = path_iter.next()?;

        let KeyValue { value, .. } = self.elems.iter().find(|kv| kv.key == key)?;

        if path_iter.next().is_some() {
            value.get(&path[key.len() + 1..])
        } else {
            Some(value)
        }
    }

    /// Returns the indices of the object, excluding the key value pairs.
    pub fn without_pairs(&self) -> RangeSet<usize> {
        let mut indices = self.span.indices.clone();
        for kv in &self.elems {
            indices = indices.difference(&kv.span.indices);
        }
        indices
    }
}

impl Index<&str> for Object {
    type Output = JsonValue;

    /// Returns the value at the given key of the object.
    ///
    /// # Panics
    ///
    /// Panics if the key is not present.
    fn index(&self, key: &str) -> &Self::Output {
        self.get(key).expect("key is present")
    }
}

macro_rules! impl_type {
    ($ty:ident, $span:tt) => {
        impl $ty {
            /// Returns the span corresponding to the value.
            pub fn into_span(self) -> Span<str> {
                self.$span
            }

            /// Shifts the span range by the given offset.
            pub fn offset(&mut self, offset: usize) {
                self.$span.offset(offset);
            }
        }

        impl Spanned<str> for $ty {
            fn span(&self) -> &Span<str> {
                &self.$span
            }
        }

        impl AsRef<str> for $ty {
            fn as_ref(&self) -> &str {
                self.$span.as_ref()
            }
        }

        impl AsRef<[u8]> for $ty {
            fn as_ref(&self) -> &[u8] {
                self.$span.as_ref()
            }
        }

        impl AsRef<RangeSet<usize>> for $ty {
            fn as_ref(&self) -> &RangeSet<usize> {
                &self.$span.indices
            }
        }

        impl ToRangeSet<usize> for $ty {
            fn to_range_set(&self) -> RangeSet<usize> {
                self.$span.indices.clone()
            }
        }

        impl PartialEq<str> for $ty {
            fn eq(&self, other: &str) -> bool {
                self.$span == other
            }
        }

        impl PartialEq<$ty> for str {
            fn eq(&self, other: &$ty) -> bool {
                self == &other.$span
            }
        }

        impl PartialEq<&str> for $ty {
            fn eq(&self, other: &&str) -> bool {
                self.$span == *other
            }
        }

        impl PartialEq<$ty> for &str {
            fn eq(&self, other: &$ty) -> bool {
                *self == &other.$span
            }
        }

        impl PartialEq<Range<usize>> for $ty {
            fn eq(&self, other: &Range<usize>) -> bool {
                &self.$span == other
            }
        }

        impl PartialEq<$ty> for Range<usize> {
            fn eq(&self, other: &$ty) -> bool {
                self == &other.$span
            }
        }

        impl PartialEq<Span<str>> for $ty {
            fn eq(&self, other: &Span<str>) -> bool {
                &self.$span == other
            }
        }

        impl PartialEq<$ty> for Span<str> {
            fn eq(&self, other: &$ty) -> bool {
                self == &other.$span
            }
        }
    };
}

impl_type!(JsonKey, 0);
impl_type!(Null, 0);
impl_type!(Bool, 0);
impl_type!(Number, 0);
impl_type!(String, 0);
impl_type!(Redacted, 0);
impl_type!(Array, span);
impl_type!(Object, span);
impl_type!(KeyValue, span);

#[cfg(test)]
mod tests {
    use rangeset::IndexRanges;

    use crate::json::parse_str;

    use super::*;

    #[test]
    fn test_obj_index() {
        let src = "{\"foo\": \"bar\"}";

        let value = parse_str(src).unwrap();

        assert_eq!(value.get("foo").unwrap(), "bar");
    }

    #[test]
    fn test_array_index() {
        let src = "{\"foo\": [42, 14]}";

        let value = parse_str(src).unwrap();

        assert_eq!(value.get("foo.1").unwrap(), "14");
    }

    #[test]
    fn test_nested_index() {
        let src = "{\"foo\": {\"bar\": [42, 14]}}";

        let value = parse_str(src).unwrap();

        assert_eq!(value.get("foo.bar.1").unwrap(), "14");
    }

    #[test]
    fn test_key_value_without_value() {
        let src = "{\"foo\": \"bar\"\n}";

        let JsonValue::Object(value) = parse_str(src).unwrap() else {
            panic!("expected object");
        };

        let indices = value.elems[0].without_value();

        assert_eq!(src.index_ranges(&indices), "\"foo\": \"\"");
    }

    #[test]
    fn test_array_without_values() {
        let src = "[42, 14]";

        let JsonValue::Array(value) = parse_str(src).unwrap() else {
            panic!("expected object");
        };

        let indices = value.without_values();

        assert_eq!(src.index_ranges(&indices), "[]");
    }

    #[test]
    fn test_object_without_pairs() {
        let src = "{\"foo\": \"bar\"\n}";

        let JsonValue::Object(value) = parse_str(src).unwrap() else {
            panic!("expected object");
        };

        let indices = value.without_pairs();

        assert_eq!(src.index_ranges(&indices), "{\n}");
    }

    #[test]
    fn test_redacted_value() {
        let src = r#"\0\0\0"#;
        let value = parse_str(src).unwrap();
        assert_eq!(value.span(), r#"\0\0\0"#);
    }

    #[test]
    fn test_redacted_value_in_object() {
        let src = r#"{"foo": \0\0\0}"#;
        let value = parse_str(src).unwrap();
        assert_eq!(value.get("foo").unwrap().span(), r#"\0\0\0"#);
    }

    #[test]
    fn test_redacted_value_in_array() {
        let src = r#"[1, \0\0\0]"#;
        let value = parse_str(src).unwrap();
        assert_eq!(value.get("1").unwrap().span(), r#"\0\0\0"#);
    }
    
    
}
