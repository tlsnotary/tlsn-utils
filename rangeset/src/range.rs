//! Range-specific types.

use core::ops::Range;

use crate::{
    iter::{IntoRangeIterator, RangeIterator},
    ops::{Difference, Disjoint, Intersection, Subset, Union},
};

/// Iterator over the difference of two ranges.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct RangeDiffIter<T> {
    this: Option<Range<T>>,
    other: Range<T>,
}

impl<T> RangeDiffIter<T>
where
    T: Copy + Ord,
{
    #[inline]
    pub(crate) fn new(this: Range<T>, other: Range<T>) -> Self {
        Self {
            this: (!this.is_empty()).then_some(this),
            other: if other.start > other.end {
                other.start..other.start
            } else {
                other
            },
        }
    }
}

impl<T: Copy + Ord> Iterator for RangeDiffIter<T> {
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let this = self.this.take()?;

        if this.is_disjoint(&self.other) {
            // If disjoint, return the entire range.
            Some(this)
        } else if this.start < self.other.start {
            // This range precedes the other range.
            let left = this.start..this.end.min(self.other.start);

            // Store the remainder.
            let rem_start = this.start.max(self.other.end);
            if rem_start < this.end {
                self.this = Some(rem_start..this.end);
                // Empty the other range to speed up the next iteration.
                self.other.end = self.other.start;
            }

            Some(left)
        } else {
            // The other range precedes this range.
            let right_start = this.start.max(self.other.end);
            if right_start < this.end {
                Some(right_start..this.end)
            } else {
                None
            }
        }
    }
}

impl<T: Copy + Ord> RangeIterator<T> for RangeDiffIter<T> {
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        let range = self.next()?;
        if &range.end >= n {
            Some(range)
        } else if self.this.is_some() {
            // Try again if there is a remainder.
            self.this
                .take()
                .and_then(|range| (&range.end >= n).then_some(range))
        } else {
            None
        }
    }
}

impl<T: Copy + Ord> IntoRangeIterator<T> for RangeDiffIter<T> {
    type IntoIter = RangeDiffIter<T>;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

/// Iterator over the union of two ranges.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct RangeUnionIter<T> {
    a: Option<Range<T>>,
    b: Option<Range<T>>,
}

impl<T: Copy + Ord> RangeUnionIter<T> {
    #[inline]
    pub(crate) fn new(a: Range<T>, b: Range<T>) -> Self {
        Self {
            a: (a.start < a.end).then_some(a),
            b: (b.start < b.end).then_some(b),
        }
    }
}

impl<T: Copy + Ord> Iterator for RangeUnionIter<T> {
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match (self.a.take(), self.b.take()) {
            (Some(a), Some(b)) => {
                let (left, right) = if a.start < b.start { (a, b) } else { (b, a) };
                if left.end < right.start {
                    // Left is entirely before right.
                    self.a = Some(right);
                    Some(left)
                } else {
                    // Merge them.
                    Some(left.start..left.end.max(right.end))
                }
            }
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        }
    }
}

impl<T: Copy + Ord> RangeIterator<T> for RangeUnionIter<T> {
    #[inline]
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        let range = self.next()?;
        if &range.end >= n {
            Some(range)
        } else if self.a.is_some() {
            // Try again if there is a remainder.
            self.a
                .take()
                .and_then(|range| (&range.end >= n).then_some(range))
        } else {
            None
        }
    }
}

impl<T: Copy + Ord> IntoRangeIterator<T> for RangeUnionIter<T> {
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

/// Iterator over the intersection of two ranges.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct RangeIntersectionIter<T> {
    intersection: Option<Range<T>>,
}

impl<T: Copy + Ord> RangeIntersectionIter<T> {
    #[inline]
    pub(crate) fn new(a: &Range<T>, b: &Range<T>) -> Self {
        let start = a.start.max(b.start);
        let end = a.end.min(b.end);

        Self {
            intersection: (start < end).then_some(start..end),
        }
    }
}

