mod difference;
mod index;
mod intersection;
mod subset;
mod union;

pub use difference::Difference;
pub use index::IndexRanges;
pub use intersection::Intersection;
pub use subset::Subset;
pub use union::Union;

use std::ops::{Add, BitXor, BitXorAssign, Range, Sub};

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
/// assert!(a.is_subset(&(0..30)));
/// assert!(a.is_disjoint(&(0..10)));
/// assert_eq!(a.clone(), RangeSet::from(a));
/// ```
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
    /// The ranges *MUST* be sorted, non-adjacent, non-intersecting, and non-empty.
    ranges: Vec<Range<T>>,
}

impl<T: Copy + Ord> From<Vec<Range<T>>> for RangeSet<T> {
    fn from(ranges: Vec<Range<T>>) -> Self {
        Self::new(&ranges)
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
    /// Returns a new `RangeSet` from the given ranges.
    ///
    /// The `RangeSet` is constructed by computing the union of the given ranges.
    pub fn new(ranges: &[Range<T>]) -> Self
    where
        Self: Union<Range<T>, Output = Self>,
    {
        let mut set = Self::default();

        for range in ranges {
            set = set.union(range);
        }

        set
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

    /// Returns the end of right-most range in the set, or `None` if the set is empty.
    ///
    /// # Note
    ///
    /// This is the *non-inclusive* bound of the right-most range. See `RangeSet::max` for the
    /// maximum value in the set.
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
    /// Returns a new set containing all the existing elements `>= at`. After the call,
    /// the original set will be left containing the elements `< at`.
    ///
    /// # Panics
    ///
    /// Panics if `at` is not in the set.
    pub fn split_off(&mut self, at: &T) -> Self {
        // Find the index of the range containing `at`
        let idx = self
            .ranges
            .iter()
            .position(|range| range.contains(at))
            .expect("`at` is in the set");

        // Split off the range containing `at` and all the ranges to the right.
        let mut split_ranges = self.ranges.split_off(idx);

        // If the first range starts before `at` we have to push those values back
        // into the existing set and truncate.
        if *at > split_ranges[0].start {
            self.ranges.push(Range {
                start: split_ranges[0].start,
                end: *at,
            });
            split_ranges[0].start = *at;
        }

        Self {
            ranges: split_ranges,
        }
    }
}

impl<T: Copy + Ord + Sub<Output = T>> RangeSet<T> {
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

impl<T: Copy + Ord + Add<Output = T>> RangeSet<T> {
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

    /// Returns `true` if the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Copy + Ord> TryFrom<RangeSet<T>> for Range<T> {
    type Error = RangeSet<T>;

    /// Attempts to convert a `RangeSet` into a single `Range`, returning the set if it
    /// does not contain exactly one range.
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

impl<T: Copy + Ord> ToRangeSet<T> for Range<T> {
    fn to_range_set(&self) -> RangeSet<T> {
        RangeSet::from(self.clone())
    }
}

pub trait Disjoint<Rhs> {
    /// Returns `true` if the range is disjoint with `other`.
    #[must_use]
    fn is_disjoint(&self, other: &Rhs) -> bool;
}

pub trait Contains<Rhs> {
    /// Returns `true` if `self` contains `other`.
    #[must_use]
    fn contains(&self, other: &Rhs) -> bool;
}

/// A type which successor and predecessor operations can be performed on.
///
/// Similar to `std::iter::Step`, but not nightly-only.
pub trait Step: Sized {
    /// Steps forwards by `count` elements.
    fn forward(start: Self, count: usize) -> Option<Self>;

    /// Steps backwards by `count` elements.
    fn backward(start: Self, count: usize) -> Option<Self>;
}

macro_rules! impl_step {
    ($($ty:ty),+) => {
        $(
            impl Step for $ty {
                fn forward(start: Self, count: usize) -> Option<Self> {
                    start.checked_add(count as Self)
                }

                fn backward(start: Self, count: usize) -> Option<Self> {
                    start.checked_sub(count as Self)
                }
            }
        )*
    };
}

impl_step!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);

impl<T: Copy + Ord> Disjoint<Range<T>> for Range<T> {
    fn is_disjoint(&self, other: &Range<T>) -> bool {
        self.start >= other.end || self.end <= other.start
    }
}

impl<T: Copy + Ord> Disjoint<RangeSet<T>> for Range<T> {
    fn is_disjoint(&self, other: &RangeSet<T>) -> bool {
        other.ranges.iter().all(|range| self.is_disjoint(range))
    }
}

impl<T: Copy + Ord> Disjoint<RangeSet<T>> for RangeSet<T> {
    fn is_disjoint(&self, other: &RangeSet<T>) -> bool {
        self.ranges.iter().all(|range| range.is_disjoint(other))
    }
}

impl<T: Copy + Ord> Disjoint<Range<T>> for RangeSet<T> {
    fn is_disjoint(&self, other: &Range<T>) -> bool {
        other.is_disjoint(self)
    }
}

impl<T: Copy + Ord> BitXor<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: RangeSet<T>) -> Self::Output {
        let intersection = self.intersection(&rhs);
        self -= intersection;
        self
    }
}

impl<T: Copy + Ord> BitXor<&RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: &RangeSet<T>) -> Self::Output {
        let intersection = self.intersection(rhs);
        self -= intersection;
        self
    }
}

