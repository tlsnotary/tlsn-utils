//! Parsing with span tracking.
//!
//! This crate provides parsers that track the byte positions (spans) of each
//! parsed element within the source data. This enables applications to locate,
//! extract, or redact specific values by their original position.
//!
//! # Core Types
//!
//! - [`Store`] - Trait for byte storage types (`Bytes`, `Vec<u8>`, `&[u8]`,
//!   etc.)
//! - [`View`] - A view into a store with tracked index ranges
//! - [`Span`] - Trait implemented by all parsed types for uniform data access
//!
//! # Example
//!
//! ```
//! use spansy::{Span, json};
//!
//! let src = b"{\"user\": {\"name\": \"alice\", \"id\": 42}}";
//! let value = json::parse(src).unwrap();
//!
//! // Access nested values by path
//! let name = value.get("user.name").unwrap();
//! assert_eq!(name, "alice");
//!
//! // Get the byte position of the value within the source
//! assert_eq!(name.view().offset(), 19);
//! ```

#![deny(missing_docs, unreachable_pub, unused_must_use)]
#![deny(clippy::all)]

use std::{borrow::Cow, marker::PhantomData, ops::Range, sync::Arc};

use bytes::Bytes;
use rangeset::{iter::IntoRangeIterator, ops::Set, set::RangeSet};

pub mod http;
pub mod json;

/// A parsing error.
#[derive(Debug, thiserror::Error)]
#[error("parsing error: {0}")]
pub struct ParseError(pub(crate) String);

impl ParseError {
    pub(crate) fn from_pest<R: pest::RuleType>(err: pest::error::Error<R>) -> Self {
        Self(err.to_string())
    }
}

impl From<std::str::Utf8Error> for ParseError {
    fn from(value: std::str::Utf8Error) -> Self {
        Self(value.to_string())
    }
}

/// Trait for contiguous byte storage.
///
/// Views clone the store when creating subviews, so prefer reference-counted
/// types like [`Arc`] or [`Bytes`] to avoid expensive deep copies.
pub trait Store: AsRef<[u8]> + Clone {}

impl Store for Bytes {}
impl Store for Vec<u8> {}
impl Store for &[u8] {}
impl<const N: usize> Store for &[u8; N] {}
impl Store for Arc<[u8]> {}
impl Store for Box<[u8]> {}

/// A view into a byte store with tracked indices.
///
/// Views can be sliced into subviews while preserving the original indices.
///
/// # Example
///
/// ```
/// use spansy::View;
///
/// let data = b"hello world";
/// let view = View::new(data.as_slice());
///
/// // Create a subview of "world" (indices 6..11)
/// let subview = view.select(6..11).unwrap();
///
/// assert_eq!(&*subview.as_bytes(), b"world");
/// assert_eq!(subview.offset(), 6); // Original index preserved
/// ```
#[derive(Debug)]
pub struct View<S, T: ?Sized = [u8]> {
    store: S,
    indices: RangeSet<usize>,
    _marker: PhantomData<*const T>,
}

impl<S: Clone, T: ?Sized> Clone for View<S, T> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            indices: self.indices.clone(),
            _marker: PhantomData,
        }
    }
}

impl<S: Store, T: ?Sized> View<S, T> {
    /// Returns `true` if the view is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the length of the view.
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Returns the absolute indices in the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        &self.indices
    }

    /// Returns the base offset (start of first index range).
    pub fn offset(&self) -> usize {
        self.indices.min().unwrap_or(0)
    }
}

impl<S: Store> View<S, [u8]> {
    /// Create a byte view of the entire store.
    pub fn new(store: S) -> Self {
        let len = store.as_ref().len();
        Self {
            store,
            indices: RangeSet::from(0..len),
            _marker: PhantomData,
        }
    }

    /// Returns the data as bytes.
    pub fn as_bytes(&self) -> Cow<'_, [u8]> {
        let bytes = self.store.as_ref();

        if self.indices.len_ranges() == 1 {
            let range = self.indices.iter().next().unwrap();
            return Cow::Borrowed(&bytes[range]);
        }

