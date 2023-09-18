use std::ops::Range;

use crate::range::{RangeDisjoint, RangeSet, RangeSuperset, RangeUnion};

impl<T: Copy + Ord> RangeUnion<Range<T>> for Range<T> {
    type Output = RangeSet<T>;

    fn union(&self, other: &Range<T>) -> Self::Output {
        // If the two are equal, or other is a subset, return self.
        if self == other || self.is_superset(other) {
            return RangeSet::from(self.clone());
        }

        // If other is a superset, return other.
        if other.is_superset(self) {
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

impl<T: Copy + Ord> RangeUnion<RangeSet<T>> for Range<T> {
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

        RangeSet { ranges }
    }
}

impl<T: Copy + Ord> RangeUnion<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn union(&self, other: &Range<T>) -> Self::Output {
        other.union(self)
    }
}

impl<T: Copy + Ord> RangeUnion<RangeSet<T>> for RangeSet<T> {
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
}
