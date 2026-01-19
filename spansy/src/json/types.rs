use std::{borrow::Cow, ops::Index};

use bytes::Bytes;
use rangeset::{
    iter::{IntoRangeIterator, RangeIterator},
    ops::Set,
    set::{RangeIter, RangeSet},
};

use crate::{Span, Store, View};

/// A JSON document.
#[derive(Debug, Clone)]
pub struct Document<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
    /// The root value of the document.
    pub root: JsonValue<S>,
}

impl<S: Store> Document<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Get a reference to the value using the given path.
    ///
    /// # Example
    ///
    /// ```
    /// use spansy::json::parse;
    ///
    /// let src = b"{\"foo\": {\"bar\": [42, 14]}}";
    ///
    /// let doc = parse(src).unwrap();
    ///
    /// assert_eq!(doc.get("foo.bar.1").unwrap(), "14");
    /// ```
    pub fn get(&self, path: &str) -> Option<&JsonValue<S>> {
        self.root.get(path)
    }
}

impl<S: Store> IntoRangeIterator<usize> for Document<S> {
    type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.view().indices().clone().into_range_iter()
    }
}

impl<S: Store> Span<str> for Document<S> {
    fn data(&self) -> Cow<'_, str> {
        self.view().as_str()
    }

    fn len(&self) -> usize {
        self.view().len()
    }

    fn offset(&self) -> usize {
        self.view().offset()
    }

    fn is_empty(&self) -> bool {
        self.view().indices().is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.view().indices().len_ranges() <= 1
    }
}

impl<S: Store> PartialEq for Document<S> {
    fn eq(&self, other: &Self) -> bool {
        self.view().as_str() == other.view().as_str()
    }
}

impl<S: Store> Eq for Document<S> {}

impl<S: Store> std::hash::Hash for Document<S> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.view().as_str().hash(state);
    }
}

impl<S: Store> PartialEq<str> for Document<S> {
    fn eq(&self, other: &str) -> bool {
        self.view().as_str() == other
    }
}

impl<S: Store> PartialEq<&str> for Document<S> {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl<S: Store> AsRef<View<S, str>> for Document<S> {
    fn as_ref(&self) -> &View<S, str> {
        self.view()
    }
}

/// A JSON value with span tracking.
///
/// # Example
///
/// ```
/// use spansy::json::{parse, JsonValue};
///
/// let doc = parse(b"{\"count\": 42}").unwrap();
///
/// // Pattern match on the value type
/// let JsonValue::Object(obj) = &doc.root else {
///     panic!("value should be object");
/// };
///
/// let Some(JsonValue::Number(n)) = obj.get("count") else {
///     panic!("count number should exist");
/// };
/// assert_eq!(n, "42");
///
/// // Or use path-based access
/// assert_eq!(doc.get("count").unwrap(), "42");
/// ```
#[derive(Debug, Clone)]
pub enum JsonValue<S: Store = Bytes> {
    /// A null value.
    Null(Null<S>),
    /// A boolean value.
    Bool(Bool<S>),
    /// A number value.
    Number(Number<S>),
    /// A string value.
    String(String<S>),
    /// An array value.
    Array(Array<S>),
    /// An object value.
    Object(Object<S>),
}

impl<S: Store> JsonValue<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        match self {
            JsonValue::Null(v) => v.view(),
            JsonValue::Bool(v) => v.view(),
            JsonValue::Number(v) => v.view(),
            JsonValue::String(v) => v.view(),
            JsonValue::Array(v) => v.view(),
            JsonValue::Object(v) => v.view(),
        }
    }

    /// Get a reference to the value using the given path.
    ///
    /// # Example
    ///
    /// ```
    /// use spansy::json::{parse, JsonValue};
    ///
    /// let src = b"{\"foo\": {\"bar\": [42, 14]}}";
    ///
    /// let doc = parse(src).unwrap();
    ///
    /// assert_eq!(doc.root.get("foo.bar.1").unwrap(), "14");
    /// ```
    pub fn get(&self, path: &str) -> Option<&JsonValue<S>> {
        match self {
            JsonValue::Null(_) => None,
            JsonValue::Bool(_) => None,
            JsonValue::Number(_) => None,
            JsonValue::String(_) => None,
            JsonValue::Array(v) => v.get(path),
            JsonValue::Object(v) => v.get(path),
        }
    }
}

impl<S: Store> IntoRangeIterator<usize> for JsonValue<S> {
    type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.view().indices().clone().into_range_iter()
    }
}

impl<S: Store> Span<str> for JsonValue<S> {
    fn data(&self) -> Cow<'_, str> {
        self.view().as_str()
    }

    fn len(&self) -> usize {
        self.view().len()
    }

    fn offset(&self) -> usize {
        self.view().offset()
    }

    fn is_empty(&self) -> bool {
        self.view().indices().is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.view().indices().len_ranges() <= 1
    }
}

