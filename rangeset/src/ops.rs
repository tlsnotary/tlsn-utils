//! Set operations.

#[cfg(feature = "alloc")]
mod cover;

#[cfg(feature = "alloc")]
pub use cover::Cover;

use core::marker::PhantomData;

use crate::iter::{IntoRangeIterator, RangeIterator};

/// Set operations.
pub trait Set<Rhs> {
    /// Union type.
    type Union<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Difference type.
    type Difference<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Intersection type.
    type Intersection<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Symmetric difference type.
    type SymmetricDifference<'a>
    where
        Self: 'a,
        Rhs: 'a;

    /// Returns the set union of `self` and `rhs`.
    fn union<'a>(&'a self, rhs: Rhs) -> Self::Union<'a>
    where
        Rhs: 'a;

    /// Returns the set difference of `self` and `rhs`.
    fn difference<'a>(&'a self, rhs: Rhs) -> Self::Difference<'a>
    where
        Rhs: 'a;

    /// Returns the set intersection of `self` and `rhs`.
    fn intersection<'a>(&'a self, rhs: Rhs) -> Self::Intersection<'a>
    where
        Rhs: 'a;

    /// Returns the set symmetric difference of `self` and `rhs`.
    fn symmetric_difference<'a>(&'a self, rhs: Rhs) -> Self::SymmetricDifference<'a>
    where
        Rhs: 'a;

    /// Returns `true` if `self` is disjoint with `other`.
    fn is_disjoint(&self, other: Rhs) -> bool;

    /// Returns `true` if `self` is a subset of `other`.
    fn is_subset(&self, other: Rhs) -> bool;

    /// Returns `true` if `self` is a superset of `other`.
    fn is_superset(&self, other: Rhs) -> bool;
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
