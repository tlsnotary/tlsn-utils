//! A set of values represented using ranges.
//!
//! A `RangeSet` is similar to any other kind of set, such as `HashSet`, with
//! the difference being that the values in the set are represented using ranges
//! rather than storing each value individually.
//!
//! # Invariants
//!
//! `RangeSet` enforces the following invariants on the ranges it contains:
//!
//! - The ranges are sorted.
//! - The ranges are non-adjacent.
//! - The ranges are non-intersecting.
//! - The ranges are non-empty.
//!
//! This is enforced in the constructor, and guaranteed to hold after applying
//! any operation on a range or set.
//!
//! # Examples
//!
//! ```
//! use rangeset::{
//!     ops::Set,
//!     set::RangeSet,
//! };
//!
//! let a = 10..20;
//!
//! // Difference
//! let diff: RangeSet<_> = a.difference(15..25).collect();
//! assert_eq!(diff, RangeSet::from([10..15]));
//!
//! let diff: RangeSet<_> = a.difference(12..15).collect();
//! assert_eq!(diff, RangeSet::from([10..12, 15..20]));
//!
//! // Union
//! let union: RangeSet<_> = a.union(15..25).collect();
//! assert_eq!(union, RangeSet::from([10..25]));
//!
//! let union: RangeSet<_> = a.union(0..0).collect();
//! assert_eq!(union, RangeSet::from([10..20]));
//!
//! // Comparison
//! assert!(a.is_subset(0..30));
//! assert!(a.is_disjoint(0..10));
//! assert_eq!(a.clone(), RangeSet::from(a));
//! ```

extern crate alloc;

use alloc::vec::Vec;

use core::ops::{
    Add, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Range, Sub, SubAssign,
};

use crate::{
    Step,
    iter::{
        DifferenceIter, FromRangeIterator, IntersectionIter, IntoRangeIterator, RangeIterator,
        SymmetricDifferenceIter, UnionIter,
    },
    ops::Set,
};

/// Set of values stored as ranges.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(
        bound = "for<'a> T: serde::Serialize + serde::de::Deserialize<'a> + Copy + Ord",
        from = "Vec<Range<T>>",
        into = "Vec<Range<T>>"
    )
)]
pub struct RangeSet<T> {
    /// The ranges of the set.
    ///
    /// The ranges *MUST* be sorted, non-adjacent, non-intersecting, and
    /// non-empty.
    ranges: Vec<Range<T>>,
}

/// Sorts and merges the ranges in the given vector. This function assumes the
/// vector does not contain empty ranges.
fn sort_merge<T: Copy + Ord>(ranges: &mut Vec<Range<T>>) {
    if ranges.len() <= 1 {
        return;
    }

    debug_assert!(
        ranges.iter().all(|range| !range.is_empty()),
        "vector contains empty ranges"
    );

    ranges.sort_unstable_by(|a, b| match a.start.cmp(&b.start) {
        // If the ranges start at the same value, sort by the end.
        core::cmp::Ordering::Equal => a.end.cmp(&b.end),
        ord => ord,
    });

    // Merge ranges.
    let mut i = 0;
    let mut current = ranges[0].clone();
    for j in 1..ranges.len() {
        let candidate = ranges[j].clone();
        if candidate.start <= current.end {
            if candidate.end > current.end {
                // Merge the ranges if they are adjacent or overlap.
                current.end = candidate.end;
            }
        } else {
            // Otherwise, keep the current range and start a new one.
            ranges[i] = current;
            i += 1;
            current = candidate;
        }
    }
    ranges[i] = current;
    ranges.truncate(i + 1);
}

impl<T: Copy + Ord> From<Vec<Range<T>>> for RangeSet<T> {
    fn from(ranges: Vec<Range<T>>) -> Self {
        Self::new_from_slice(&ranges)
    }
}

impl<T> From<RangeSet<T>> for Vec<Range<T>> {
    fn from(ranges: RangeSet<T>) -> Self {
        ranges.into_inner()
    }
}