impl<S: Store> PartialEq for JsonValue<S> {
    fn eq(&self, other: &Self) -> bool {
        self.view().as_str() == other.view().as_str()
    }
}

impl<S: Store> Eq for JsonValue<S> {}

impl<S: Store> std::hash::Hash for JsonValue<S> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.view().as_str().hash(state);
    }
}

impl<S: Store> PartialEq<str> for JsonValue<S> {
    fn eq(&self, other: &str) -> bool {
        self.view().as_str() == other
    }
}

impl<S: Store> PartialEq<&str> for JsonValue<S> {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl<S: Store> AsRef<View<S, str>> for JsonValue<S> {
    fn as_ref(&self) -> &View<S, str> {
        self.view()
    }
}

/// A key value pair in a JSON object.
#[derive(Debug, Clone)]
pub struct KeyValue<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
    /// The key of the pair.
    pub key: JsonKey<S>,
    /// The value of the pair.
    pub value: JsonValue<S>,
}

impl<S: Store> KeyValue<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns a view of the key value pair, excluding the value.
    pub fn without_value(&self) -> View<S, str> {
        let indices = self.view.indices().difference(self.value.view().indices());
        self.view.subview(indices.into_set())
    }
}

impl<S: Store> IntoRangeIterator<usize> for KeyValue<S> {
    type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.view.indices().clone().into_range_iter()
    }
}

impl<S: Store> Span<str> for KeyValue<S> {
    fn data(&self) -> Cow<'_, str> {
        self.view.as_str()
    }

    fn len(&self) -> usize {
        self.view.len()
    }

    fn offset(&self) -> usize {
        self.view().offset()
    }

    fn is_empty(&self) -> bool {
        self.view.indices().is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.view.indices().len_ranges() <= 1
    }
}

impl<S: Store> PartialEq for KeyValue<S> {
    fn eq(&self, other: &Self) -> bool {
        self.view.as_str() == other.view.as_str()
    }
}

impl<S: Store> Eq for KeyValue<S> {}

impl<S: Store> std::hash::Hash for KeyValue<S> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.view.as_str().hash(state);
    }
}

impl<S: Store> AsRef<View<S, str>> for KeyValue<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

/// A key in a JSON object.
#[derive(Debug, Clone)]
pub struct JsonKey<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

impl<S: Store> JsonKey<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> IntoRangeIterator<usize> for JsonKey<S> {
    type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.view.indices().clone().into_range_iter()
    }
}

impl<S: Store> Span<str> for JsonKey<S> {
    fn data(&self) -> Cow<'_, str> {
        self.view.as_str()
    }

    fn len(&self) -> usize {
        self.view.len()
    }

    fn offset(&self) -> usize {
        self.view().offset()
    }

    fn is_empty(&self) -> bool {
        self.view.indices().is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.view.indices().len_ranges() <= 1
    }
}

impl<S: Store> PartialEq for JsonKey<S> {
    fn eq(&self, other: &Self) -> bool {
        self.view.as_str() == other.view.as_str()
    }
}

impl<S: Store> Eq for JsonKey<S> {}

impl<S: Store> std::hash::Hash for JsonKey<S> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.view.as_str().hash(state);
    }
}

impl<S: Store> PartialEq<str> for JsonKey<S> {
    fn eq(&self, other: &str) -> bool {
        self.view.as_str().as_ref() == other
    }
}

impl<S: Store> PartialEq<&str> for JsonKey<S> {
    fn eq(&self, other: &&str) -> bool {
        self.view.as_str().as_ref() == *other
    }
}

impl<S: Store> AsRef<View<S, str>> for JsonKey<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

/// A null value.
#[derive(Debug, Clone)]
pub struct Null<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

/// A boolean value.
#[derive(Debug, Clone)]
pub struct Bool<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

/// A number value.
#[derive(Debug, Clone)]
pub struct Number<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

/// A JSON string value.
///
/// This span does not capture the quotation marks around the string.
#[derive(Debug, Clone)]
pub struct String<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

/// A JSON array value.
///
/// # Example
///
/// ```
/// use spansy::json::{parse, JsonValue};
///
/// let doc = parse(b"[1, 2, 3]").unwrap();
///
/// if let JsonValue::Array(arr) = doc.root {
///     for elem in &arr.elems {
///         println!("{}", elem.view().as_str());
///     }
///     assert_eq!(arr.elems.len(), 3);
///     assert_eq!(arr[1], "2");
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Array<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
    /// The elements of the array.
    pub elems: Vec<JsonValue<S>>,
}

