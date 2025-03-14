use std::fmt;

use crate::range::{
    difference::DifferenceMut, intersection::Intersection, subset::Subset, Range, RangeSet,
};

pub trait Cover<'a, Rhs: 'a> {
    type Error;
    /// Returns subsets from others that can exactly cover self.
    fn cover(
        &self,
        others: impl IntoIterator<Item = &'a Rhs>,
    ) -> Result<impl Iterator<Item = Rhs>, Self::Error>;
}

impl<'a, T: Copy + Ord + 'a> Cover<'a, RangeSet<T>> for RangeSet<T>
where
    Range<T>: ExactSizeIterator<Item = T>,
{
    type Error = RangeSetCoverError;

    /// Uses a greedy algorithm to find the smallest number of subsets from `others` that can exactly cover `self`.
    fn cover(
        &self,
        others: impl IntoIterator<Item = &'a RangeSet<T>>,
    ) -> Result<impl Iterator<Item = RangeSet<T>>, Self::Error> {
        let mut uncovered = self.clone();
        let mut cover_subsets = Vec::new();

        // If self is empty, return an empty iterator.
        if uncovered.is_empty() {
            return Ok(cover_subsets.into_iter());
        }

        // Filter out rangesets that are not a subset of self.
        let mut others = others
            .into_iter()
            .filter(|other| other.is_subset(&uncovered))
            .collect::<Vec<_>>();

        if others.is_empty() {
            return Err(RangeSetCoverError::new(
                ErrorKind::FailedToCover,
                "No given rangesets is a subset of self",
            ));
        }

        while !uncovered.is_empty() {
            let mut largest_cover_size = 0;
            let mut largest_cover_subset = RangeSet::default();
            let mut largest_cover_subset_index = None;

            // Find the subset in others that covers the most of uncovered.
            for (i, candidate) in others.iter().enumerate() {
                let cover_size = candidate.intersection(&uncovered).len();
                if cover_size > largest_cover_size {
                    largest_cover_size = cover_size;
                    largest_cover_subset = (*candidate).clone();
                    largest_cover_subset_index = Some(i);
                }
            }

            if let Some(index) = largest_cover_subset_index {
                cover_subsets.push(largest_cover_subset.clone());
                uncovered.difference_mut(&largest_cover_subset);
                others.swap_remove(index);
            } else {
                return Err(RangeSetCoverError::new(
                    ErrorKind::FailedToCover,
                    "Failed to cover self with given rangesets",
                ));
            }
        }

        Ok(cover_subsets.into_iter())
    }
}

/// Error for [`RangeSetCover`].
#[derive(Debug, thiserror::Error)]
pub struct RangeSetCoverError {
    kind: ErrorKind,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl RangeSetCoverError {
    fn new<E>(kind: ErrorKind, source: E) -> Self
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self {
            kind,
            source: Some(source.into()),
        }
    }
}

#[derive(Debug)]
enum ErrorKind {
    FailedToCover,
}

