use crate::{
    Range, RangeSet, difference::DifferenceMut, intersection::Intersection, subset::Subset,
};

/// Set cover methods.
pub trait Cover<Rhs> {
    /// Returns the positions of the fewest sets from `others` which exactly cover `self`.
    fn find_cover<'a>(&self, others: impl IntoIterator<Item = &'a Rhs>) -> Option<Vec<usize>>
    where
        Rhs: 'a;

    /// Returns the fewest sets from `others` which exactly cover `self`.
    fn cover<'a>(&self, others: impl IntoIterator<Item = &'a Rhs>) -> Option<Vec<&'a Rhs>>
    where
        Rhs: 'a;

    /// Returns the fewest sets from `others` which exactly cover `self`.
    ///
    /// # Arguments
    ///
    /// * `others` - The collection of items to cover `self`.
    /// * `f` - A function that extracts a set from an item.
    fn cover_by<'a, T>(
        &self,
        others: impl IntoIterator<Item = &'a T>,
        f: impl Fn(&T) -> &Rhs,
    ) -> Option<Vec<&'a T>>
    where
        T: 'a;
}

impl<T> Cover<RangeSet<T>> for RangeSet<T>
where
    T: Copy + Ord + 'static,
    Range<T>: ExactSizeIterator<Item = T>,
{
    fn find_cover<'a>(
        &self,
        others: impl IntoIterator<Item = &'a RangeSet<T>>,
    ) -> Option<Vec<usize>>
    where
        RangeSet<T>: 'a,
    {
        cover(self, |set| set, others).map(|sets| sets.into_iter().map(|(pos, _)| pos).collect())
    }

    fn cover<'a>(
        &self,
        others: impl IntoIterator<Item = &'a RangeSet<T>>,
    ) -> Option<Vec<&'a RangeSet<T>>>
    where
        RangeSet<T>: 'a,
    {
        cover(self, |set| set, others).map(|sets| sets.into_iter().map(|(_, set)| set).collect())
    }

    fn cover_by<'a, U>(
        &self,
        others: impl IntoIterator<Item = &'a U>,
        f: impl Fn(&U) -> &RangeSet<T>,
    ) -> Option<Vec<&'a U>>
    where
        T: 'a,
    {
        cover(self, f, others).map(|sets| sets.into_iter().map(|(_, item)| item).collect())
    }
}

struct Candidate<'a, T> {
    /// Index in the remaining collection.
    i: usize,
    /// Position in the original collection.
    pos: usize,
    item: &'a T,
    /// The number of elements in the intersection of the set and the uncovered elements.
    cover: usize,
}

