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
//!     ops::{Difference, Union, Subset, Disjoint},
//!     set::RangeSet,
//! };
//!
//! let a = 10..20;
//!
//! // Difference
//! let diff: RangeSet<_> = a.difference(&(15..25)).collect();
//! assert_eq!(diff, RangeSet::from([10..15]));
//!
//! let diff: RangeSet<_> = a.difference(&(12..15)).collect();
//! assert_eq!(diff, RangeSet::from([10..12, 15..20]));
//!
//! // Union
//! let union: RangeSet<_> = a.union(&(15..25)).collect();
//! assert_eq!(union, RangeSet::from([10..25]));
//!
//! let union: RangeSet<_> = a.union(&(0..0)).collect();
//! assert_eq!(union, RangeSet::from([10..20]));
//!
//! // Comparison
//! assert!(a.is_subset(&(0..30)));
//! assert!(a.is_disjoint(&(0..10)));
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
    ops::{
        Difference, DifferenceMut, Disjoint, Intersection, Subset, SymmetricDifference,
        SymmetricDifferenceMut, Union, UnionMut,
    },
    range::Once,
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
}

impl<T: Copy + Ord> RangeSet<T> {
    fn new_from_iter(iter: impl IntoIterator<Item = Range<T>>) -> Self {
        let mut ranges: Vec<_> = iter
            .into_iter()
            .filter(|range| range.start < range.end)
            .collect();

        if ranges.len() <= 1 {
            return Self { ranges };
        }

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

    /// Returns an iterator over the values in the set.
    pub fn iter(&self) -> ValueIter<'_, T> {
        ValueIter {
            iter: self.ranges.iter(),
            current: None,
        }
    }