        let mut result = Vec::with_capacity(self.indices.len());
        for range in self.indices.iter() {
            result.extend_from_slice(&bytes[range]);
        }
        Cow::Owned(result)
    }

    /// Create a subview with the given absolute indices.
    ///
    /// # Panics
    ///
    /// Panics if indices are not a subset of this view's indices.
    pub(crate) fn subview(&self, indices: RangeSet<usize>) -> Self {
        assert!(
            indices.is_subset(&self.indices),
            "indices should be subset of view"
        );

        Self {
            store: self.store.clone(),
            indices,
            _marker: PhantomData,
        }
    }

    /// Select a range from concatenated space, returning a new view.
    ///
    /// Maps a range `[start, end)` as if all ranges were concatenated
    /// contiguously starting at 0, to the corresponding absolute indices.
    ///
    /// Returns `None` if the range extends beyond the view's length.
    pub fn select(&self, range: Range<usize>) -> Option<Self> {
        let indices = select_indices(&self.indices, range)?;
        Some(self.subview(indices))
    }
}

impl<S: Store> View<S, str> {
    /// Create a string view of the entire store.
    ///
    /// # Panics
    ///
    /// Panics if the store does not contain valid UTF-8.
    pub fn new_str(store: S) -> Self {
        let bytes = store.as_ref();
        std::str::from_utf8(bytes).expect("store should contain valid UTF-8");
        let len = bytes.len();
        Self {
            store,
            indices: RangeSet::from(0..len),
            _marker: PhantomData,
        }
    }

    /// Returns the data as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        let bytes = self.store.as_ref();

        if self.indices.len_ranges() == 1 {
            let range = self.indices.iter().next().unwrap();
            return Cow::Borrowed(unsafe { std::str::from_utf8_unchecked(&bytes[range]) });
        }

        let mut result = Vec::with_capacity(self.indices.len());
        for range in self.indices.iter() {
            result.extend_from_slice(&bytes[range]);
        }
        Cow::Owned(unsafe { std::string::String::from_utf8_unchecked(result) })
    }

    /// Create a subview with the given absolute indices.
    ///
    /// # Panics
    ///
    /// Panics if indices are not a subset of this view's indices or split a
    /// UTF-8 character.
    pub(crate) fn subview(&self, indices: RangeSet<usize>) -> Self {
        assert!(
            indices.is_subset(&self.indices),
            "indices should be subset of view"
        );

        // Validate UTF-8 boundaries
        let bytes = self.store.as_ref();
        for range in indices.iter() {
            std::str::from_utf8(&bytes[range]).expect("indices should not split UTF-8 characters");
        }

        Self {
            store: self.store.clone(),
            indices,
            _marker: PhantomData,
        }
    }

    /// Select a range from concatenated space, returning a new view.
    ///
    /// Maps a range `[start, end)` as if all ranges were concatenated
    /// contiguously starting at 0, to the corresponding absolute indices.
    ///
    /// Returns `None` if the range extends beyond the view's length or splits a
    /// UTF-8 character.
    pub fn select(&self, range: Range<usize>) -> Option<Self> {
        let indices = select_indices(&self.indices, range)?;

        // Validate UTF-8 boundaries
        let bytes = self.store.as_ref();
        for r in indices.iter() {
            if std::str::from_utf8(&bytes[r]).is_err() {
                return None;
            }
        }

        Some(Self {
            store: self.store.clone(),
            indices,
            _marker: PhantomData,
        })
    }
}

impl<S: Store> TryFrom<View<S>> for View<S, str> {
    type Error = std::str::Utf8Error;

    fn try_from(value: View<S>) -> Result<Self, Self::Error> {
        let data = value.store.as_ref();
        for range in value.indices.iter() {
            std::str::from_utf8(&data[range])?;
        }

        Ok(Self {
            store: value.store,
            indices: value.indices,
            _marker: PhantomData,
        })
    }
}

impl<S: Store> From<S> for View<S, [u8]> {
    fn from(store: S) -> Self {
        Self::new(store)
    }
}

impl<S: Clone, T: ?Sized> IntoRangeIterator<usize> for View<S, T> {
    type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.indices.into_range_iter()
    }
}