/// Greedy set cover algorithm.
///
/// Returns the fewest sets from `others` which exactly cover `query`.
///
/// # Arguments
///
/// * `query` - The set to cover.
/// * `f` - A function that extracts a set from an item.
/// * `others` - The collection of items to cover `query`.
fn cover<'a, T: Copy + Ord + 'static, U: 'a>(
    query: &RangeSet<T>,
    f: impl Fn(&U) -> &RangeSet<T>,
    others: impl IntoIterator<Item = &'a U>,
) -> Option<Vec<(usize, &'a U)>>
where
    Range<T>: ExactSizeIterator<Item = T>,
{
    if query.is_empty() {
        return Some(Default::default());
    }

    // Filter out rangesets that are not a subset of query.
    let mut others: Vec<_> = others
        .into_iter()
        .enumerate()
        .filter_map(|(pos, other)| {
            if f(other).is_subset(query) {
                Some((pos, other))
            } else {
                None
            }
        })
        .collect();

    if others.is_empty() {
        return None;
    }

    let mut uncovered = query.clone();
    let mut candidates = Vec::new();
    let mut candidate: Option<Candidate<'_, U>> = None;
    while !uncovered.is_empty() {
        // Find the set with the most coverage.
        for (i, (pos, item)) in others.iter().enumerate() {
            let cover = f(item).intersection(&uncovered).len();
            // If cover is non-empty or greater than the current candidate, update the candidate.
            if cover > candidate.as_ref().map_or(1, |c| c.cover) {
                candidate = Some(Candidate {
                    i,
                    pos: *pos,
                    item,
                    cover,
                });
            }
        }

        if let Some(Candidate { i, pos, item, .. }) = candidate.take() {
            // Remove the set from the uncovered elements.
            uncovered.difference_mut(f(item));
            // Remove the set from the remaining sets.
            others.swap_remove(i);
            // Add the set to the candidates.
            candidates.push((pos, item));
        } else {
            // If no set was found, we cannot cover the query.
            return None;
        }
    }

    Some(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_rangeset_cover() {
        let query = RangeSet::<u32>::default();
        let others = [RangeSet::from(1..5), RangeSet::from(6..10)];

        let result = query.cover(others.iter()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_missing_rangesets() {
        let query = RangeSet::from(1..5);
        let others: Vec<RangeSet<u32>> = vec![];

        let result = query.cover(others.iter());
        assert!(result.is_none());
    }

    #[test]
    fn test_no_subset_in_others() {
        let query = RangeSet::from(5..10);
        let others = [
            RangeSet::from(1..4),  // Completely outside query
            RangeSet::from(3..7),  // Partially overlaps but not a subset
            RangeSet::from(8..15), // Partially overlaps but not a subset
            RangeSet::from(11..20),
        ];

        let result = query.cover(others.iter());
        assert!(result.is_none());
    }

    #[test]
    fn test_simple_cover() {
        let query = RangeSet::from(1..5);
        let others = [query.clone()];

        let result = query.cover(others.iter()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], &query);
    }

    #[test]
    fn test_simple_cover_with_multi_ranges() {
        let query = RangeSet::from(vec![1..5, 10..15]);
        let others = [query.clone()];

        let result = query.cover(others.iter()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], &query);
    }

    #[test]
    fn test_multiple_subsets_cover() {
        let query = RangeSet::from(1..10);
        let others = [RangeSet::from(1..5), RangeSet::from(5..10)];

        let result = query.cover(others.iter()).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&&others[0]));
        assert!(result.contains(&&others[1]));
    }

    #[test]
    fn test_multi_range_cover_with_multi_range_sets() {
        // query with multiple disjoint ranges
        let query = RangeSet::from(vec![1..5, 10..15, 20..25]);

        // Others with multiple ranges in each RangeSet
        let others = [
            RangeSet::from(vec![1..3, 20..23]), // Covers part of first and third ranges
            RangeSet::from(vec![3..5, 10..12]), // Covers rest of first and part of second
            RangeSet::from(vec![12..15, 23..25]),
        ];

        let result = query.cover(others.iter()).unwrap();

        assert_eq!(result.len(), 3);
        assert!(result.contains(&&others[0]));
        assert!(result.contains(&&others[1]));
        assert!(result.contains(&&others[2]));
    }

    #[allow(clippy::single_range_in_vec_init)]
    #[test]
    fn test_complex_nested_subsets() {
        // query with multiple ranges
        let query = RangeSet::from(vec![1..10, 15..20]);

        // Collection with nested subsets
        let others = [
            RangeSet::from(vec![1..9, 16..20]),
            RangeSet::from(vec![1..5, 16..18]),
            RangeSet::from(2..3),
            RangeSet::from(8..20), // Not a subset
            RangeSet::from(vec![9..10, 15..17]),
            RangeSet::from(vec![21..30]),
        ];

        let result = query.cover(others.iter()).unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains(&&others[0]));
        assert!(result.contains(&&others[4]));
    }

    #[test]
    fn test_unable_to_cover_simple() {
        let query = RangeSet::from(1..10);
        let others = [RangeSet::from(1..5), RangeSet::from(6..10)];

        let result = query.cover(others.iter());
        assert!(result.is_none());
    }

    #[test]
    fn test_unable_to_cover_multiple_ranges() {
        // query with multiple ranges
        let query = RangeSet::from(vec![1..10, 15..25, 30..35]);

        // Collection with multiple ranges in each RangeSet
        let others = [
            RangeSet::from(vec![1..5, 16..20]), // Covers part of first and second ranges
            RangeSet::from(vec![5..8, 21..25]), // Covers part of first and second ranges
            RangeSet::from(vec![15..16, 30..33]), // Covers part of second and third ranges
            RangeSet::from(vec![9..10, 34..35]),
        ];

        let result = query.cover(others.iter());
        assert!(result.is_none());
    }
}