impl<T: Copy + Ord> Default for RangeSet<T> {
    fn default() -> Self {
        Self {
            ranges: Default::default(),
        }
    }
}

impl<T> RangeSet<T> {
    /// Returns the ranges of the set.
    pub fn into_inner(self) -> Vec<Range<T>> {
        self.ranges
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Returns the number of ranges in the set.
    pub fn len_ranges(&self) -> usize {
        self.ranges.len()
    }

    /// Clears the set, removing all ranges.
    pub fn clear(&mut self) {
        self.ranges.clear();
    }

    /// Returns an iterator over the values in the set.
    pub fn iter_values(&self) -> ValueIter<'_, T> {
        ValueIter {
            iter: self.ranges.iter(),
            current: None,
        }
    }

    /// Returns an iterator over the ranges in the set.
    pub fn iter(&self) -> RangeIter<'_, T> {
        RangeIter {
            iter: self.ranges.iter(),
        }
    }
}

impl<T: Copy + Ord> RangeSet<T> {
    fn new_from_iter(iter: impl IntoIterator<Item = Range<T>>) -> Self {
        let mut ranges: Vec<_> = iter
            .into_iter()
            .filter(|range| range.start < range.end)
            .collect();

        sort_merge(&mut ranges);

        Self { ranges }
    }

    fn new_from_iter_borrow<'a>(iter: impl IntoIterator<Item = &'a Range<T>>) -> Self
    where
        T: 'a,
    {
        Self::new_from_iter(iter.into_iter().map(|range| range.start..range.end))
    }

    /// Returns a new `RangeSet` from the given ranges.
    ///
    /// The `RangeSet` is constructed by computing the union of the given
    /// ranges.
    pub fn new_from_slice(ranges: &[Range<T>]) -> Self {
        Self::new_from_iter_borrow(ranges)
    }

    /// Returns `true` if the set contains the given value.
    pub fn contains(&self, value: &T) -> bool {
        self.iter().seek(value).is_some()
    }

    /// Returns the minimum value in the set, or `None` if the set is empty.
    pub fn min(&self) -> Option<T> {
        self.ranges.first().map(|range| range.start)
    }

    /// Returns the end of right-most range in the set, or `None` if the set is
    /// empty.
    ///
    /// # Note
    ///
    /// This is the *non-inclusive* bound of the right-most range. See
    /// `RangeSet::max` for the maximum value in the set.
    pub fn end(&self) -> Option<T> {
        self.ranges.last().map(|range| range.end)
    }

    /// Unions in-place with the given ranges.
    pub fn union_mut(&mut self, other: impl IntoRangeIterator<T>) {
        self.ranges.extend(other.into_range_iter());
        sort_merge(&mut self.ranges);
    }

    /// Differences in-place with the given ranges.
    pub fn difference_mut(&mut self, other: impl IntoRangeIterator<T>) {
        // TODO: optimize this.
        *self = self.iter().difference(other).into_set();
    }

    /// Intersects in-place with the given ranges.
    pub fn intersection_mut(&mut self, other: impl IntoRangeIterator<T>) {
        // TODO: optimize this.
        *self = self.iter().intersection(other).into_set();
    }

    /// Symmetric differences in-place with the given ranges.
    pub fn symmetric_difference_mut(&mut self, other: impl IntoRangeIterator<T>) {
        // TODO: optimize this.
        *self = self.iter().symmetric_difference(other).into_set();
    }
}

impl<T: Copy + Ord + Step + Sub<Output = T>> RangeSet<T> {
    /// Returns the maximum value in the set, or `None` if the set is empty.
    pub fn max(&self) -> Option<T> {
        // This should never underflow because of the invariant that a set
        // never contains empty ranges.
        self.ranges
            .last()
            .map(|range| Step::backward(range.end, 1).expect("set is not empty"))
    }

