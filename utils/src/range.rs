use std::ops::Range;

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

impl<T: Clone + Copy + Ord> RangeDifference<Range<T>> for Range<T> {
    type Output = RangeSet<T>;

    fn difference(&self, other: &Range<T>) -> Self::Output {
        if self.is_empty() {
            return RangeSet::default();
        } else if other.is_empty() {
            return RangeSet::from(self.clone());
        }

        // If the two are equal, or if other is a superset of self, return an empty set.
        if self == other || (other.start <= self.start && other.end >= self.end) {
            return RangeSet::default();
        }

        // If they are disjoint, return self.
        if self.is_disjoint(other) {
            return RangeSet::from(self.clone());
        }

        let mut set = RangeSet::default();

        if self.start < other.start && self.end > other.end {
            set.ranges.push(self.start..other.start);
            set.ranges.push(other.end..self.end);
        } else if self.start < other.start {
            set.ranges.push(self.start..other.start);
        } else {
            set.ranges.push(other.end..self.end);
        }

        set
    }
}

impl<T: Clone + Copy + Ord> RangeDifference<RangeSet<T>> for Range<T>
where
    RangeSet<T>: RangeDifference<Range<T>, Output = RangeSet<T>>,
{
    type Output = RangeSet<T>;

    fn difference(&self, other: &RangeSet<T>) -> Self::Output {
        if self.is_empty() {
            return RangeSet::default();
        }

        let mut diff = RangeSet::from(self.clone());

        for range in &other.ranges {
            diff = diff.difference(range);
        }

        diff
    }
}

impl<T: Clone + Copy + Ord> RangeDifference<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn difference(&self, other: &Range<T>) -> Self::Output {
        if other.is_empty() {
            return self.clone();
        }

        let mut i = 0;
        let mut ranges = self.ranges.clone();
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
            else if other.is_superset(&ranges[i]) {
                ranges.remove(i);
                continue;
            }
            // If other is a subset of the current range
            else if other.is_subset(&ranges[i]) {
                let mut new_range = ranges[i].clone();
                new_range.start = other.end;
                ranges[i].end = other.start;
                ranges.insert(i + 1, new_range);
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

        RangeSet { ranges }
    }
}

impl<T: Clone + Copy + Ord> RangeDifference<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn difference(&self, other: &RangeSet<T>) -> Self::Output {
        let mut set = RangeSet::default();
        for range in &self.ranges {
            set = set.union(&range.difference(other));
        }
        set
    }
}

impl<T: Clone + Copy + Ord> RangeUnion<Range<T>> for Range<T> {
    type Output = RangeSet<T>;

    fn union(&self, other: &Range<T>) -> Self::Output {
        // If the two are equal, or other is a subset, return self.
        if self == other || self.is_superset(other) {
            return RangeSet::from(self.clone());
        }

        // If other is a superset, return other.
        if other.is_subset(self) {
            return RangeSet::from(other.clone());
        }

        // If they are disjoint, return a set containing both, making sure
        // the ranges are in order.
        if self.is_disjoint(other) {
            if self.start < other.start {
                return RangeSet::new(&[self.clone(), other.clone()]);
            } else {
                return RangeSet::new(&[other.clone(), self.clone()]);
            }
        }

        // Otherwise, return a set containing the union of the two.
        let start = self.start.min(other.start);
        let end = self.end.max(other.end);

        RangeSet::from(start..end)
    }
}

impl<T: Clone + Copy + Ord> RangeUnion<RangeSet<T>> for Range<T> {
    type Output = RangeSet<T>;

    fn union(&self, other: &RangeSet<T>) -> Self::Output {
        if self.is_empty() {
            return other.clone();
        }

        let mut ranges = other.ranges.clone();

        let mut i = 0;
        let mut new_range = self.clone();
        while i < ranges.len() {
            // If the new_range comes before the current range without overlapping
            if new_range.end < ranges[i].start {
                ranges.insert(i, new_range);

                return RangeSet { ranges };
            }
            // If the new_range overlaps with the current range
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

        RangeSet { ranges }
    }
}

impl<T: Clone + Copy + Ord> RangeUnion<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn union(&self, other: &Range<T>) -> Self::Output {
        other.union(self)
    }
}

