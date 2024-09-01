use crate::range::{Range, RangeSet, Subset};

impl<T: Copy + Ord> Subset<Range<T>> for Range<T> {
    fn is_subset(&self, other: &Range<T>) -> bool {
        self.start >= other.start && self.end <= other.end
    }
}

impl<T: Copy + Ord> Subset<RangeSet<T>> for Range<T> {
    fn is_subset(&self, other: &RangeSet<T>) -> bool {
        if self.is_empty() {
            // empty range is subset of any set
            return true;
        } else if other.ranges.is_empty() {
            // non-empty range is not subset of empty set
            return false;
        }

        for other in &other.ranges {
            if self.start >= other.end {
                // self is rightward of other, proceed to next other
                continue;
            } else {
                return self.is_subset(other);
            }
        }

        false
    }
}

impl<T: Copy + Ord> Subset<Range<T>> for RangeSet<T> {
    fn is_subset(&self, other: &Range<T>) -> bool {
        let (Some(start), Some(end)) = (self.min(), self.end()) else {
            // empty set is subset of any set
            return true;
        };

        // check if the outer bounds of this set are within the range
        start >= other.start && end <= other.end
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

        let mut i = 0;
        let mut j = 0;

        while i < self.ranges.len() && j < other.ranges.len() {
            let a = &self.ranges[i];
            let b = &other.ranges[j];

            if a.start >= b.end {
                // a is rightward of b, proceed to next b
                j += 1;
            } else if a.is_subset(b) {
                // a is subset of b, proceed to next a
                i += 1;
            } else {
                // self contains values not in other
                return false;
            }
        }

        // If we've reached the end of self, then all ranges are contained in other.
        i == self.ranges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_range_subset_of_range() {
        let a = 10..20;

        // empty
        assert!(!a.is_subset(&(0..0)));
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
    fn test_range_subset_of_rangeset() {
        let a = 10..20;

        let empty = RangeSet::<i32>::default();

        // empty set is subset of any range
        assert!(empty.is_subset(&a));
        // non-empty range is not subset of empty set
        assert!(!a.is_subset(&empty));

        assert!(a.is_subset(&RangeSet::from(vec![0..20])));
        assert!(a.is_subset(&RangeSet::from(vec![10..20])));
        assert!(!a.is_subset(&RangeSet::from(vec![0..10])));
        assert!(!a.is_subset(&RangeSet::from(vec![20..30])));
        assert!(!a.is_subset(&RangeSet::from(vec![0..10, 20..30])));
    }

    #[test]
    fn test_rangeset_subset_of_range() {
        let a = RangeSet::from(vec![10..20, 30..40]);

        assert!(!a.is_subset(&(0..0)));
        assert!(!a.is_subset(&(0..10)));
        assert!(!a.is_subset(&(0..15)));
        assert!(!a.is_subset(&(0..20)));
        assert!(!a.is_subset(&(20..30)));
        assert!(!a.is_subset(&(20..40)));
        assert!(!a.is_subset(&(20..50)));
        assert!(!a.is_subset(&(30..40)));
        assert!(!a.is_subset(&(11..40)));
        assert!(!a.is_subset(&(10..39)));

        assert!(a.is_subset(&(10..40)));
    }

    #[test]
    fn test_rangeset_subset_of_rangeset() {
        let empty = RangeSet::<i32>::default();

        // empty set is subset of itself
        assert!(empty.is_subset(&empty));
        // empty set is subset of non-empty set
        assert!(empty.is_subset(&RangeSet::from(vec![10..20])));
        // non-empty set is not subset of empty set
        assert!(!RangeSet::from(vec![10..20]).is_subset(&empty));

        let a = RangeSet::from(vec![10..20, 30..40]);

        // equal
        assert!(a.is_subset(&a));

        assert!(a.is_subset(&RangeSet::from(vec![0..20, 30..50])));
        assert!(a.is_subset(&RangeSet::from(vec![10..20, 30..40, 40..50])));
        assert!(!a.is_subset(&RangeSet::from(vec![10..20])));
        assert!(!a.is_subset(&RangeSet::from(vec![30..40])));
        assert!(!a.is_subset(&RangeSet::from(vec![0..20, 30..39])));
        assert!(!a.is_subset(&RangeSet::from(vec![10..19, 30..40])));
        assert!(!a.is_subset(&RangeSet::from(vec![0..10, 30..40])));
    }
}