    /// Splits the set into two at the provided value.
    ///
    /// Returns a new set containing all the existing values `>= at`. After the
    /// call, the original set will be left containing the values `< at`.
    pub fn split_off(&mut self, at: &T) -> Self {
        if self.ranges.is_empty() {
            return Self::default();
        }

        let idx = self.ranges.partition_point(|range| range.start < *at);
        if idx > 0 {
            let prev = &mut self.ranges[idx - 1];
            if at < &prev.end {
                let old_end = prev.end;
                prev.end = *at;

                let mut right = Vec::with_capacity(self.ranges.len() - idx);
                right.push(*at..old_end);
                right.extend_from_slice(&self.ranges[idx..]);
                self.ranges.truncate(idx);

                Self { ranges: right }
            } else {
                Self {
                    ranges: self.ranges.split_off(idx),
                }
            }
        } else {
            Self {
                ranges: core::mem::take(&mut self.ranges),
            }
        }
    }
}

impl<T: Copy + Sub<Output = T>> RangeSet<T> {
    /// Shifts every range in the set to the left by the provided offset.
    ///
    /// # Panics
    ///
    /// Panics if the shift causes an underflow.
    pub fn shift_left(&mut self, offset: &T) {
        self.ranges.iter_mut().for_each(|range| {
            range.start = range.start - *offset;
            range.end = range.end - *offset;
        });
    }
}

impl<T: Copy + Add<Output = T>> RangeSet<T> {
    /// Shifts every range in the set to the right by the provided offset.
    ///
    /// # Panics
    ///
    /// Panics if the the shift causes an overflow.
    pub fn shift_right(&mut self, offset: &T) {
        self.ranges.iter_mut().for_each(|range| {
            range.start = range.start + *offset;
            range.end = range.end + *offset;
        });
    }
}

impl<T: Copy + Ord> RangeSet<T>
where
    Range<T>: ExactSizeIterator<Item = T>,
{
    /// Returns the number of values in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ranges.iter().map(|range| range.len()).sum()
    }
}

impl<T> FromRangeIterator<T> for RangeSet<T> {
    fn from_range_iter<I>(iter: I) -> Self
    where
        I: IntoRangeIterator<T>,
    {
        Self {
            ranges: iter.into_range_iter().collect(),
        }
    }
}

impl<T: Copy + Ord> IntoRangeIterator<T> for RangeSet<T> {
    type IntoIter = IntoRangeIter<T>;

    fn into_range_iter(self) -> Self::IntoIter {
        IntoRangeIter {
            pos: 0,
            ranges: self.ranges,
        }
    }
}

impl<'a, T: Copy + Ord> IntoRangeIterator<T> for &'a RangeSet<T> {
    type IntoIter = RangeIter<'a, T>;

    fn into_range_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Copy + Ord> TryFrom<RangeSet<T>> for Range<T> {
    type Error = RangeSet<T>;

    /// Attempts to convert a `RangeSet` into a single `Range`, returning the
    /// set if it does not contain exactly one range.
    fn try_from(set: RangeSet<T>) -> Result<Self, Self::Error> {
        if set.len_ranges() == 1 {
            Ok(set.ranges.into_iter().next().unwrap())
        } else {
            Err(set)
        }
    }
}

impl<T: Copy + Ord> From<Range<T>> for RangeSet<T> {
    fn from(range: Range<T>) -> Self {
        if range.is_empty() {
            return Self::default();
        }

        Self {
            ranges: Vec::from([range]),
        }
    }
}

impl<const N: usize, T: Copy + Ord> From<[Range<T>; N]> for RangeSet<T> {
    fn from(ranges: [Range<T>; N]) -> Self {
        Self::new_from_slice(&ranges)
    }
}

impl<T: Copy + Ord> FromIterator<Range<T>> for RangeSet<T> {
    fn from_iter<I: IntoIterator<Item = Range<T>>>(iter: I) -> Self {
        Self::new_from_iter(iter)
    }
}

impl<'a, T: Copy + Ord> FromIterator<&'a Range<T>> for RangeSet<T> {
    fn from_iter<I: IntoIterator<Item = &'a Range<T>>>(iter: I) -> Self {
        Self::new_from_iter_borrow(iter)
    }
}