impl<T: Copy + Ord> Iterator for RangeIntersectionIter<T> {
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.intersection.take()
    }
}

impl<T: Copy + Ord> RangeIterator<T> for RangeIntersectionIter<T> {
    #[inline]
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        self.next()
            .and_then(|range| (&range.end >= n).then_some(range))
    }
}

impl<T: Copy + Ord> IntoRangeIterator<T> for RangeIntersectionIter<T> {
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

impl<T: Copy + Ord> Intersection<Range<T>> for Range<T> {
    type Output<'a>
        = RangeIntersectionIter<T>
    where
        T: 'a;

    fn intersection<'a>(&'a self, other: &'a Range<T>) -> Self::Output<'a> {
        RangeIntersectionIter::new(self, other)
    }
}

/// Iterator which yields a single range.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Once<T> {
    range: Option<Range<T>>,
}

impl<T> Once<T>
where
    T: Ord,
{
    /// Creates a new iterator which yields a single range.
    #[inline]
    pub fn new(range: Range<T>) -> Self {
        Self {
            range: (range.start < range.end).then_some(range),
        }
    }
}

impl<T> Iterator for Once<T> {
    type Item = Range<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.range.take()
    }
}

impl<T> RangeIterator<T> for Once<T> {
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        self.range
            .take()
            .and_then(|range| (&range.end >= n).then_some(range))
    }
}

impl<T> IntoRangeIterator<T> for Once<T> {
    type IntoIter = Once<T>;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

impl<T> IntoRangeIterator<T> for Range<T>
where
    T: Ord,
{
    type IntoIter = Once<T>;

    fn into_range_iter(self) -> Self::IntoIter {
        Once::new(self)
    }
}

impl<'a, T> IntoRangeIterator<T> for &'a Range<T>
where
    T: Copy + Ord,
{
    type IntoIter = Once<T>;

    fn into_range_iter(self) -> Self::IntoIter {
        Once::new(self.start..self.end)
    }
}

impl<T: Copy + Ord> Union<Range<T>> for Range<T> {
    type Output<'a>
        = RangeUnionIter<T>
    where
        T: 'a;

    fn union<'a>(&'a self, other: &'a Range<T>) -> Self::Output<'a> {
        RangeUnionIter::new(self.clone(), other.clone())
    }
}

impl<T: Copy + Ord> Difference<Range<T>> for Range<T> {
    type Output<'a>
        = RangeDiffIter<T>
    where
        T: 'a;

    fn difference<'a>(&'a self, other: &'a Range<T>) -> Self::Output<'a> {
        RangeDiffIter::new(self.clone(), other.clone())
    }
}

impl<T: Copy + Ord> Disjoint<Range<T>> for Range<T> {
    #[inline]
    fn is_disjoint(&self, other: &Range<T>) -> bool {
        self.start >= other.end || self.end <= other.start
    }
}

impl<T: Copy + Ord> Subset<Range<T>> for Range<T> {
    fn is_subset(&self, other: &Range<T>) -> bool {
        self.start >= other.start && self.end <= other.end
    }
}

#[cfg(feature = "alloc")]
mod alloc {
    use core::ops::BitAnd;

    use super::*;
    use crate::{
        iter::{DifferenceIter, FromRangeIterator, IntersectionIter, UnionIter},
        set::{RangeIter, RangeSet, ToRangeSet},
    };

    impl<T: Copy + Ord> ToRangeSet<T> for Range<T> {
        fn to_range_set(&self) -> RangeSet<T> {
            RangeSet::from_range_iter(self.clone())
        }
    }

    impl<T: Copy + Ord> Union<RangeSet<T>> for Range<T> {
        type Output<'a>
            = UnionIter<T, Once<T>, RangeIter<'a, T>>
        where
            T: 'a;

        fn union<'a>(&'a self, other: &'a RangeSet<T>) -> Self::Output<'a> {
            Once::new(self.clone()).union(other.iter_ranges())
        }
    }