impl<S: Store> Array<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Get a reference to the value using the given path.
    pub fn get(&self, path: &str) -> Option<&JsonValue<S>> {
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

    /// Returns a view of the array brackets, excluding the values and
    /// separators.
    pub fn without_values(&self) -> View<S, str> {
        let len = self.view.len();
        let first = self
            .view
            .select(0..1)
            .expect("array should have opening bracket");
        let last = self
            .view
            .select(len - 1..len)
            .expect("array should have closing bracket");
        let indices = first.indices().union(last.indices()).into_set();
        self.view.subview(indices)
    }
}

impl<S: Store> IntoRangeIterator<usize> for Array<S> {
    type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.view.indices().clone().into_range_iter()
    }
}

impl<S: Store> Span<str> for Array<S> {
    fn data(&self) -> Cow<'_, str> {
        self.view.as_str()
    }

    fn len(&self) -> usize {
        self.view.len()
    }

    fn offset(&self) -> usize {
        self.view().offset()
    }

    fn is_empty(&self) -> bool {
        self.view.indices().is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.view.indices().len_ranges() <= 1
    }
}

impl<S: Store> PartialEq for Array<S> {
    fn eq(&self, other: &Self) -> bool {
        self.view.as_str() == other.view.as_str()
    }
}

impl<S: Store> Eq for Array<S> {}

impl<S: Store> std::hash::Hash for Array<S> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.view.as_str().hash(state);
    }
}

impl<S: Store> Index<usize> for Array<S> {
    type Output = JsonValue<S>;

    /// Returns the value at the given index of the array.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    fn index(&self, index: usize) -> &Self::Output {
        self.elems.get(index).expect("index should be in bounds")
    }
}

impl<S: Store> AsRef<View<S, str>> for Array<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

/// A JSON object value.
///
/// # Example
///
/// ```
/// use spansy::json::{parse, JsonValue};
///
/// let doc = parse(b"{\"a\": 1, \"b\": 2}").unwrap();
///
/// if let JsonValue::Object(obj) = doc.root {
///     // Iterate over key-value pairs
///     for kv in &obj.elems {
///         println!("{}: {}", kv.key.view().as_str(), kv.value.view().as_str());
///     }
///     // Access by key
///     assert_eq!(obj["a"], "1");
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Object<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
    /// The key value pairs of the object.
    pub elems: Vec<KeyValue<S>>,
}

impl<S: Store> Object<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Get a reference to the value using the given path.
    pub fn get(&self, path: &str) -> Option<&JsonValue<S>> {
        let mut path_iter = path.split('.');

        let key = path_iter.next()?;

        let KeyValue { value, .. } = self.elems.iter().find(|kv| kv.key == key)?;

        if path_iter.next().is_some() {
            value.get(&path[key.len() + 1..])
        } else {
            Some(value)
        }
    }

    /// Returns a view of the object, excluding the key value pairs.
    pub fn without_pairs(&self) -> View<S, str> {
        let mut indices = self.view.indices().clone();
        for kv in &self.elems {
            indices = indices.difference(kv.view().indices()).into_set();
        }
        self.view.subview(indices)
    }
}

impl<S: Store> IntoRangeIterator<usize> for Object<S> {
    type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.view.indices().clone().into_range_iter()
    }
}

impl<S: Store> Span<str> for Object<S> {
    fn data(&self) -> Cow<'_, str> {
        self.view.as_str()
    }

    fn len(&self) -> usize {
        self.view.len()
    }

    fn offset(&self) -> usize {
        self.view().offset()
    }

    fn is_empty(&self) -> bool {
        self.view.indices().is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.view.indices().len_ranges() <= 1
    }
}

impl<S: Store> PartialEq for Object<S> {
    fn eq(&self, other: &Self) -> bool {
        self.view.as_str() == other.view.as_str()
    }
}

impl<S: Store> Eq for Object<S> {}

impl<S: Store> std::hash::Hash for Object<S> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.view.as_str().hash(state);
    }
}

impl<S: Store> Index<&str> for Object<S> {
    type Output = JsonValue<S>;

    /// Returns the value at the given key of the object.
    ///
    /// # Panics
    ///
    /// Panics if the key is not present.
    fn index(&self, key: &str) -> &Self::Output {
        self.get(key).expect("key should be present")
    }
}

impl<S: Store> AsRef<View<S, str>> for Object<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