impl<T: Copy + Ord> From<&[Range<T>]> for RangeSet<T> {
    fn from(ranges: &[Range<T>]) -> Self {
        Self::new_from_slice(ranges)
    }
}

impl<T: Copy + Ord> PartialEq<Range<T>> for RangeSet<T> {
    fn eq(&self, other: &Range<T>) -> bool {
        self.ranges.len() == 1 && self.ranges[0] == *other
    }
}

impl<T: Copy + Ord> PartialEq<Range<T>> for &RangeSet<T> {
    fn eq(&self, other: &Range<T>) -> bool {
        *self == other
    }
}

impl<T: Copy + Ord> PartialEq<RangeSet<T>> for Range<T> {
    fn eq(&self, other: &RangeSet<T>) -> bool {
        other == self
    }
}

impl<T: Copy + Ord> PartialEq<RangeSet<T>> for &Range<T> {
    fn eq(&self, other: &RangeSet<T>) -> bool {
        other == *self
    }
}

/// Iterator over the ranges in [`RangeSet`].
pub struct IntoRangeIter<T> {
    pos: usize,
    ranges: Vec<Range<T>>,
}

impl<T: Copy> Iterator for IntoRangeIter<T> {
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let next = self.ranges.get(self.pos);
        self.pos += 1;
        next.cloned()
    }
}

impl<T: Copy + Ord> RangeIterator<T> for IntoRangeIter<T> {
    #[inline]
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>> {
        self.pos = self.ranges.partition_point(|range| &range.end < n);
        self.next()
    }
}

/// Iterator over the values in [`RangeSet`].
pub struct ValueIter<'a, T> {
    iter: core::slice::Iter<'a, Range<T>>,
    current: Option<Range<T>>,
}

impl<T> Iterator for ValueIter<'_, T>
where
    T: Copy + Ord,
    Range<T>: Iterator<Item = T>,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(range) = &mut self.current {
            if let Some(value) = range.next() {
                return Some(value);
            } else {
                self.current = None;
                return self.next();
            }
        }

        if let Some(range) = self.iter.next() {
            self.current = Some(range.clone());
            return self.next();
        }

        None
    }
}

/// Iterator over the ranges in [`RangeSet`].
#[derive(Debug)]
pub struct RangeIter<'a, T> {
    iter: core::slice::Iter<'a, Range<T>>,
}

impl<T> Iterator for RangeIter<'_, T>
where
    T: Copy,
{
    type Item = Range<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|range| range.start..range.end)
    }
}

impl<'a, T> RangeIterator<T> for RangeIter<'a, T>
where
    T: Copy,
{
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        let ranges = self.iter.as_slice();
        let pos = ranges.partition_point(|range| &range.end < n);
        self.iter = ranges[pos..].iter();
        self.next()
    }
}

impl<'a, T: Copy + Ord> IntoRangeIterator<T> for RangeIter<'a, T> {
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

impl<T> ExactSizeIterator for RangeIter<'_, T>
where
    T: Copy,
{
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<T> DoubleEndedIterator for RangeIter<'_, T>
where
    T: Copy,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().cloned()
    }
}

/// A type which has a corresponding range set.
pub trait ToRangeSet<T: Copy + Ord> {
    /// Returns a corresponding range set.
    fn to_range_set(&self) -> RangeSet<T>;
}

impl<T: Copy + Ord> ToRangeSet<T> for RangeSet<T> {
    fn to_range_set(&self) -> RangeSet<T> {
        self.clone()
    }
}