/// A span of bytes or string data with tracked indices.
///
/// All parsed types implement this trait, providing a uniform interface for
/// accessing span data and indices.
///
/// # Example
///
/// ```
/// use spansy::{Span, json::parse};
///
/// let value = parse(b"{\"name\": \"alice\"}").unwrap();
/// let name = value.get("name").unwrap();
///
/// assert_eq!(name, "alice");
/// assert_eq!(name.view().offset(), 10);
/// ```
pub trait Span<T: ToOwned + ?Sized = [u8]>: IntoRangeIterator<usize> {
    /// Returns the span data.
    fn data(&self) -> Cow<'_, T>;

    /// Returns the length of the span.
    fn len(&self) -> usize;

    /// Returns the offset of the span in the store.
    fn offset(&self) -> usize;

    /// Returns true if the span is empty.
    fn is_empty(&self) -> bool;

    /// Returns true if the span is contiguous.
    fn is_contiguous(&self) -> bool;
}

impl<S: Store> Span<[u8]> for View<S, [u8]> {
    fn data(&self) -> Cow<'_, [u8]> {
        <View<S, [u8]>>::as_bytes(self)
    }

    fn len(&self) -> usize {
        self.indices.len()
    }

    fn offset(&self) -> usize {
        self.indices.min().unwrap_or_default()
    }

    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.indices.len_ranges() <= 1
    }
}

/// Maps a range in concatenated space to absolute indices.
///
/// A RangeSet defines an order-preserving bijection between:
/// - **Concatenated space**: `[0, len)` where ranges are flattened contiguously
/// - **Absolute space**: the actual positions in the store
///
/// This function computes the inverse mapping: given a range in concatenated
/// space, it returns the corresponding absolute indices.
///
/// # Example
///
/// ```text
/// indices: [100..105, 200..205]  (absolute space)
/// flatten: [0..5, 5..10]         (concatenated space)
///
/// select(3..8) maps:
///   concat [3..5] -> abs [103..105]
///   concat [5..8] -> abs [200..203]
/// result: [103..105, 200..203]
/// ```
fn select_indices(indices: &RangeSet<usize>, range: Range<usize>) -> Option<RangeSet<usize>> {
    let total_len = indices.len();
    if range.start > total_len || range.end > total_len {
        return None;
    }

    let mut result = Vec::new();
    let mut concat_pos = 0usize;

    for abs_range in indices.iter() {
        let range_len = abs_range.end - abs_range.start;
        let concat_end = concat_pos + range_len;

        if range.start < concat_end && range.end > concat_pos {
            let overlap_start = range.start.max(concat_pos);
            let overlap_end = range.end.min(concat_end);

            let offset = overlap_start - concat_pos;
            let len = overlap_end - overlap_start;
            let abs_start = abs_range.start + offset;
            let abs_end = abs_start + len;

            result.push(abs_start..abs_end);
        }

        concat_pos = concat_end;

        if concat_pos >= range.end {
            break;
        }
    }

    Some(RangeSet::from(result))
}

