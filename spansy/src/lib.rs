//! Parsing span information.

#![deny(missing_docs, unreachable_pub, unused_must_use)]
#![deny(clippy::all)]

use std::{fmt::Debug, marker::PhantomData, ops::Range};

use bytes::Bytes;

pub(crate) mod helpers;
pub mod http;
pub mod json;

use utils::range::{RangeSet, ToRangeSet};

/// A parsing error.
#[derive(Debug, thiserror::Error)]
#[error("parsing error: {0}")]
pub struct ParseError(String);

impl<R: pest::RuleType> From<pest::error::Error<R>> for ParseError {
    fn from(value: pest::error::Error<R>) -> Self {
        Self(value.to_string())
    }
}

impl From<std::str::Utf8Error> for ParseError {
    fn from(value: std::str::Utf8Error) -> Self {
        Self(value.to_string())
    }
}

/// A spanned value.
pub trait Spanned<T: ?Sized = [u8]> {
    /// Get a reference to the span of the value.
    fn span(&self) -> &Span<T>;
}

/// A span of a source string.
#[derive(PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Span<T: ?Sized = [u8]> {
    /// The original source bytes from when the span was parsed.
    pub(crate) data: Bytes,
    /// The set of indices within the source data.
    pub(crate) indices: RangeSet<usize>,
    _pd: PhantomData<T>,
}

impl Clone for Span<[u8]> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            indices: self.indices.clone(),
            _pd: PhantomData,
        }
    }
}

impl Clone for Span<str> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            indices: self.indices.clone(),
            _pd: PhantomData,
        }
    }
}

impl Debug for Span<[u8]> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Span")
            .field("span", &self.as_bytes())
            .field("indices", &self.indices)
            .finish()
    }
}

impl Debug for Span<str> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Span")
            .field("span", &self.as_str())
            .field("indices", &self.indices)
            .finish()
    }
}

impl<T: ?Sized> AsRef<[u8]> for Span<T> {
    fn as_ref(&self) -> &[u8] {
        self.data.as_ref()
    }
}

impl<T: ?Sized> AsRef<RangeSet<usize>> for Span<T> {
    fn as_ref(&self) -> &RangeSet<usize> {
        &self.indices
    }
}

impl<T: ?Sized> Into<Bytes> for Span<T> {
    fn into(self) -> Bytes {
        self.data
    }
}

impl<T: ?Sized> Into<RangeSet<usize>> for Span<T> {
    fn into(self) -> RangeSet<usize> {
        self.indices
    }
}

impl<T: ?Sized> Span<T> {
    /// Returns a reference to the span data.
    pub fn data(&self) -> &[u8] {
        self.data.as_ref()
    }

    /// Converts the span into bytes.
    pub fn to_bytes(self) -> Bytes {
        self.data
    }

    /// Returns the indices within the source data.
    pub fn indices(&self) -> &RangeSet<usize> {
        &self.indices
    }

    /// Returns the length of the span in bytes.
    ///
    /// Just like `str::len()`, this is not necessarily the number of characters.
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Returns `true` if the span is empty.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Shifts the span indices by the given offset.
    ///
    /// # Panics
    ///
    /// Panics if the offset causes the indices to overflow `usize::MAX`.
    pub fn offset(&mut self, offset: usize) {
        self.indices.shift_right(&offset);
    }
}

impl Span<str> {
    /// Create a new string span.
    ///
    /// # Panics
    ///
    /// Panics if the given range is not within the source bytes, or
    /// if the span is not a valid UTF-8 string.
    pub(crate) fn new_str(src: Bytes, range: Range<usize>) -> Self {
        assert!(
            std::str::from_utf8(&src[range.clone()]).is_ok(),
            "span is not a valid UTF-8 string"
        );

        Self {
            data: src.slice(range.clone()),
            indices: range.into(),
            _pd: PhantomData,
        }
    }