    /// Returns an iterator over the ranges in the set.
    pub fn iter_ranges(&self) -> RangeIter<'_, T> {
        RangeIter {
            iter: self.ranges.iter(),
        }
    }

    /// Returns `true` if the set contains the given value.
    pub fn contains(&self, value: &T) -> bool {
        self.iter_ranges().seek(value).is_some()
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
            if &prev.start < at && at < &prev.end {
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

impl<'a, T: Copy + Ord> IntoRangeIterator<T> for &'a RangeSet<T> {
    type IntoIter = RangeIter<'a, T>;

    fn into_range_iter(self) -> Self::IntoIter {
        self.iter_ranges()
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

impl<T: Copy + Ord> UnionMut<Range<T>> for RangeSet<T> {
    fn union_mut(&mut self, other: &Range<T>) {
        if other.is_empty() {
            return;
        } else if self.ranges.is_empty() {
            self.ranges.push(other.clone());
            return;
        }

        let ranges = &mut self.ranges;

        let mut i = 0;
        let mut new_range = other.clone();
        while i < ranges.len() {
            // If the new_range comes before the current range without overlapping
            if new_range.end < ranges[i].start {
                ranges.insert(i, new_range);

                return;
            }
            // If the new_range overlaps or is adjacent with the current range
            else if new_range.start <= ranges[i].end {
                // Expand new_range to include the current range
                new_range.start = new_range.start.min(ranges[i].start);
                new_range.end = new_range.end.max(ranges[i].end);
                // Remove the current range as it is now included in new_range
                ranges.remove(i);
            }
            // If the new_range comes after the current range
            else {
                i += 1;
            }
        }

        // If the new_range comes after all the ranges, add it to the end
        ranges.push(new_range);
    }
}

impl<T: Copy + Ord> UnionMut<RangeSet<T>> for RangeSet<T> {
    fn union_mut(&mut self, other: &RangeSet<T>) {
        for range in &other.ranges {
            self.union_mut(range);
        }
    }
}

impl<T: Copy + Ord> Union<Range<T>> for RangeSet<T> {
    type Output<'a>
        = UnionIter<T, RangeIter<'a, T>, Once<T>>
    where
        T: 'a;

    fn union<'a>(&'a self, other: &'a Range<T>) -> Self::Output<'a> {
        self.iter_ranges().union(Once::new(other.clone()))
    }
}

impl<T: Copy + Ord> Union<RangeSet<T>> for RangeSet<T> {
    type Output<'a>
        = UnionIter<T, RangeIter<'a, T>, RangeIter<'a, T>>
    where
        T: 'a;

    fn union<'a>(&'a self, other: &'a RangeSet<T>) -> Self::Output<'a> {
        self.iter_ranges().union(other.iter_ranges())
    }
}

impl<T: Copy + Ord> DifferenceMut<Range<T>> for RangeSet<T> {
    fn difference_mut(&mut self, other: &Range<T>) {
        if other.is_empty() || self.ranges.is_empty() {
            return;
        }

        let mut i = 0;
        let ranges = &mut self.ranges;
        while i < ranges.len() {
            // If the current range is entirely before other
            if ranges[i].end <= other.start {
                // no-op
            }
            // If the current range is entirely after other
            else if ranges[i].start >= other.end {
                // we're done
                break;
            }
            // If the current range is entirely contained within other
            else if ranges[i].is_subset(other) {
                ranges.remove(i);
                continue;
            }
            // If other is a subset of the current range
            else if other.is_subset(&ranges[i]) {
                if ranges[i].start == other.start {
                    ranges[i].start = other.end;
                } else if ranges[i].end == other.end {
                    ranges[i].end = other.start;
                } else {
                    ranges.insert(i + 1, other.end..ranges[i].end);
                    ranges[i].end = other.start;
                }
            } else {
                // Trim end
                if ranges[i].start < other.start {
                    ranges[i].end = other.start;
                }

                // Trim start
                if ranges[i].end > other.end {
                    ranges[i].start = other.end;
                }
            }

            i += 1;
        }
    }
}

impl<T: Copy + Ord> DifferenceMut<RangeSet<T>> for RangeSet<T> {
    fn difference_mut(&mut self, other: &RangeSet<T>) {
        for range in &other.ranges {
            self.difference_mut(range);
        }
    }
}

impl<T: Copy + Ord> Difference<Range<T>> for RangeSet<T> {
    type Output<'a>
        = DifferenceIter<T, RangeIter<'a, T>, Once<T>>
    where
        T: 'a;

    fn difference<'a>(&'a self, other: &'a Range<T>) -> Self::Output<'a> {
        self.iter_ranges().difference(Once::new(other.clone()))
    }
}

impl<T: Copy + Ord> Difference<RangeSet<T>> for RangeSet<T> {
    type Output<'a>
        = DifferenceIter<T, RangeIter<'a, T>, RangeIter<'a, T>>
    where
        T: 'a;

    fn difference<'a>(&'a self, other: &'a RangeSet<T>) -> Self::Output<'a> {
        self.iter_ranges().difference(other.iter_ranges())
    }
}

impl<T: Copy + Ord> Intersection<Range<T>> for RangeSet<T> {
    type Output<'a>
        = IntersectionIter<T, RangeIter<'a, T>, Once<T>>
    where
        T: 'a;

    fn intersection<'a>(&'a self, other: &'a Range<T>) -> Self::Output<'a> {
        self.iter_ranges()
            .intersection(Once::new(other.start..other.end))
    }
}

impl<T: Copy + Ord> Intersection<RangeSet<T>> for RangeSet<T> {
    type Output<'a>
        = IntersectionIter<T, RangeIter<'a, T>, RangeIter<'a, T>>
    where
        T: 'a;

    fn intersection<'a>(&'a self, other: &'a RangeSet<T>) -> Self::Output<'a> {
        self.iter_ranges().intersection(other)
    }
}

impl<T: Copy + Ord> SymmetricDifferenceMut<Range<T>> for RangeSet<T> {
    fn symmetric_difference_mut(&mut self, other: &Range<T>) {
        let intersection = self.intersection(other).into_set();
        self.union_mut(other);
        self.difference_mut(&intersection);
    }
}

impl<T: Copy + Ord> SymmetricDifferenceMut<RangeSet<T>> for RangeSet<T> {
    fn symmetric_difference_mut(&mut self, other: &RangeSet<T>) {
        let intersection = self.intersection(other).into_set();
        self.union_mut(other);
        self.difference_mut(&intersection);
    }
}

impl<T: Copy + Ord> SymmetricDifference<Range<T>> for RangeSet<T> {
    type Output<'a>
        = SymmetricDifferenceIter<T, RangeIter<'a, T>, Once<T>>
    where
        T: 'a;

    fn symmetric_difference<'a>(&'a self, other: &'a Range<T>) -> Self::Output<'a> {
        self.iter_ranges()
            .symmetric_difference(Once::new(other.start..other.end))
    }
}

impl<T: Copy + Ord> SymmetricDifference<RangeSet<T>> for RangeSet<T> {
    type Output<'a>
        = SymmetricDifferenceIter<T, RangeIter<'a, T>, RangeIter<'a, T>>
    where
        T: 'a;

    fn symmetric_difference<'a>(&'a self, other: &'a RangeSet<T>) -> Self::Output<'a> {
        self.iter_ranges().symmetric_difference(other.iter_ranges())
    }
}

impl<T: Copy + Ord> Subset<Range<T>> for RangeSet<T> {
    fn is_subset(&self, other: &Range<T>) -> bool {
        if let Some(range) = self.ranges.first() {
            range.start >= other.start && range.end <= other.end
        } else {
            // empty set is subset of any set
            return true;
        }
    }
}

impl<T: Copy + Ord> Subset<RangeSet<T>> for RangeSet<T> {
    fn is_subset(&self, other: &RangeSet<T>) -> bool {
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

        self.iter_ranges().is_subset(other.iter_ranges())
    }
}

impl<T: Copy + Ord> Disjoint<RangeSet<T>> for RangeSet<T> {
    fn is_disjoint(&self, other: &RangeSet<T>) -> bool {
        self.iter_ranges().is_disjoint(other.iter_ranges())
    }
}

impl<T: Copy + Ord> Disjoint<Range<T>> for RangeSet<T> {
    fn is_disjoint(&self, other: &Range<T>) -> bool {
        other.is_disjoint(self)
    }
}

impl<T: Copy + Ord> BitOrAssign<Range<T>> for RangeSet<T> {
    fn bitor_assign(&mut self, other: Range<T>) {
        self.union_mut(&other);
    }
}

impl<T: Copy + Ord> BitOrAssign<&Range<T>> for RangeSet<T> {
    fn bitor_assign(&mut self, other: &Range<T>) {
        self.union_mut(other);
    }
}

impl<T: Copy + Ord> BitOr<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitor(mut self, other: Range<T>) -> Self::Output {
        self.union_mut(&other);
        self
    }
}

impl<T: Copy + Ord> BitOr<&Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitor(mut self, other: &Range<T>) -> Self::Output {
        self.union_mut(other);
        self
    }
}

impl<T: Copy + Ord> BitOrAssign<RangeSet<T>> for RangeSet<T> {
    fn bitor_assign(&mut self, other: RangeSet<T>) {
        self.union_mut(&other);
    }
}

impl<T: Copy + Ord> BitOrAssign<&RangeSet<T>> for RangeSet<T> {
    fn bitor_assign(&mut self, other: &RangeSet<T>) {
        self.union_mut(other);
    }
}

impl<T: Copy + Ord> BitOr<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitor(mut self, other: RangeSet<T>) -> Self::Output {
        self.union_mut(&other);
        self
    }
}

impl<T: Copy + Ord> BitOr<&RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitor(mut self, other: &RangeSet<T>) -> Self::Output {
        self.union_mut(other);
        self
    }
}