impl<T: Clone + Copy + Ord> RangeUnion<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn union(&self, other: &RangeSet<T>) -> Self::Output {
        let mut union = self.clone();
        for range in &other.ranges {
            union = union.union(range);
        }
        union
    }
}

#[cfg(test)]
#[allow(clippy::all)]
mod tests {
    use super::*;

    use itertools::iproduct;

    // Yields every possible non-empty range bounded by `max` and `offset`.
    #[derive(Debug, Clone, Copy)]
    struct EveryNonEmptyRange {
        max: usize,
        offset: usize,
        start: usize,
        end: usize,
    }

    impl EveryNonEmptyRange {
        fn new(max: usize, offset: usize) -> Self {
            Self {
                max,
                offset,
                start: 0,
                end: 0,
            }
        }
    }

    impl Iterator for EveryNonEmptyRange {
        type Item = Range<usize>;

        fn next(&mut self) -> Option<Self::Item> {
            if self.start >= self.max {
                return None;
            }

            if self.end >= self.max {
                self.start += 1;
                self.end = self.start;
            }

            let range = self.start + self.offset..self.end + self.offset;
            self.end += 1;

            Some(range)
        }
    }

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
    fn test_range_difference() {
        let a = 10..20;

        // rightward subset
        // [-----)
        //     [-----)
        assert_eq!(a.difference(&(15..25)), RangeSet::from([(10..15)]));

        // rightward aligned
        // [-----)
        //       [-----)
        assert_eq!(a.difference(&(20..25)), RangeSet::from([(10..20)]));

        // rightward aligned inclusive
        // [-----)
        //      [-----)
        assert_eq!(a.difference(&(19..25)), RangeSet::from([(10..19)]));

        // rightward disjoint
        // [-----)
        //           [-----)
        assert_eq!(a.difference(&(25..30)), RangeSet::from([(10..20)]));

        // leftward subset
        //    [-----)
        // [-----)
        assert_eq!(a.difference(&(5..15)), RangeSet::from([(15..20)]));

        // leftward aligned
        //       [-----)
        // [-----)
        assert_eq!(a.difference(&(5..10)), RangeSet::from([(10..20)]));

        // leftward aligned inclusive
        //      [-----)
        // [-----)
        assert_eq!(a.difference(&(5..11)), RangeSet::from([(11..20)]));

        // leftward disjoint
        //           [-----)
        // [-----)
        assert_eq!(a.difference(&(0..5)), RangeSet::from([(10..20)]));

        // superset
        assert_eq!(a.difference(&(5..25)), RangeSet::default());

        // equal
        assert_eq!(a.difference(&(10..20)), RangeSet::default());
    }

    #[test]
    fn test_range_union() {
        let a = 10..20;

        // rightward subset
        // [-----)
        //     [-----)
        assert_eq!(a.union(&(15..25)), RangeSet::from([(10..25)]));

        // leftward subset
        //    [-----)
        // [-----)
        assert_eq!(a.union(&(5..15)), RangeSet::from([(5..20)]));

        // subset
        // [-----)
        //   [--)
        assert_eq!(a.union(&(12..15)), RangeSet::from([(10..20)]));

        // superset
        //   [-----)
        // [---------)
        assert_eq!(a.union(&(5..25)), RangeSet::from([(5..25)]));

        // rightward disjoint
        // [-----)
        //           [-----)
        assert_eq!(a.union(&(25..30)), RangeSet::from([(10..20), (25..30)]));

        // leftward disjoint
        //           [-----)
        // [-----)
        assert_eq!(a.union(&(0..5)), RangeSet::from([(0..5), (10..20)]));

        // equal
        assert_eq!(a.union(&(10..20)), RangeSet::from([(10..20)]));
    }

