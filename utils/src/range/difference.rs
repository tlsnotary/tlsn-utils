use std::ops::Range;

use crate::range::{
    RangeDifference, RangeDisjoint, RangeSet, RangeSubset, RangeSuperset, RangeUnion,
};

impl<T: Copy + Ord> RangeDifference<Range<T>> for Range<T> {
    type Output = RangeSet<T>;

    fn difference(&self, other: &Range<T>) -> Self::Output {
        if self.is_empty() {
            return RangeSet::default();
        } else if other.is_empty() {
            return RangeSet::from(self.clone());
        }

        // If other is a superset of self, return an empty set.
        if other.is_superset(self) {
            return RangeSet::default();
        }

        // If they are disjoint, return self.
        if self.is_disjoint(other) {
            return RangeSet::from(self.clone());
        }

        let mut set = RangeSet::default();

        if self.start < other.start {
            set.ranges.push(self.start..other.start);
        }

        if self.end > other.end {
            set.ranges.push(other.end..self.end);
        }

        set
    }
}

impl<T: Copy + Ord> RangeDifference<RangeSet<T>> for Range<T>
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

impl<T: Copy + Ord> RangeDifference<Range<T>> for RangeSet<T> {
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

        RangeSet { ranges }
    }
}

impl<T: Copy + Ord> RangeDifference<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn difference(&self, other: &RangeSet<T>) -> Self::Output {
        let mut set = RangeSet::default();
        for range in &self.ranges {
            set = set.union(&range.difference(other));
        }
        set
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

        // subset
        assert_eq!(
            a.difference(&(14..16)),
            RangeSet::from([(10..14), (16..20)])
        );

        // equal
        assert_eq!(a.difference(&(10..20)), RangeSet::default());
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