impl<T> Set<RangeSet<T>> for RangeSet<T>
where
    T: Copy + Ord,
{
    type Union<'a>
        = UnionIter<T, RangeIter<'a, T>, IntoRangeIter<T>>
    where
        T: 'a;

    type Difference<'a>
        = DifferenceIter<T, RangeIter<'a, T>, IntoRangeIter<T>>
    where
        T: 'a;

    type Intersection<'a>
        = IntersectionIter<T, RangeIter<'a, T>, IntoRangeIter<T>>
    where
        T: 'a;

    type SymmetricDifference<'a>
        = SymmetricDifferenceIter<T, RangeIter<'a, T>, IntoRangeIter<T>>
    where
        T: 'a;

    fn union<'a>(&'a self, rhs: RangeSet<T>) -> Self::Union<'a>
    where
        T: 'a,
    {
        self.iter().union(rhs)
    }

    fn difference<'a>(&'a self, rhs: RangeSet<T>) -> Self::Difference<'a>
    where
        T: 'a,
    {
        self.iter().difference(rhs)
    }

    fn intersection<'a>(&'a self, rhs: RangeSet<T>) -> Self::Intersection<'a>
    where
        T: 'a,
    {
        self.iter().intersection(rhs)
    }

    fn symmetric_difference<'a>(&'a self, rhs: RangeSet<T>) -> Self::SymmetricDifference<'a>
    where
        T: 'a,
    {
        self.iter().symmetric_difference(rhs)
    }

    fn is_disjoint(&self, other: RangeSet<T>) -> bool {
        self.iter().is_disjoint(other)
    }

    fn is_subset(&self, other: RangeSet<T>) -> bool {
        self.is_subset(&other)
    }

    fn is_superset(&self, other: RangeSet<T>) -> bool {
        self.is_superset(&other)
    }
}

impl<'rhs, T> Set<&'rhs RangeSet<T>> for RangeSet<T>
where
    T: Copy + Ord,
{
    type Union<'a>
        = UnionIter<T, RangeIter<'a, T>, RangeIter<'rhs, T>>
    where
        'rhs: 'a,
        T: 'a;

    type Difference<'a>
        = DifferenceIter<T, RangeIter<'a, T>, RangeIter<'rhs, T>>
    where
        'rhs: 'a,
        T: 'a;

    type Intersection<'a>
        = IntersectionIter<T, RangeIter<'a, T>, RangeIter<'rhs, T>>
    where
        'rhs: 'a,
        T: 'a;

    type SymmetricDifference<'a>
        = SymmetricDifferenceIter<T, RangeIter<'a, T>, RangeIter<'rhs, T>>
    where
        'rhs: 'a,
        T: 'a;

    fn union<'a>(&'a self, rhs: &'rhs RangeSet<T>) -> Self::Union<'a>
    where
        'rhs: 'a,
    {
        self.iter().union(rhs)
    }

    fn difference<'a>(&'a self, rhs: &'rhs RangeSet<T>) -> Self::Difference<'a>
    where
        'rhs: 'a,
    {
        self.iter().difference(rhs)
    }

    fn intersection<'a>(&'a self, rhs: &'rhs RangeSet<T>) -> Self::Intersection<'a>
    where
        'rhs: 'a,
    {
        self.iter().intersection(rhs)
    }

    fn symmetric_difference<'a>(&'a self, rhs: &'rhs RangeSet<T>) -> Self::SymmetricDifference<'a>
    where
        'rhs: 'a,
    {
        self.iter().symmetric_difference(rhs)
    }

    fn is_disjoint(&self, other: &'rhs RangeSet<T>) -> bool {
        self.iter().is_disjoint(other)
    }

    fn is_subset(&self, other: &'rhs RangeSet<T>) -> bool {
        if self.ranges.is_empty() {
            // empty set is subset of any set
            return true;
        } else if other.ranges.is_empty() {
            // non-empty set is not subset of empty set
            return false;
        }

        // Boundary short circuit.
        if self.min().unwrap() < other.min().unwrap() || self.end().unwrap() > other.end().unwrap()
        {
            // Check if self's start & end are contained within (or same as) other's.
            return false;
        }

        self.iter().is_subset(other)
    }

    fn is_superset(&self, other: &'rhs RangeSet<T>) -> bool {
        self.iter().is_superset(other)
    }
}