    #[test]
    fn test_range_set_union() {
        let a = RangeSet::from([(10..20), (30..40), (50..60)]);

        // rightward intersect
        // [-----) [-----) [-----)
        //     [-----)
        assert_eq!(
            a.union(&(35..45)),
            RangeSet::from([(10..20), (30..45), (50..60)])
        );

        // leftward intersect
        //     [-----) [-----) [-----)
        // [-----)
        assert_eq!(
            a.union(&(5..15)),
            RangeSet::from([(5..20), (30..40), (50..60)])
        );

        // subset
        // [-----) [-----) [-----)
        //   [--)
        assert_eq!(
            a.union(&(12..15)),
            RangeSet::from([(10..20), (30..40), (50..60)])
        );

        // superset of 1
        //   [-----) [-----) [-----)
        // [---------)
        assert_eq!(
            a.union(&(5..25)),
            RangeSet::from([(5..25), (30..40), (50..60)])
        );

        // superset of 2
        //   [-----) [-----) [-----)
        // [----------------)
        assert_eq!(a.union(&(5..45)), RangeSet::from([(5..45), (50..60)]));

        // superset
        //   [-----) [-----) [-----)
        // [------------------------)
        assert_eq!(a.union(&(5..65)), RangeSet::from([(5..65)]));

        // leftwards disjoint
        //           [-----) [-----) [-----)
        // [-----)
        assert_eq!(
            a.union(&(0..5)),
            RangeSet::from([(0..5), (10..20), (30..40), (50..60)])
        );

        // rightwards disjoint
        // [-----) [-----) [-----)
        //                           [-----)
        assert_eq!(
            a.union(&(65..70)),
            RangeSet::from([(10..20), (30..40), (50..60), (65..70)])
        );

        // disjoint
        // [-----)        [-----) [-----)
        //         [-----)
        assert_eq!(
            a.union(&(25..28)),
            RangeSet::from([(10..20), (25..28), (30..40), (50..60)])
        );

        // empty
        assert_eq!(a.union(&(0..0)), a);
        assert_eq!((0..0).union(&a), a);
    }

    #[test]
    fn test_union_set() {
        let a = RangeSet::from([(10..20), (30..40), (50..60)]);
        let b = RangeSet::from([(15..25), (35..45), (55..65)]);

        assert_eq!(a.union(&b), RangeSet::from([(10..25), (30..45), (50..65)]));

        // disjoint
        let b = RangeSet::from([(22..23), (41..48)]);
        assert_eq!(
            a.union(&b),
            RangeSet::from([(10..20), (22..23), (30..40), (41..48), (50..60)])
        );

        // superset
        let b = RangeSet::from([(5..65)]);
        assert_eq!(a.union(&b), b);

        // subset
        let b = RangeSet::from([(12..18)]);
        assert_eq!(a.union(&b), a);
    }

    #[test]
    fn test_range_diff_set() {
        let a = 10..20;

        // rightward subset
        // [-----)
        //     [-----)
        let b = RangeSet::from([(15..25)]);
        assert_eq!(a.difference(&b), RangeSet::from([(10..15)]));

        // leftward subset
        //    [-----)
        // [-----)
        let b = RangeSet::from([(5..15)]);
        assert_eq!(a.difference(&b), RangeSet::from([(15..20)]));

        // subset
        // [-----)
        //   [--)
        let b = RangeSet::from([(12..15)]);
        assert_eq!(a.difference(&b), RangeSet::from([(10..12), (15..20)]));

        // 2 subsets
        // [-------)
        //  [-)[-)
        let b = RangeSet::from([(11..13), (15..18)]);
        assert_eq!(
            a.difference(&b),
            RangeSet::from([(10..11), (13..15), (18..20)])
        );

        // 3 subsets
        // [---------)
        //  [-)[-)[-)
        let b = RangeSet::from([(11..12), (13..15), (17..19)]);
        assert_eq!(
            a.difference(&b),
            RangeSet::from([(10..11), (12..13), (15..17), (19..20)])
        );
    }

