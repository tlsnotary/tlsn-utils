mod difference;
mod index;
mod union;

pub use index::IndexRanges;

use std::ops::Range;

/// A set of values represented using ranges.
///
/// A `RangeSet` is similar to any other kind of set, such as `HashSet`, with the difference being that the
/// values in the set are represented using ranges rather than storing each value individually.
///
/// # Invariants
///
/// `RangeSet` enforces the following invariants on the ranges it contains:
///
/// - The ranges are sorted.
/// - The ranges are non-adjacent.
/// - The ranges are non-intersecting.
/// - The ranges are non-empty.
///
/// This is enforced in the constructor, and guaranteed to hold after applying any operation on a range or set.
///
/// # Examples
///
/// ```
/// use utils::range::*;
///
/// let a = 10..20;
///
/// // Difference
/// assert_eq!(a.difference(&(15..25)), RangeSet::from([10..15]));
/// assert_eq!(a.difference(&(12..15)), RangeSet::from([(10..12), (15..20)]));
///
/// // Union
/// assert_eq!(a.union(&(15..25)), RangeSet::from([10..25]));
/// assert_eq!(a.union(&(0..0)), RangeSet::from([10..20]));
///
/// // Comparison
/// assert!(a.is_superset(&(15..18)));
/// assert!(a.is_subset(&(0..30)));
/// assert!(a.is_disjoint(&(0..10)));
/// assert_eq!(a.clone(), RangeSet::from(a));
/// ```
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(from = "Vec<Range<T>>", into = "Vec<Range<T>>")
)]
pub struct RangeSet<T: Copy + Ord> {
    /// The ranges of the set.
    ///
    /// The ranges *MUST* be sorted, non-adjacent, non-intersecting, and non-empty.
    ranges: Vec<Range<T>>,
}

impl<T: Copy + Ord> From<Vec<Range<T>>> for RangeSet<T> {
    fn from(ranges: Vec<Range<T>>) -> Self {
        Self::new(&ranges)
    }
}

impl<T: Copy + Ord> From<RangeSet<T>> for Vec<Range<T>> {
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

impl<T: Copy + Ord> RangeSet<T> {
    /// Returns a new `RangeSet` from the given ranges.
    ///
    /// The `RangeSet` is constructed by computing the union of the given ranges.
    pub fn new(ranges: &[Range<T>]) -> Self
    where
        Self: RangeUnion<Range<T>, Output = Self>,
    {
        let mut set = Self::default();

        for range in ranges {
            set = set.union(range);
        }

        set
    }

    /// Returns the ranges of the set.
    pub fn into_inner(self) -> Vec<Range<T>> {
        self.ranges
    }

    /// Returns an iterator over the values in the set.
    pub fn iter(&self) -> RangeSetIter<'_, T> {
        RangeSetIter {
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
        self.ranges.iter().any(|range| range.contains(value))
    }

    /// Returns the minimum value in the set, or `None` if the set is empty.
    pub fn min(&self) -> Option<T> {
        self.ranges.first().map(|range| range.start)
    }

    /// Returns the maximum value in the set, or `None` if the set is empty.
    pub fn max(&self) -> Option<T> {
        self.ranges.last().map(|range| range.end)
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

    /// Returns `true` if the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        Self::new(&ranges)
    }
}

impl<T: Copy + Ord> From<&[Range<T>]> for RangeSet<T> {
    fn from(ranges: &[Range<T>]) -> Self {
        Self::new(ranges)
    }
}

impl<T: Copy + Ord> PartialEq<Range<T>> for RangeSet<T> {
    fn eq(&self, other: &Range<T>) -> bool {
        self.ranges.len() == 1 && self.ranges[0] == *other
    }
}

impl<T: Copy + Ord> PartialEq<RangeSet<T>> for Range<T> {
    fn eq(&self, other: &RangeSet<T>) -> bool {
        other == self
    }
}

/// An iterator over the values in a `RangeSet`.
pub struct RangeSetIter<'a, T> {
    iter: std::slice::Iter<'a, Range<T>>,
    current: Option<Range<T>>,
}

impl<'a, T> Iterator for RangeSetIter<'a, T>
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

/// An iterator over the ranges in a `RangeSet`.
pub struct RangeIter<'a, T> {
    iter: std::slice::Iter<'a, Range<T>>,
}

impl<'a, T> Iterator for RangeIter<'a, T>
where
    T: Copy + Ord,
    Range<T>: Iterator<Item = T>,
{
    type Item = Range<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().cloned()
    }
}

impl<'a, T> ExactSizeIterator for RangeIter<'a, T>
where
    T: Copy + Ord,
    Range<T>: Iterator<Item = T>,
{
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<'a, T> DoubleEndedIterator for RangeIter<'a, T>
where
    T: Copy + Ord,
    Range<T>: Iterator<Item = T>,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().cloned()
    }
}

pub trait RangeDisjoint<Rhs> {
    /// Returns `true` if the range is disjoint with `other`.
    #[must_use]
    fn is_disjoint(&self, other: &Rhs) -> bool;
}

pub trait RangeSuperset<Rhs> {
    /// Returns `true` if `self` is a superset of `other`.
    #[must_use]
    fn is_superset(&self, other: &Rhs) -> bool;
}