impl<T: Copy + Ord> BitXorAssign<RangeSet<T>> for RangeSet<T> {
    fn bitxor_assign(&mut self, rhs: RangeSet<T>) {
        let intersection = self.intersection(&rhs);
        *self -= intersection;
    }
}

impl<T: Copy + Ord> BitXorAssign<&RangeSet<T>> for RangeSet<T> {
    fn bitxor_assign(&mut self, rhs: &RangeSet<T>) {
        let intersection = self.intersection(rhs);
        *self -= intersection;
    }
}

/// Asserts that the ranges of the given set are sorted, non-adjacent, non-intersecting, and non-empty.
#[cfg(test)]
pub fn assert_invariants<T: Copy + Ord>(set: &RangeSet<T>) {
    assert!(set.ranges.windows(2).all(|w| w[0].start < w[1].start
        && w[0].end < w[1].start
        && w[0].start < w[0].end
        && w[1].start < w[1].end));
}

#[cfg(test)]
#[allow(clippy::all)]
mod tests {
    use super::*;
    use rstest::*;

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

    #[rstest]
    #[case(RangeSet::from([(0..1)]), 0)]
    #[case(RangeSet::from([(0..5)]), 1)]
    #[case(RangeSet::from([(0..5), (6..10)]), 4)]
    #[case(RangeSet::from([(0..5), (6..10)]), 6)]
    #[case(RangeSet::from([(0..5), (6..10)]), 9)]
    fn test_range_set_split_off(#[case] set: RangeSet<usize>, #[case] at: usize) {
        let mut a = set.clone();
        let b = a.split_off(&at);

        assert!(a
            .ranges
            .last()
            .map(|range| !range.is_empty())
            .unwrap_or(true));
        assert!(b
            .ranges
            .first()
            .map(|range| !range.is_empty())
            .unwrap_or(true));
        assert_eq!(a.len() + b.len(), set.len());
        assert!(a.iter().chain(b.iter()).eq(set.iter()));
    }

    #[test]
    #[should_panic = "`at` is in the set"]
    fn test_range_set_split_off_panic_not_in_set() {
        RangeSet::from([0..1]).split_off(&1);
    }

    #[test]
    fn test_range_set_shift_left() {
        let mut a = RangeSet::from([(1..5), (6..10)]);
        a.shift_left(&1);

        assert_eq!(a, RangeSet::from([(0..4), (5..9)]));
    }

    #[test]
    fn test_range_set_shift_right() {
        let mut a = RangeSet::from([(0..4), (5..9)]);
        a.shift_right(&1);

        assert_eq!(a, RangeSet::from([(1..5), (6..10)]));
    }

    #[test]
    fn test_range_set_max() {
        assert!(RangeSet::<u8>::default().max().is_none());
        assert_eq!(RangeSet::from([0..1]).max(), Some(0));
        assert_eq!(RangeSet::from([0..2]).max(), Some(1));
        assert_eq!(RangeSet::from([(0..5), (6..10)]).max(), Some(9));
    }
}