    impl<T: Copy + Ord> Difference<RangeSet<T>> for Range<T> {
        type Output<'a>
            = DifferenceIter<T, Once<T>, RangeIter<'a, T>>
        where
            T: 'a;

        fn difference<'a>(&'a self, other: &'a RangeSet<T>) -> Self::Output<'a> {
            Once::new(self.clone()).difference(other.iter_ranges())
        }
    }

    impl<T: Copy + Ord> Intersection<RangeSet<T>> for Range<T> {
        type Output<'a>
            = IntersectionIter<T, Once<T>, RangeIter<'a, T>>
        where
            T: 'a;

        fn intersection<'a>(&'a self, other: &'a RangeSet<T>) -> Self::Output<'a> {
            Once::new(self.start..self.end).intersection(other)
        }
    }

    impl<T: Copy + Ord> Disjoint<RangeSet<T>> for Range<T> {
        fn is_disjoint(&self, other: &RangeSet<T>) -> bool {
            other.iter_ranges().all(|range| self.is_disjoint(&range))
        }
    }

    impl<T: Copy + Ord> Subset<RangeSet<T>> for Range<T> {
        fn is_subset(&self, other: &RangeSet<T>) -> bool {
            if self.is_empty() {
                // empty range is subset of any set
                return true;
            } else if other.is_empty() {
                // non-empty range is not subset of empty set
                return false;
            }

            // Boundary short circuit.
            if self.start < other.min().unwrap() || self.end > other.end().unwrap() {
                // Check if self's start & end are contained within (or same as) other's.
                return false;
            }

            for other in other.iter_ranges() {
                if self.start >= other.end {
                    // self is rightward of other, proceed to next other
                    continue;
                } else {
                    return self.is_subset(&other);
                }
            }

            false
        }
    }

    impl<T: Copy + Ord> BitAnd<RangeSet<T>> for Range<T> {
        type Output = RangeSet<T>;

        fn bitand(self, other: RangeSet<T>) -> Self::Output {
            self.intersection(&other).into_set()
        }
    }

    impl<T: Copy + Ord> BitAnd<&RangeSet<T>> for Range<T> {
        type Output = RangeSet<T>;

        fn bitand(self, other: &RangeSet<T>) -> Self::Output {
            self.intersection(other).into_set()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{TEST_DOMAIN_SIZE, Universe, assert_pairwise_ranges};

    #[test]
    fn test_once() {
        for set in Universe::new(TEST_DOMAIN_SIZE).iter_ranges() {
            let range = set.into_range().unwrap();
            let mut iter = Once::new(range.clone());
            if !range.is_empty() {
                assert_eq!(iter.next(), Some(range));
            }
            assert_eq!(iter.next(), None);
        }
    }

    #[test]
    fn test_range_union_iter() {
        assert_pairwise_ranges(
            TEST_DOMAIN_SIZE,
            |a, b| a | b,
            |a, b| RangeUnionIter::new(a, b),
        );
    }

    #[test]
    fn test_range_diff_iter() {
        assert_pairwise_ranges(
            TEST_DOMAIN_SIZE,
            |a, b| a - b,
            |a, b| RangeDiffIter::new(a, b),
        );
    }

    #[test]
    fn test_range_intersection_iter() {
        assert_pairwise_ranges(
            TEST_DOMAIN_SIZE,
            |a, b| a & b,
            |a, b| RangeIntersectionIter::new(&a, &b),
        );
    }

    #[test]
    fn test_range_disjoint() {
        for a in Universe::new(TEST_DOMAIN_SIZE).iter_ranges() {
            for b in Universe::new(TEST_DOMAIN_SIZE).iter_ranges() {
                let expected = (a & b).is_empty();
                let a = a.into_range().unwrap();
                let b = b.into_range().unwrap();
                assert_eq!(a.is_disjoint(&b), expected, "{:?} {:?}", a, b);
            }
        }
    }
}