pub trait RangeSubset<Rhs> {
    /// Returns `true` if `self` is a subset of `other`.
    #[must_use]
    fn is_subset(&self, other: &Rhs) -> bool;
}

pub trait RangeDifference<Rhs> {
    type Output;

    /// Returns the set difference of `self` and `other`.
    #[must_use]
    fn difference(&self, other: &Rhs) -> Self::Output;
}

pub trait RangeUnion<Rhs> {
    type Output;

    /// Returns the set union of `self` and `other`.
    #[must_use]
    fn union(&self, other: &Rhs) -> Self::Output;
}

impl<T: Copy + Ord> RangeDisjoint<Range<T>> for Range<T> {
    fn is_disjoint(&self, other: &Range<T>) -> bool {
        self.start >= other.end || self.end <= other.start
    }
}

impl<T: Copy + Ord> RangeDisjoint<RangeSet<T>> for Range<T> {
    fn is_disjoint(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().all(|range| self.is_disjoint(range))
    }
}

impl<T: Copy + Ord> RangeDisjoint<RangeSet<T>> for RangeSet<T> {
    fn is_disjoint(&self, other: &RangeSet<T>) -> bool {
        self.ranges.iter().all(|range| range.is_disjoint(other))
    }
}

impl<T: Copy + Ord> RangeDisjoint<Range<T>> for RangeSet<T> {
    fn is_disjoint(&self, other: &Range<T>) -> bool {
        other.is_disjoint(self)
    }
}

impl<T: Copy + Ord> RangeSuperset<Range<T>> for Range<T> {
    fn is_superset(&self, other: &Range<T>) -> bool {
        self.start <= other.start && self.end >= other.end
    }
}

impl<T: Copy + Ord> RangeSuperset<RangeSet<T>> for Range<T> {
    fn is_superset(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().all(|range| self.is_superset(range))
    }
}

impl<T: Copy + Ord> RangeSubset<Range<T>> for Range<T> {
    fn is_subset(&self, other: &Range<T>) -> bool {
        self.start >= other.start && self.end <= other.end
    }
}

impl<T: Copy + Ord> RangeSubset<RangeSet<T>> for Range<T> {
    fn is_subset(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().any(|range| self.is_subset(range))
    }
}

#[cfg(test)]
#[allow(clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_range_disjoint() {
        let a = 10..20;

        // rightward
        assert!(a.is_disjoint(&(20..30)));
        // rightward aligned
        assert!(!a.is_disjoint(&(19..25)));
        // leftward
        assert!(a.is_disjoint(&(0..10)));
        // leftward aligned
        assert!(!a.is_disjoint(&(5..11)));
        // rightward subset
        assert!(!a.is_disjoint(&(15..25)));
        // leftward subset
        assert!(!a.is_disjoint(&(5..15)));
        // superset
        assert!(!a.is_disjoint(&(5..25)));
        // equal
        assert!(!a.is_disjoint(&(10..20)));
    }

    #[test]
    fn test_range_superset() {
        let a = 10..20;

        // rightward
        assert!(!a.is_superset(&(20..30)));
        // rightward aligned
        assert!(!a.is_superset(&(19..25)));
        // leftward
        assert!(!a.is_superset(&(0..10)));
        // leftward aligned
        assert!(!a.is_superset(&(5..11)));
        // rightward subset
        assert!(a.is_superset(&(15..20)));
        // leftward subset
        assert!(a.is_superset(&(10..15)));
        // superset
        assert!(!a.is_superset(&(5..25)));
        // equal
        assert!(a.is_superset(&(10..20)));
    }

    #[test]
    fn test_range_subset() {
        let a = 10..20;

        // rightward
        assert!(!a.is_subset(&(20..30)));
        // rightward aligned
        assert!(!a.is_subset(&(19..25)));
        // leftward
        assert!(!a.is_subset(&(0..10)));
        // leftward aligned
        assert!(!a.is_subset(&(5..11)));
        // rightward subset
        assert!(!a.is_subset(&(15..20)));
        // leftward subset
        assert!(!a.is_subset(&(10..15)));
        // superset
        assert!(a.is_subset(&(5..25)));
        // equal
        assert!(a.is_subset(&(10..20)));
    }

    #[test]
    fn test_range_set_iter() {
        let a = RangeSet::from([(10..20), (30..40), (50..60)]);

        let values = a.iter().collect::<Vec<_>>();
        let expected_values = (10..20).chain(30..40).chain(50..60).collect::<Vec<_>>();
        assert_eq!(values, expected_values);
    }

    #[test]
    fn test_range_iter() {
        let a = RangeSet::from([(10..20), (30..40), (50..60)]);

        let values = a.iter_ranges().collect::<Vec<_>>();
        let expected_values = vec![10..20, 30..40, 50..60];

        assert_eq!(values, expected_values);

        let reversed_values = a.iter_ranges().rev().collect::<Vec<_>>();
        let expected_reversed_values = vec![50..60, 30..40, 10..20];

        assert_eq!(reversed_values, expected_reversed_values);

        let mut iter = a.iter_ranges();
        assert_eq!(iter.len(), 3);
        _ = iter.next();
        assert_eq!(iter.len(), 2);
    }
}