    #[test]
    fn test_set_diff_range() {
        let a = RangeSet::from([(10..20), (30..40), (50..60)]);

        // rightward subset
        // [-----) [-----) [-----)
        //     [-----)
        assert_eq!(
            a.difference(&(15..35)),
            RangeSet::from([(10..15), (35..40), (50..60)])
        );

        // leftward subset
        //    [-----) [-----) [-----)
        // [-----)
        assert_eq!(
            a.difference(&(5..15)),
            RangeSet::from([(15..20), (30..40), (50..60)])
        );

        // subset
        // [-----) [-----) [-----)
        //   [--)
        assert_eq!(
            a.difference(&(12..15)),
            RangeSet::from([(10..12), (15..20), (30..40), (50..60)])
        );

        // subset 2
        // [-----) [-----) [-----)
        //          [--)
        assert_eq!(
            a.difference(&(35..38)),
            RangeSet::from([(10..20), (30..35), (38..40), (50..60)])
        );

        // superset of 1
        //   [-----) [-----) [-----)
        // [--------)
        assert_eq!(a.difference(&(5..25)), RangeSet::from([(30..40), (50..60)]));

        // superset of 2
        //   [-----) [-----) [-----)
        // [----------------)
        assert_eq!(a.difference(&(5..45)), RangeSet::from([(50..60)]));

        // superset
        //   [-----) [-----) [-----)
        // [------------------------)
        assert_eq!(a.difference(&(5..65)), RangeSet::default());

        // leftwards disjoint
        //           [-----) [-----) [-----)
        // [-----)
        assert_eq!(a.difference(&(0..5)), a);

        // rightwards disjoint
        // [-----) [-----) [-----)
        //                           [-----)
        assert_eq!(a.difference(&(65..70)), a);

        // disjoint
        // [-----)        [-----) [-----)
        //         [-----)
        assert_eq!(
            a.difference(&(25..28)),
            RangeSet::from([(10..20), (30..40), (50..60)])
        );

        // empty
        assert_eq!(a.difference(&(0..0)), a);
    }

    #[test]
    fn test_range_set_iter() {
        let a = RangeSet::from([(10..20), (30..40), (50..60)]);

        let values = a.iter().collect::<Vec<_>>();
        let expected_values = (10..20).chain(30..40).chain(50..60).collect::<Vec<_>>();
        assert_eq!(values, expected_values);
    }

    // This proves the union operation for 3 sets, up to size 16.
    #[test]
    #[ignore = "expensive"]
    fn test_prove_range_union_range_16x3() {
        fn expected(x: Range<usize>, y: Range<usize>, z: Range<usize>) -> Vec<usize> {
            let mut expected_values = x.chain(y).chain(z).collect::<Vec<_>>();

            expected_values.sort();
            expected_values.dedup();

            expected_values
        }

        for (xs, xe, ys, ye, zs, ze) in iproduct!(0..16, 0..16, 0..16, 0..16, 0..16, 0..16) {
            let set = (xs..xe).union(&(ys..ye)).union(&(zs..ze));

            let actual = set
                .clone()
                .into_inner()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            assert_eq!(
                actual,
                expected(xs..xe, ys..ye, zs..ze),
                "{:?} {:?} {:?} => {:?}",
                xs..xe,
                ys..ye,
                zs..ze,
                set
            );
        }
    }

    #[test]
    #[ignore = "expensive"]
    fn test_prove_set_union_range_8x3_8() {
        fn expected(
            x: Range<usize>,
            y: Range<usize>,
            w: Range<usize>,
            z: Range<usize>,
        ) -> Vec<usize> {
            let mut expected_values = x.chain(y).chain(w).chain(z).collect::<Vec<_>>();

            expected_values.sort();
            expected_values.dedup();

            expected_values
        }

        for (xs, xe, ys, ye, ws, we, zs, ze) in
            iproduct!(0..8, 0..8, 0..8, 0..8, 0..8, 0..8, 0..8, 0..8)
        {
            let set = RangeSet::new(&[(xs..xe), (ys..ye), (ws..we)]).union(&(zs..ze));

            let actual = set
                .clone()
                .into_inner()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            assert_eq!(
                actual,
                expected(xs..xe, ys..ye, ws..we, zs..ze),
                "{:?} {:?} {:?} {:?} => {:?}",
                xs..xe,
                ys..ye,
                ws..we,
                zs..ze,
                set
            );
        }
    }