impl<T: Copy + Ord> BitAnd<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitand(self, other: Range<T>) -> Self::Output {
        other.intersection(&self).into_set()
    }
}

impl<T: Copy + Ord> BitAnd<&Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitand(self, other: &Range<T>) -> Self::Output {
        other.intersection(&self).into_set()
    }
}

impl<T: Copy + Ord> BitAndAssign<RangeSet<T>> for RangeSet<T> {
    fn bitand_assign(&mut self, other: RangeSet<T>) {
        *self = self.intersection(&other).into_set();
    }
}

impl<T: Copy + Ord> BitAndAssign<&RangeSet<T>> for RangeSet<T> {
    fn bitand_assign(&mut self, other: &RangeSet<T>) {
        *self = self.intersection(other).into_set();
    }
}

impl<T: Copy + Ord> BitAnd<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitand(self, other: RangeSet<T>) -> Self::Output {
        self.intersection(&other).into_set()
    }
}

impl<T: Copy + Ord> BitAnd<&RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitand(self, other: &RangeSet<T>) -> Self::Output {
        self.intersection(other).into_set()
    }
}

impl<T: Copy + Ord> BitAndAssign<Range<T>> for RangeSet<T> {
    fn bitand_assign(&mut self, other: Range<T>) {
        *self = self.intersection(&other).into_set();
    }
}