impl<T> Set<Range<T>> for RangeSet<T>
where
    T: Copy + Ord,
{
    type Union<'a>
        = UnionIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        T: 'a;

    type Difference<'a>
        = DifferenceIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        T: 'a;

    type Intersection<'a>
        = IntersectionIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        T: 'a;

    type SymmetricDifference<'a>
        = SymmetricDifferenceIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        T: 'a;

    fn union<'a>(&'a self, rhs: Range<T>) -> Self::Union<'a>
    where
        T: 'a,
    {
        self.iter().union(rhs)
    }

    fn difference<'a>(&'a self, rhs: Range<T>) -> Self::Difference<'a>
    where
        T: 'a,
    {
        self.iter().difference(rhs)
    }

    fn intersection<'a>(&'a self, rhs: Range<T>) -> Self::Intersection<'a>
    where
        T: 'a,
    {
        self.iter().intersection(rhs)
    }

    fn symmetric_difference<'a>(&'a self, rhs: Range<T>) -> Self::SymmetricDifference<'a>
    where
        T: 'a,
    {
        self.iter().symmetric_difference(rhs)
    }

    fn is_disjoint(&self, other: Range<T>) -> bool {
        self.iter().is_disjoint(other)
    }

    fn is_subset(&self, other: Range<T>) -> bool {
        self.is_subset(&other)
    }

    fn is_superset(&self, other: Range<T>) -> bool {
        self.is_superset(&other)
    }
}

impl<'rhs, T> Set<&'rhs Range<T>> for RangeSet<T>
where
    T: Copy + Ord,
{
    type Union<'a>
        = UnionIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        'rhs: 'a,
        T: 'a;

    type Difference<'a>
        = DifferenceIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        'rhs: 'a,
        T: 'a;

    type Intersection<'a>
        = IntersectionIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        'rhs: 'a,
        T: 'a;

    type SymmetricDifference<'a>
        = SymmetricDifferenceIter<T, RangeIter<'a, T>, <Range<T> as IntoRangeIterator<T>>::IntoIter>
    where
        'rhs: 'a,
        T: 'a;

    fn union<'a>(&'a self, rhs: &'rhs Range<T>) -> Self::Union<'a>
    where
        'rhs: 'a,
        T: 'a,
    {
        self.iter().union(rhs)
    }

    fn difference<'a>(&'a self, rhs: &'rhs Range<T>) -> Self::Difference<'a>
    where
        'rhs: 'a,
    {
        self.iter().difference(rhs)
    }

    fn intersection<'a>(&'a self, rhs: &'rhs Range<T>) -> Self::Intersection<'a>
    where
        'rhs: 'a,
    {
        self.iter().intersection(rhs)
    }

    fn symmetric_difference<'a>(&'a self, rhs: &'rhs Range<T>) -> Self::SymmetricDifference<'a>
    where
        'rhs: 'a,
    {
        self.iter().symmetric_difference(rhs)
    }

    fn is_disjoint(&self, other: &'rhs Range<T>) -> bool {
        self.iter().is_disjoint(other)
    }

    fn is_subset(&self, other: &'rhs Range<T>) -> bool {
        if let Some(range) = self.ranges.first() {
            range.start >= other.start && range.end <= other.end
        } else {
            // empty set is subset of any set
            true
        }
    }

    fn is_superset(&self, other: &'rhs Range<T>) -> bool {
        self.iter().is_superset(other)
    }
}

impl<I, T> BitOrAssign<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    fn bitor_assign(&mut self, other: I) {
        self.union_mut(other);
    }
}

impl<I, T> BitOr<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    type Output = UnionIter<T, IntoRangeIter<T>, I::IntoIter>;

    fn bitor(self, other: I) -> Self::Output {
        self.into_range_iter().union(other)
    }
}

impl<I, T> BitAnd<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    type Output = IntersectionIter<T, IntoRangeIter<T>, I::IntoIter>;

    fn bitand(self, other: I) -> Self::Output {
        self.into_range_iter().intersection(other)
    }
}

impl<I, T> BitAndAssign<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    fn bitand_assign(&mut self, other: I) {
        self.intersection_mut(other);
    }
}

impl<I, T> SubAssign<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    fn sub_assign(&mut self, other: I) {
        self.difference_mut(other);
    }
}