    #[test]
    #[ignore = "expensive"]
    fn test_prove_set_union_set_8x2_8x2() {
        fn expected(
            x: Range<usize>,
            y: Range<usize>,
            w: Range<usize>,
            z: Range<usize>,
        ) -> Vec<usize> {
            let mut expected_values = x.chain(y).chain(w).chain(z).collect::<Vec<_>>();

            expected_values.sort();
            expected_values.dedup();

            expected_values
        }

        for (xs, xe, ys, ye, ws, we, zs, ze) in
            iproduct!(0..8, 0..8, 0..8, 0..8, 0..8, 0..8, 0..8, 0..8)
        {
            let s1 = RangeSet::new(&[(xs..xe), (ys..ye)]);
            let s2 = RangeSet::new(&[(ws..we), (zs..ze)]);

            let set = s1.union(&s2);

            let actual = set
                .clone()
                .into_inner()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            assert_eq!(
                actual,
                expected(xs..xe, ys..ye, ws..we, zs..ze),
                "{:?} {:?} {:?} {:?} => {:?}",
                xs..xe,
                ys..ye,
                ws..we,
                zs..ze,
                set
            );
        }
    }

    #[test]
    #[ignore = "expensive"]
    fn test_prove_range_diff_range_16_16() {
        fn expected(x: Range<usize>, y: Range<usize>) -> Vec<usize> {
            x.filter(|x| !y.contains(x)).collect::<Vec<_>>()
        }

        for (xs, xe, ys, ye) in iproduct!(0..16, 0..16, 0..16, 0..16) {
            let set = (xs..xe).difference(&(ys..ye));

            let actual = set
                .clone()
                .into_inner()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            assert_eq!(
                actual,
                expected(xs..xe, ys..ye),
                "{:?} {:?} => {:?}",
                xs..xe,
                ys..ye,
                set
            );
        }
    }

    #[test]
    #[ignore = "expensive"]
    fn test_prove_range_diff_set_16_16x2() {
        fn expected(x: Range<usize>, y: Range<usize>, z: Range<usize>) -> Vec<usize> {
            let set = y.union(&z);
            x.filter(|x| !set.contains(x)).collect::<Vec<_>>()
        }

        for (xs, xe, ys, ye, zs, ze) in iproduct!(0..16, 0..16, 0..16, 0..16, 0..16, 0..16) {
            let set = (xs..xe).difference(&RangeSet::new(&[(ys..ye), (zs..ze)]));

            let actual = set
                .clone()
                .into_inner()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            assert_eq!(
                actual,
                expected(xs..xe, ys..ye, zs..ze),
                "{:?} {:?} {:?} => {:?}",
                xs..xe,
                ys..ye,
                zs..ze,
                set
            );
        }
    }

    // Proves every set difference operation up to size 12,
    // with up to 4 non-empty partitions.
    #[test]
    #[ignore = "expensive"]
    fn test_prove_set_diff_set_12x4_12x4() {
        #[allow(clippy::all)]
        fn expected(
            a: Range<usize>,
            b: Range<usize>,
            c: Range<usize>,
            d: Range<usize>,
            e: Range<usize>,
            f: Range<usize>,
            g: Range<usize>,
            h: Range<usize>,
        ) -> Vec<usize> {
            let s2 = e.union(&f).union(&g).union(&h);
            a.union(&b)
                .union(&c)
                .union(&d)
                .iter()
                .filter(|x| !s2.contains(x))
                .collect::<Vec<_>>()
        }

        for (a, b, c, d, e, f, g, h) in iproduct!(
            EveryNonEmptyRange::new(3, 0),
            EveryNonEmptyRange::new(3, 3),
            EveryNonEmptyRange::new(3, 6),
            EveryNonEmptyRange::new(3, 9),
            EveryNonEmptyRange::new(3, 0),
            EveryNonEmptyRange::new(3, 3),
            EveryNonEmptyRange::new(3, 6),
            EveryNonEmptyRange::new(3, 9)
        ) {
            let set = RangeSet::new(&[a.clone(), b.clone(), c.clone(), d.clone()]).difference(
                &RangeSet::new(&[e.clone(), f.clone(), g.clone(), h.clone()]),
            );

            let actual = set
                .clone()
                .into_inner()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            assert_eq!(actual, expected(a, b, c, d, e, f, g, h),);
        }
    }
}