    /// Create a new string span from a string slice.
    ///
    /// # Panics
    ///
    /// Panics if the given slice is not within the source bytes.
    pub(crate) fn new_from_str(src: Bytes, span: &str) -> Self {
        let range = helpers::get_span_range(src.as_ref(), span.as_bytes());

        Self {
            data: src.slice(range.clone()),
            indices: range.into(),
            _pd: PhantomData,
        }
    }

    /// Converts this type to a string slice.
    pub fn as_str(&self) -> &str {
        // # Safety
        // The span is guaranteed to be a valid UTF-8 string because it is not
        // possible to create a `Span<str>` from a non-UTF-8 string.
        unsafe { std::str::from_utf8_unchecked(&self.data) }
    }

    /// Returns the span as a byte span.
    pub fn as_byte_span(&self) -> &Span<[u8]> {
        self.as_ref()
    }

    /// Returns the corresponding byte span.
    pub fn to_byte_span(&self) -> Span<[u8]> {
        self.into()
    }
}

impl AsRef<Span<[u8]>> for Span<str> {
    fn as_ref(&self) -> &Span<[u8]> {
        // # Safety
        // Span<str> can be safely converted to Span<[u8]> because they have
        // an identical layout and only differ in the phantom type parameter
        // which is a ZST.
        unsafe { &*(self as *const Span<str> as *const Span<[u8]>) }
    }
}

impl AsRef<str> for Span<str> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Span<[u8]> {
    /// Create a new byte span.
    ///
    /// # Panics
    ///
    /// Panics if the given range is not within the source bytes.
    pub(crate) fn new_bytes(src: Bytes, range: Range<usize>) -> Self {
        assert!(src.len() >= range.end, "span is not within source bytes");

        Self {
            data: src.slice(range.clone()),
            indices: range.into(),
            _pd: PhantomData,
        }
    }

    /// Converts this type to a byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        self.as_ref()
    }
}

impl From<Span<str>> for Span<[u8]> {
    fn from(span: Span<str>) -> Self {
        Self {
            data: span.data,
            indices: span.indices,
            _pd: PhantomData,
        }
    }
}

impl From<&Span<str>> for Span<[u8]> {
    fn from(span: &Span<str>) -> Self {
        Self {
            data: span.data.clone(),
            indices: span.indices.clone(),
            _pd: PhantomData,
        }
    }
}

impl<T: ?Sized> ToRangeSet<usize> for Span<T> {
    fn to_range_set(&self) -> RangeSet<usize> {
        self.indices.clone()
    }
}

impl<T: ?Sized> PartialEq<Span<T>> for [u8] {
    fn eq(&self, other: &Span<T>) -> bool {
        self == other.data
    }
}

impl<T: ?Sized> PartialEq<[u8]> for Span<T> {
    fn eq(&self, other: &[u8]) -> bool {
        self.data == other
    }
}

impl<T: ?Sized> PartialEq<&[u8]> for Span<T> {
    fn eq(&self, other: &&[u8]) -> bool {
        self.data == *other
    }
}

impl<T: ?Sized> PartialEq<[u8]> for &Span<T> {
    fn eq(&self, other: &[u8]) -> bool {
        self.data == other
    }
}

impl PartialEq<Span<str>> for str {
    fn eq(&self, other: &Span<str>) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<str> for Span<str> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Span<str> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<str> for &Span<str> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<T: ?Sized> PartialEq<Range<usize>> for Span<T> {
    fn eq(&self, other: &Range<usize>) -> bool {
        &self.indices == other
    }
}

impl<T: ?Sized> PartialEq<Span<T>> for Range<usize> {
    fn eq(&self, other: &Span<T>) -> bool {
        other == self
    }
}

impl<T: ?Sized> PartialEq<Range<usize>> for &Span<T> {
    fn eq(&self, other: &Range<usize>) -> bool {
        *self == other
    }
}

impl<T: ?Sized> PartialEq<Span<T>> for &Range<usize> {
    fn eq(&self, other: &Span<T>) -> bool {
        other == *self
    }
}