impl<S: Store> Span<str> for View<S, str> {
    fn data(&self) -> Cow<'_, str> {
        <View<S, str>>::as_str(self)
    }

    fn len(&self) -> usize {
        self.indices.len()
    }

    fn offset(&self) -> usize {
        self.indices.min().unwrap_or_default()
    }

    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn is_contiguous(&self) -> bool {
        self.indices.len_ranges() <= 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_indices_single_range() {
        let indices = RangeSet::from(0..10);
        assert_eq!(select_indices(&indices, 3..7), Some(RangeSet::from(3..7)));
    }

    #[test]
    fn test_select_indices_within_first() {
        let indices = RangeSet::from([0..5, 10..15]);
        assert_eq!(select_indices(&indices, 2..4), Some(RangeSet::from(2..4)));
    }

    #[test]
    fn test_select_indices_within_second() {
        // concat: [0..5, 5..10] maps to abs: [0..5, 10..15]
        // select concat 6..8 -> abs 11..13
        let indices = RangeSet::from([0..5, 10..15]);
        assert_eq!(select_indices(&indices, 6..8), Some(RangeSet::from(11..13)));
    }

    #[test]
    fn test_select_indices_spanning_both() {
        // concat: [0..5, 5..10] maps to abs: [0..5, 10..15]
        // select concat 3..8 -> abs [3..5, 10..13]
        let indices = RangeSet::from([0..5, 10..15]);
        assert_eq!(
            select_indices(&indices, 3..8),
            Some(RangeSet::from([3..5, 10..13]))
        );
    }

    #[test]
    fn test_select_indices_entire_range() {
        let indices = RangeSet::from([0..5, 10..15]);
        assert_eq!(
            select_indices(&indices, 0..10),
            Some(RangeSet::from([0..5, 10..15]))
        );
    }

    #[test]
    fn test_select_indices_empty() {
        let indices = RangeSet::from(0..10);
        assert_eq!(
            select_indices(&indices, 5..5),
            Some(RangeSet::from([] as [Range<usize>; 0]))
        );
    }

    #[test]
    fn test_select_indices_three_ranges() {
        // abs: [0..3, 10..13, 20..23] -> concat: [0..3, 3..6, 6..9]
        // select concat 2..7 -> abs [2..3, 10..13, 20..21]
        let indices = RangeSet::from([0..3, 10..13, 20..23]);
        assert_eq!(
            select_indices(&indices, 2..7),
            Some(RangeSet::from([2..3, 10..13, 20..21]))
        );
    }

    #[test]
    fn test_select_indices_boundary_start() {
        let indices = RangeSet::from([0..5, 10..15]);
        assert_eq!(select_indices(&indices, 0..3), Some(RangeSet::from(0..3)));
    }

    #[test]
    fn test_select_indices_boundary_end() {
        let indices = RangeSet::from([0..5, 10..15]);
        assert_eq!(
            select_indices(&indices, 7..10),
            Some(RangeSet::from(12..15))
        );
    }

    #[test]
    fn test_select_indices_at_range_boundary() {
        // Select exactly at the boundary between ranges
        let indices = RangeSet::from([0..5, 10..15]);
        assert_eq!(
            select_indices(&indices, 5..5),
            Some(RangeSet::from([] as [Range<usize>; 0]))
        );
        assert_eq!(
            select_indices(&indices, 4..6),
            Some(RangeSet::from([4..5, 10..11]))
        );
    }

    #[test]
    fn test_view_select_non_contiguous() {
        let data = b"hello-----world";
        let view: View<&[u8], str> = View::new_str(data.as_slice());
        let indices = RangeSet::from([0..5, 10..15]);
        let non_contig = view.subview(indices);

        // concat "helloworld", select "lowo" (3..7)
        let selected = non_contig.select(3..7).unwrap();
        assert_eq!(selected.as_str(), "lowo");
        assert_eq!(*selected.indices(), RangeSet::from([3..5, 10..12]));
    }

    #[test]
    fn test_view_as_str_contiguous_borrowed() {
        let data = b"hello";
        let view = View::new_str(data.as_slice());
        let result = view.as_str();
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "hello");
    }

    #[test]
    fn test_view_as_str_non_contiguous_owned() {
        let data = b"hello-----world";
        let view = View::new_str(data.as_slice());
        let indices = RangeSet::from([0..5, 10..15]);
        let non_contig = view.subview(indices);

        let result = non_contig.as_str();
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result.as_ref(), "helloworld");
    }

    #[test]
    fn test_view_as_bytes_non_contiguous() {
        let data = b"AAAA----BBBB";
        let view = View::new(data.as_slice());
        let indices = RangeSet::from([0..4, 8..12]);
        let non_contig = view.subview(indices);

        assert_eq!(non_contig.as_bytes().as_ref(), b"AAAABBBB");
    }

    #[test]
    fn test_view_select_str_valid_utf8() {
        let data = "hello世界";
        let view = View::new_str(data.as_bytes());
        // "世" is 3 bytes at positions 5..8
        let sub = view.select(0..8).unwrap();
        assert_eq!(sub.as_str(), "hello世");
    }

    #[test]
    fn test_view_select_str_invalid_utf8_boundary() {
        let data = "hello世界";
        let view = View::new_str(data.as_bytes());
        // Try to split "世" (positions 5..8)
        assert!(view.select(0..6).is_none());
        assert!(view.select(0..7).is_none());
    }

    #[test]
    fn test_view_select_str_multibyte_chars() {
        let data = "日本語";
        let view = View::new_str(data.as_bytes());
        // Each char is 3 bytes
        let sub = view.select(0..3).unwrap();
        assert_eq!(sub.as_str(), "日");

        let sub = view.select(3..6).unwrap();
        assert_eq!(sub.as_str(), "本");
    }
}