impl<I, T> Sub<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    type Output = DifferenceIter<T, IntoRangeIter<T>, I::IntoIter>;

    fn sub(self, other: I) -> Self::Output {
        self.into_range_iter().difference(other)
    }
}

impl<I, T> BitXor<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    type Output = SymmetricDifferenceIter<T, IntoRangeIter<T>, I::IntoIter>;

    fn bitxor(self, other: I) -> Self::Output {
        self.into_range_iter().symmetric_difference(other)
    }
}

impl<I, T> BitXorAssign<I> for RangeSet<T>
where
    I: IntoRangeIterator<T>,
    T: Copy + Ord,
{
    fn bitxor_assign(&mut self, other: I) {
        self.symmetric_difference_mut(other);
    }
}

#[cfg(test)]
#[allow(clippy::all)]
mod tests {
    use super::*;
    use crate::{
        ops::Index,
        test::{Set, TEST_DOMAIN_SIZE, Universe},
    };

    #[test]
    fn test_set_value_iter() {
        for set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            let expected = set_ref.iter_ranges().flatten().collect::<Vec<_>>();
            let set = RangeSet::from_range_iter(set_ref);
            let values = set.iter_values().collect::<Vec<_>>();
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn test_set_range_iter() {
        for set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            let expected = set_ref.iter_ranges().collect::<Vec<_>>();
            let set = RangeSet::from_range_iter(set_ref);
            let values = set.iter().collect::<Vec<_>>();
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn test_set_split_off() {
        for set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            for i in 0..TEST_DOMAIN_SIZE {
                let mut a = RangeSet::from_range_iter(set_ref);
                let b = a.split_off(&i);
                for n in a.iter_values() {
                    assert!(n < i, "{n} should be less than {i}");
                }
                for n in b.iter_values() {
                    assert!(n >= i, "{n} should be greater than or equal to {i}");
                }
            }
        }
    }

    #[test]
    fn test_set_shift_left() {
        for mut set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            set_ref = set_ref - Set::new(1);
            let mut set: RangeSet<usize> = RangeSet::from_range_iter(set_ref);
            set.shift_left(&1);
            set_ref.shift_left(1);
            for (a, b) in set.iter().zip(set_ref.iter_ranges()) {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn test_set_shift_right() {
        for mut set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            let mut set: RangeSet<usize> = RangeSet::from_range_iter(set_ref);
            set.shift_right(&1);
            set_ref.shift_right(1);
            for (a, b) in set.iter().zip(set_ref.iter_ranges()) {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn test_set_max() {
        for set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            let set: RangeSet<usize> = RangeSet::from_range_iter(set_ref);
            if set_ref.is_empty() {
                assert_eq!(set.max(), None);
            } else {
                assert_eq!(set.max(), Some(set_ref.max().unwrap()));
            }
        }
    }

    #[test]
    fn test_set_contains() {
        let uni = Universe::new(TEST_DOMAIN_SIZE);
        for set_ref in uni.iter_sets() {
            let set: RangeSet<usize> = RangeSet::from_range_iter(set_ref);
            for value in set_ref.iter_ranges().flatten() {
                assert!(set.contains(&value));
            }
            for value in (uni.full() - set_ref).iter_ranges().flatten() {
                assert!(!set.contains(&value));
            }
        }
    }

    #[test]
    fn test_set_index_slice() {
        let data = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let index = RangeSet::from([(0..3), (5..8)]);

        assert_eq!(
            data.index(index).fold(Vec::default(), |mut vec, slice| {
                vec.extend_from_slice(slice);
                vec
            }),
            vec![1, 2, 3, 6, 7, 8]
        );
    }

    #[test]
    fn test_set_index_empty_slice() {
        let data = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let index = RangeSet::from([]);

        assert_eq!(
            data.index(index).flatten().copied().collect::<Vec<_>>(),
            vec![]
        );
    }

    #[test]
    #[should_panic]
    fn test_set_index_ranges_out_of_bounds_slice() {
        let data = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let index = RangeSet::from([(0..3), (5..8), (10..12)]);

        data.index(index).for_each(drop);
    }
}
