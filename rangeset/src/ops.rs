//! Set operations.

#[cfg(feature = "alloc")]
mod cover;

#[cfg(feature = "alloc")]
pub use cover::Cover;

use core::marker::PhantomData;

use crate::iter::{IntoRangeIterator, RangeIterator};

/// Set union.
pub trait Union<Rhs> {
    type Output<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Returns the set union of `self` and `other`.
    fn union<'a>(&'a self, other: &'a Rhs) -> Self::Output<'a>;
}

/// Set union in-place.
pub trait UnionMut<Rhs> {
    /// Replaces `self` with the set union of `self` and `other`.
    fn union_mut(&mut self, other: &Rhs);
}

/// Set difference.
pub trait Difference<Rhs> {
    /// The output type of the difference operation.
    type Output<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Returns the set difference of `self` and `other`.
    fn difference<'a>(&'a self, other: &'a Rhs) -> Self::Output<'a>;
}

/// Set difference in-place.
pub trait DifferenceMut<Rhs> {
    /// Subtracts `other` from `self`.
    fn difference_mut(&mut self, other: &Rhs);
}

/// Set symmetric difference.
pub trait SymmetricDifference<Rhs> {
    type Output<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Returns the set symmetric difference of `self` and `other`.
    fn symmetric_difference<'a>(&'a self, other: &'a Rhs) -> Self::Output<'a>;
}

/// Set symmetric difference in-place.
pub trait SymmetricDifferenceMut<Rhs> {
    /// Replaces `self` with the set symmetric difference of `self` and `other`.
    fn symmetric_difference_mut(&mut self, other: &Rhs);
}

/// Set intersection.
pub trait Intersection<Rhs> {
    type Output<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Returns the set intersection of `self` and `other`.
    fn intersection<'a>(&'a self, other: &'a Rhs) -> Self::Output<'a>;
}

/// Set disjoint check.
pub trait Disjoint<Rhs> {
    /// Returns `true` if the range is disjoint with `other`.
    fn is_disjoint(&self, other: &Rhs) -> bool;
}

/// Set subset check.
pub trait Subset<Rhs> {
    /// Returns `true` if `self` is a subset of `other`.
    fn is_subset(&self, other: &Rhs) -> bool;
}

/// Indexing operation.
pub trait Index<Idx> {
    /// Output type.
    type Output<'a>
    where
        Self: 'a,
        Idx: 'a;

    /// Index the collection by the given index.
    fn index<'a>(&'a self, index: Idx) -> Self::Output<'a>
    where
        Idx: 'a;
}

/// Iterator over a slice using an index.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct SliceIndexIter<'a, T, I> {
    slice: &'a [T],
    iter: I,
    _pd: PhantomData<&'a T>,
}

impl<'a, T, I> Iterator for SliceIndexIter<'a, T, I>
where
    I: RangeIterator<usize>,
{
    type Item = &'a [T];

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|range| &self.slice[range])
    }
}

impl<T, Idx> Index<Idx> for [T]
where
    Idx: IntoRangeIterator<usize>,
{
    type Output<'a>
        = SliceIndexIter<'a, T, Idx::IntoIter>
    where
        T: 'a,
        Idx: 'a;

    fn index<'a>(&'a self, index: Idx) -> Self::Output<'a>
    where
        Idx: 'a,
    {
        SliceIndexIter {
            slice: self,
            iter: index.into_range_iter(),
            _pd: PhantomData,
        }
    }
}