/// Macro to implement common traits for simple JSON types.
macro_rules! impl_span_type {
    ($ty:ident) => {
        impl<S: Store> $ty<S> {
            /// Returns the underlying view.
            pub fn view(&self) -> &View<S, str> {
                &self.view
            }
        }

        impl<S: Store> AsRef<View<S, str>> for $ty<S> {
            fn as_ref(&self) -> &View<S, str> {
                &self.view
            }
        }

        impl<S: Store> rangeset::iter::IntoRangeIterator<usize> for $ty<S> {
            type IntoIter = <RangeSet<usize> as rangeset::iter::IntoRangeIterator<usize>>::IntoIter;

            fn into_range_iter(self) -> Self::IntoIter {
                self.view.indices().clone().into_range_iter()
            }
        }

        impl<S: Store> Span<str> for $ty<S> {
            fn data(&self) -> Cow<'_, str> {
                self.view.as_str()
            }

            fn len(&self) -> usize {
                self.view.len()
            }

            fn offset(&self) -> usize {
                self.view().offset()
            }

            fn is_empty(&self) -> bool {
                self.view.indices().is_empty()
            }

            fn is_contiguous(&self) -> bool {
                self.view.indices().len_ranges() <= 1
            }
        }

        impl<S: Store> PartialEq for $ty<S> {
            fn eq(&self, other: &Self) -> bool {
                self.view.as_str() == other.view.as_str()
            }
        }

        impl<S: Store> Eq for $ty<S> {}

        impl<S: Store> std::hash::Hash for $ty<S> {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.view.as_str().hash(state);
            }
        }

        impl<S: Store> PartialEq<str> for $ty<S> {
            fn eq(&self, other: &str) -> bool {
                self.view.as_str().as_ref() == other
            }
        }

        impl<S: Store> PartialEq<&str> for $ty<S> {
            fn eq(&self, other: &&str) -> bool {
                self.view.as_str().as_ref() == *other
            }
        }
    };
}

impl_span_type!(Null);
impl_span_type!(Bool);
impl_span_type!(Number);
impl_span_type!(String);

macro_rules! impl_ref_range_iter {
    ($($ty:ident),*) => {$(
        impl<'a, S: Store> IntoRangeIterator<usize> for &'a $ty<S> {
            type IntoIter = RangeIter<'a, usize>;

            fn into_range_iter(self) -> Self::IntoIter {
                self.view().indices().into_range_iter()
            }
        }
    )*};
}

impl_ref_range_iter!(
    Document, JsonValue, KeyValue, JsonKey, Array, Object, Null, Bool, Number, String
);

#[cfg(test)]
mod tests {
    use crate::json::parse;

    use super::*;

    #[test]
    fn test_obj_index() {
        let src = b"{\"foo\": \"bar\"}";

        let value = parse(src).unwrap();

        assert_eq!(value.get("foo").unwrap(), "bar");
    }

    #[test]
    fn test_array_index() {
        let src = b"{\"foo\": [42, 14]}";

        let value = parse(src).unwrap();

        assert_eq!(value.get("foo.1").unwrap(), "14");
    }

    #[test]
    fn test_nested_index() {
        let src = b"{\"foo\": {\"bar\": [42, 14]}}";

        let value = parse(src).unwrap();

        assert_eq!(value.get("foo.bar.1").unwrap(), "14");
    }

    #[test]
    fn test_key_value_without_value() {
        let src = b"{\"foo\": \"bar\"\n}";

        let JsonValue::Object(value) = parse(src).unwrap().root else {
            panic!("expected object");
        };

        let view = value.elems[0].without_value();
        assert_eq!(view.as_str().as_ref(), "\"foo\": \"\"");
    }

    #[test]
    fn test_key_value() {
        let src = b"{\"foo\": \"bar\", \"baz\": \"buzz\"\n}";

        let JsonValue::Object(value) = parse(src).unwrap().root else {
            panic!("expected object");
        };

        // KeyValue should not include the trailing comma
        assert_eq!(value.elems[0].view().as_str().as_ref(), "\"foo\": \"bar\"");
        assert_eq!(value.elems[1].view().as_str().as_ref(), "\"baz\": \"buzz\"");
    }

    #[test]
    fn test_array_elements() {
        let src = b"[1, 2, 3]";

        let JsonValue::Array(value) = parse(src).unwrap().root else {
            panic!("expected array");
        };

        // Array elements should not include commas
        assert_eq!(value.elems[0].view().as_str().as_ref(), "1");
        assert_eq!(value.elems[1].view().as_str().as_ref(), "2");
        assert_eq!(value.elems[2].view().as_str().as_ref(), "3");
    }

    #[test]
    fn test_array_without_values() {
        let src = b"[42, 14]";

        let JsonValue::Array(value) = parse(src).unwrap().root else {
            panic!("expected array");
        };

        let view = value.without_values();
        assert_eq!(view.as_str().as_ref(), "[]");
    }

    #[test]
    fn test_object_without_pairs() {
        let src = b"{\"foo\": \"bar\"\n}";

        let JsonValue::Object(value) = parse(src).unwrap().root else {
            panic!("expected object");
        };

        let view = value.without_pairs();
        assert_eq!(view.as_str().as_ref(), "{\n}");
    }
}