impl fmt::Display for RangeSetCoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("rangeset cover error: ")?;

        match self.kind {
            ErrorKind::FailedToCover => f.write_str("cover failure error")?,
        }

        if let Some(source) = &self.source {
            write!(f, " caused by: {}", source)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_rangeset_cover() {
        let target = RangeSet::<u32>::default();
        let others = [RangeSet::from(1..5), RangeSet::from(6..10)];

        let result = target.cover(others.iter()).unwrap();
        assert_eq!(
            result.collect::<Vec<RangeSet<u32>>>(),
            Vec::<RangeSet<u32>>::new()
        );
    }

    #[test]
    fn test_missing_rangesets() {
        let target = RangeSet::from(1..5);
        let others: Vec<RangeSet<u32>> = vec![];

        let result = target.cover(others.iter());
        assert!(matches!(
            result,
            Err(RangeSetCoverError {
                kind: ErrorKind::FailedToCover,
                ..
            })
        ));
    }

    #[test]
    fn test_no_subset_in_others() {
        let target = RangeSet::from(5..10);
        let others = [
            RangeSet::from(1..4),  // Completely outside target
            RangeSet::from(3..7),  // Partially overlaps but not a subset
            RangeSet::from(8..15), // Partially overlaps but not a subset
            RangeSet::from(11..20),
        ];

        let result = target.cover(others.iter());
        assert!(matches!(
            result,
            Err(RangeSetCoverError {
                kind: ErrorKind::FailedToCover,
                ..
            })
        ));
    }

    #[test]
    fn test_simple_cover() {
        let target = RangeSet::from(1..5);
        let others = [RangeSet::from(1..5)];

        let result = target.cover(others.iter()).unwrap();
        let cover_sets = result.collect::<Vec<_>>();
        assert_eq!(cover_sets.len(), 1);
        assert_eq!(cover_sets[0], RangeSet::from(1..5));
    }

    #[test]
    fn test_simple_cover_with_multi_ranges() {
        let target = RangeSet::from(vec![1..5, 10..15]);
        let others = [RangeSet::from(vec![1..5, 10..15])];

        let result = target.cover(others.iter()).unwrap();
        let cover_sets = result.collect::<Vec<_>>();
        assert_eq!(cover_sets.len(), 1);
        assert_eq!(cover_sets[0], RangeSet::from(vec![1..5, 10..15]));
    }

    #[test]
    fn test_multiple_subsets_cover() {
        let target = RangeSet::from(1..10);
        let others = [RangeSet::from(1..5), RangeSet::from(5..10)];

        let result = target.cover(others.iter()).unwrap();
        let cover_sets = result.collect::<Vec<_>>();
        assert_eq!(cover_sets.len(), 2);
        assert!(cover_sets.contains(&RangeSet::from(1..5)));
        assert!(cover_sets.contains(&RangeSet::from(5..10)));
    }

    #[test]
    fn test_multi_range_cover_with_multi_range_sets() {
        // Target with multiple disjoint ranges
        let target = RangeSet::from(vec![1..5, 10..15, 20..25]);

        // Others with multiple ranges in each RangeSet
        let others = [
            RangeSet::from(vec![1..3, 20..23]), // Covers part of first and third ranges
            RangeSet::from(vec![3..5, 10..12]), // Covers rest of first and part of second
            RangeSet::from(vec![12..15, 23..25]),
        ];

        let result = target.cover(others.iter()).unwrap();
        let cover_sets = result.collect::<Vec<_>>();

        assert_eq!(cover_sets.len(), 3);
        assert!(cover_sets.contains(&RangeSet::from(vec![1..3, 20..23])));
        assert!(cover_sets.contains(&RangeSet::from(vec![3..5, 10..12])));
        assert!(cover_sets.contains(&RangeSet::from(vec![12..15, 23..25])));
    }

    #[allow(clippy::single_range_in_vec_init)]
    #[test]
    fn test_complex_nested_subsets() {
        // Target with multiple ranges
        let target = RangeSet::from(vec![1..10, 15..20]);

        // Collection with nested subsets
        let others = [
            RangeSet::from(vec![1..9, 16..20]),
            RangeSet::from(vec![1..5, 16..18]),
            RangeSet::from(2..3),
            RangeSet::from(8..20), // Not a subset
            RangeSet::from(vec![9..10, 15..17]),
            RangeSet::from(vec![21..30]),
        ];

        let result = target.cover(others.iter()).unwrap();
        let cover_sets = result.collect::<Vec<_>>();

        assert_eq!(cover_sets.len(), 2);
        assert!(cover_sets.contains(&RangeSet::from(vec![1..9, 16..20])));
        assert!(cover_sets.contains(&RangeSet::from(vec![9..10, 15..17])));
    }

    #[test]
    fn test_unable_to_cover_simple() {
        let target = RangeSet::from(1..10);
        let others = [RangeSet::from(1..5), RangeSet::from(6..10)];

        let result = target.cover(others.iter());
        assert!(matches!(
            result,
            Err(RangeSetCoverError {
                kind: ErrorKind::FailedToCover,
                ..
            })
        ));
    }

    #[test]
    fn test_unable_to_cover_multiple_ranges() {
        // Target with multiple ranges
        let target = RangeSet::from(vec![1..10, 15..25, 30..35]);

        // Collection with multiple ranges in each RangeSet
        let others = [
            RangeSet::from(vec![1..5, 16..20]), // Covers part of first and second ranges
            RangeSet::from(vec![5..8, 21..25]), // Covers part of first and second ranges
            RangeSet::from(vec![15..16, 30..33]), // Covers part of second and third ranges
            RangeSet::from(vec![9..10, 34..35]),
        ];

        let result = target.cover(others.iter());
        assert!(matches!(
            result,
            Err(RangeSetCoverError {
                kind: ErrorKind::FailedToCover,
                ..
            })
        ));
    }
}