impl<T: Copy + Ord> BitAndAssign<&Range<T>> for RangeSet<T> {
    fn bitand_assign(&mut self, other: &Range<T>) {
        *self = self.intersection(other).into_set();
    }
}

impl<T: Copy + Ord> SubAssign<Range<T>> for RangeSet<T> {
    fn sub_assign(&mut self, rhs: Range<T>) {
        self.difference_mut(&rhs);
    }
}

impl<T: Copy + Ord> SubAssign<&Range<T>> for RangeSet<T> {
    fn sub_assign(&mut self, rhs: &Range<T>) {
        self.difference_mut(rhs);
    }
}

impl<T: Copy + Ord> Sub<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn sub(mut self, rhs: Range<T>) -> Self::Output {
        self.difference_mut(&rhs);
        self
    }
}

impl<T: Copy + Ord> Sub<&Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn sub(mut self, rhs: &Range<T>) -> Self::Output {
        self.difference_mut(rhs);
        self
    }
}

impl<T: Copy + Ord> SubAssign<RangeSet<T>> for RangeSet<T> {
    fn sub_assign(&mut self, rhs: RangeSet<T>) {
        self.difference_mut(&rhs);
    }
}

impl<T: Copy + Ord> SubAssign<&RangeSet<T>> for RangeSet<T> {
    fn sub_assign(&mut self, rhs: &RangeSet<T>) {
        self.difference_mut(rhs);
    }
}

impl<T: Copy + Ord> Sub<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn sub(mut self, rhs: RangeSet<T>) -> Self::Output {
        self.difference_mut(&rhs);
        self
    }
}

impl<T: Copy + Ord> Sub<&RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn sub(mut self, rhs: &RangeSet<T>) -> Self::Output {
        self.difference_mut(rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: Range<T>) -> Self::Output {
        self.symmetric_difference_mut(&rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<&Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: &Range<T>) -> Self::Output {
        self.symmetric_difference_mut(rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: RangeSet<T>) -> Self::Output {
        self.symmetric_difference_mut(&rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<&RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: &RangeSet<T>) -> Self::Output {
        self.symmetric_difference_mut(rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<RangeSet<T>> for &RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(self, rhs: RangeSet<T>) -> Self::Output {
        self.symmetric_difference(&rhs).into_set()
    }
}

impl<T: Copy + Ord> BitXorAssign<RangeSet<T>> for RangeSet<T> {
    fn bitxor_assign(&mut self, rhs: RangeSet<T>) {
        self.symmetric_difference_mut(&rhs);
    }
}

impl<T: Copy + Ord> BitXorAssign<&RangeSet<T>> for RangeSet<T> {
    fn bitxor_assign(&mut self, rhs: &RangeSet<T>) {
        self.symmetric_difference_mut(rhs);
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
            let values = set.iter().collect::<Vec<_>>();
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn test_set_range_iter() {
        for set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            let expected = set_ref.iter_ranges().collect::<Vec<_>>();
            let set = RangeSet::from_range_iter(set_ref);
            let values = set.iter_ranges().collect::<Vec<_>>();
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn test_set_split_off() {
        for set_ref in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            for i in 0..TEST_DOMAIN_SIZE {
                let mut a = RangeSet::from_range_iter(set_ref);
                let b = a.split_off(&i);
                for n in a.iter() {
                    assert!(n < i, "{n} should be less than {i}");
                }
                for n in b.iter() {
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
            for (a, b) in set.iter_ranges().zip(set_ref.iter_ranges()) {
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
            for (a, b) in set.iter_ranges().zip(set_ref.iter_ranges()) {
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
            data.index(&index).fold(Vec::default(), |mut vec, slice| {
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
            data.index(&index).flatten().copied().collect::<Vec<_>>(),
            vec![]
        );
    }

    #[test]
    #[should_panic]
    fn test_set_index_ranges_out_of_bounds_slice() {
        let data = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let index = RangeSet::from([(0..3), (5..8), (10..12)]);

        data.index(&index).for_each(drop);
    }
}
