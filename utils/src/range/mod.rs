mod difference;
mod union;

use std::ops::Range;

/// A set of ranges.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RangeSet<T> {
    /// The ranges of the set.
    ///
    /// The ranges *MUST* be sorted, non-intersecting, and non-empty.
    ranges: Vec<Range<T>>,
}

impl<T> Default for RangeSet<T> {
    fn default() -> Self {
        Self {
            ranges: Default::default(),
        }
    }
}

impl<T: Clone + Copy + Ord> RangeSet<T> {
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

    /// Returns `true` if the set contains the given value.
    pub fn contains(&self, value: &T) -> bool {
        self.ranges.iter().any(|range| range.contains(value))
    }
}

impl<T> RangeSet<T>
where
    Range<T>: ExactSizeIterator<Item = T>,
{
    /// Returns the number of values in the set.
    pub fn len(&self) -> usize {
        self.ranges.iter().map(|range| range.len()).sum()
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Ord> From<Range<T>> for RangeSet<T> {
    fn from(range: Range<T>) -> Self {
        if range.is_empty() {
            return Self::default();
        }

        Self {
            ranges: Vec::from([range]),
        }
    }
}

/// An iterator over the values in a `RangeSet`.
pub struct RangeSetIter<'a, T> {
    iter: std::slice::Iter<'a, Range<T>>,
    current: Option<Range<T>>,
}

impl<'a, T> Iterator for RangeSetIter<'a, T>
where
    T: Copy,
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

pub trait RangeIntersects<Rhs> {
    /// Returns `true` if the range intersects with `other`.
    fn intersects(&self, other: &Rhs) -> bool;
}

pub trait RangeDisjoint<Rhs> {
    /// Returns `true` if the range is disjoint with `other`.
    fn is_disjoint(&self, other: &Rhs) -> bool;
}

pub trait RangeSuperset<Rhs> {
    /// Returns `true` if `self` is a superset of `other`.
    fn is_superset(&self, other: &Rhs) -> bool;
}

pub trait RangeSubset<Rhs> {
    /// Returns `true` if `self` is a subset of `other`.
    fn is_subset(&self, other: &Rhs) -> bool;
}

pub trait RangeDifference<Rhs> {
    type Output;

    /// Returns the set difference of `self` and `other`.
    fn difference(&self, other: &Rhs) -> Self::Output;
}

pub trait RangeUnion<Rhs> {
    type Output;

    /// Returns the set union of `self` and `other`.
    fn union(&self, other: &Rhs) -> Self::Output;
}

impl<T: Ord> RangeIntersects<Range<T>> for Range<T> {
    fn intersects(&self, other: &Range<T>) -> bool {
        self.start < other.end && self.end > other.start
    }
}

impl<T: Ord> RangeIntersects<RangeSet<T>> for Range<T> {
    fn intersects(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().any(|range| self.intersects(range))
    }
}

impl<T: Ord> RangeDisjoint<Range<T>> for Range<T> {
    fn is_disjoint(&self, other: &Range<T>) -> bool {
        self.start >= other.end || self.end <= other.start
    }
}

impl<T: Ord> RangeDisjoint<RangeSet<T>> for Range<T> {
    fn is_disjoint(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().all(|range| self.is_disjoint(range))
    }
}

impl<T: Ord> RangeSuperset<Range<T>> for Range<T> {
    fn is_superset(&self, other: &Range<T>) -> bool {
        self.start <= other.start && self.end >= other.end
    }
}

impl<T: Ord> RangeSuperset<RangeSet<T>> for Range<T> {
    fn is_superset(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().all(|range| self.is_superset(range))
    }
}

impl<T: Ord> RangeSubset<Range<T>> for Range<T> {
    fn is_subset(&self, other: &Range<T>) -> bool {
        self.start >= other.start && self.end <= other.end
    }
}

impl<T: Ord> RangeSubset<RangeSet<T>> for Range<T> {
    fn is_subset(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().all(|range| self.is_subset(range))
    }
}

#[cfg(test)]
#[allow(clippy::all)]
mod tests {
    use super::*;

    impl<const N: usize, T: Default + Copy + Ord> From<[Range<T>; N]> for RangeSet<T> {
        fn from(value: [Range<T>; N]) -> Self {
            RangeSet::new(&value)
        }
    }

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
    fn test_range_subset() {
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
    fn test_range_superset() {
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
}