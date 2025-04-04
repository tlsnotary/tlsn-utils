use crate::{
    Range, RangeSet, difference::DifferenceMut, intersection::Intersection, subset::Subset,
};

/// Set cover methods.
pub trait Cover<Rhs> {
    /// Returns the positions of the fewest sets from `others` which exactly
    /// cover `self`. If failed, returns the positions of the sets that
    /// partially cover `self` and the uncovered values.
    fn find_cover<'a>(
        &self,
        others: impl IntoIterator<Item = &'a Rhs>,
    ) -> Result<Vec<usize>, (Vec<usize>, Rhs)>
    where
        Rhs: 'a;

    /// Returns the fewest sets from `others` which exactly cover `self`.
    /// If failed, returns the sets that partially cover `self` and the
    /// uncovered values.
    fn cover<'a>(
        &self,
        others: impl IntoIterator<Item = &'a Rhs>,
    ) -> Result<Vec<&'a Rhs>, (Vec<&'a Rhs>, Rhs)>
    where
        Rhs: 'a;

    /// Returns the fewest sets from `others` which exactly cover `self`.
    /// If failed, returns the sets that partially cover `self` and the
    /// uncovered values.
    ///
    /// # Arguments
    ///
    /// * `others` - The collection of items to cover `self`.
    /// * `f` - A function that extracts a set from an item.
    fn cover_by<'a, T>(
        &self,
        others: impl IntoIterator<Item = &'a T>,
        f: impl Fn(&T) -> &Rhs,
    ) -> Result<Vec<&'a T>, (Vec<&'a T>, Rhs)>
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
    ) -> Result<Vec<usize>, (Vec<usize>, RangeSet<T>)>
    where
        RangeSet<T>: 'a,
    {
        cover(self, |set| set, others)
            .map(|sets| sets.into_iter().map(|(pos, _)| pos).collect())
            .map_err(|PartialCover { covered, uncovered }| {
                (covered.into_iter().map(|(pos, _)| pos).collect(), uncovered)
            })
    }

    fn cover<'a>(
        &self,
        others: impl IntoIterator<Item = &'a RangeSet<T>>,
    ) -> Result<Vec<&'a RangeSet<T>>, (Vec<&'a RangeSet<T>>, RangeSet<T>)>
    where
        RangeSet<T>: 'a,
    {
        cover(self, |set| set, others)
            .map(|sets| sets.into_iter().map(|(_, set)| set).collect())
            .map_err(|PartialCover { covered, uncovered }| {
                (covered.into_iter().map(|(_, set)| set).collect(), uncovered)
            })
    }

    fn cover_by<'a, U>(
        &self,
        others: impl IntoIterator<Item = &'a U>,
        f: impl Fn(&U) -> &RangeSet<T>,
    ) -> Result<Vec<&'a U>, (Vec<&'a U>, RangeSet<T>)>
    where
        T: 'a,
    {
        cover(self, f, others)
            .map(|sets| sets.into_iter().map(|(_, item)| item).collect())
            .map_err(|PartialCover { covered, uncovered }| {
                (
                    covered.into_iter().map(|(_, item)| item).collect(),
                    uncovered,
                )
            })
    }
}

struct Candidate<'a, T> {
    /// Index in the remaining collection.
    i: usize,
    /// Position in the original collection.
    pos: usize,
    item: &'a T,
    /// The number of elements in the intersection of the set and the uncovered
    /// elements.
    cover: usize,
}

struct PartialCover<'a, T, U> {
    covered: Vec<(usize, &'a U)>,
    uncovered: RangeSet<T>,
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
) -> Result<Vec<(usize, &'a U)>, PartialCover<'a, T, U>>
where
    Range<T>: ExactSizeIterator<Item = T>,
{
    if query.is_empty() {
        return Ok(Default::default());
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
        return Err(PartialCover {
            covered: Vec::default(),
            uncovered: query.clone(),
        });
    }

    let mut uncovered = query.clone();
    let mut candidates = Vec::new();
    let mut candidate: Option<Candidate<'_, U>> = None;
    while !uncovered.is_empty() {
        // Find the set with the most coverage.
        for (i, (pos, item)) in others.iter().enumerate() {
            let cover = f(item).intersection(&uncovered).len();
            // If cover is non-empty or greater than the current candidate, update the
            // candidate.
            if cover > candidate.as_ref().map_or(0, |c| c.cover) {
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
            return Err(PartialCover {
                covered: candidates,
                uncovered,
            });
        }
    }

    Ok(candidates)
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

        let (partial, uncovered) = query.cover(others.iter()).unwrap_err();
        assert!(partial.is_empty());
        assert_eq!(uncovered, query);
    }

    #[test]
    fn test_no_subset_in_others() {
        let query = RangeSet::from(5..10);
        let others = [
            RangeSet::from(1..4),   // Completely outside query
            RangeSet::from(3..7),   // Partially overlaps but not a subset
            RangeSet::from(8..15),  // Partially overlaps but not a subset
            RangeSet::from(11..20), // Completely outside query
        ];

        let (partial, uncovered) = query.cover(others.iter()).unwrap_err();
        assert!(partial.is_empty());
        assert_eq!(uncovered, query);
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
        // Query with multiple disjoint ranges
        let query = RangeSet::from(vec![1..5, 10..15, 20..25]);

        // Others with multiple ranges in each RangeSet
        let others = [
            RangeSet::from(vec![1..3, 20..23]), // Covers part of first and third ranges
            RangeSet::from(vec![3..5, 10..12]), // Covers rest of first and part of second
            RangeSet::from(vec![12..15, 23..25]), // Covers part of second and third ranges
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
        // Query with multiple ranges
        let query = RangeSet::from(vec![1..10, 15..20]);

        // Collection with nested subsets
        let others = [
            RangeSet::from(vec![1..9, 16..20]),
            RangeSet::from(vec![1..5, 16..18]),
            RangeSet::from(2..3),
            RangeSet::from(8..20), // Not a subset
            RangeSet::from(vec![9..10, 15..17]),
            RangeSet::from(vec![21..30]), // Not a subset
        ];

        let result = query.cover(others.iter()).unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains(&&others[0]));
        assert!(result.contains(&&others[4]));
    }

    #[test]
    fn test_unable_to_cover_simple() {
        let query = RangeSet::from(1..10);
        let others = [&RangeSet::from(1..5), &RangeSet::from(6..10)];

        let (partial, uncovered) = query.cover(others.into_iter()).unwrap_err();
        assert_eq!(partial, others);
        assert_eq!(uncovered, RangeSet::from(5..6));
    }

    #[test]
    fn test_unable_to_cover_multiple_ranges() {
        // Query with multiple ranges
        let query = RangeSet::from(vec![1..10, 15..25, 30..35]);

        // Collection with multiple ranges in each RangeSet
        let others = [
            &RangeSet::from(vec![1..5, 16..20]), // Covers part of first and second ranges
            &RangeSet::from(vec![5..8, 21..25]), // Covers part of first and second ranges
            &RangeSet::from(vec![15..16, 30..33]), // Covers part of second and third ranges
            &RangeSet::from(vec![9..10, 34..35]), // Covers part of first and third ranges
        ];

        let (partial, uncovered) = query.cover(others.into_iter()).unwrap_err();
        assert_eq!(partial, others);
        assert_eq!(uncovered, RangeSet::from([8..9, 20..21, 33..34,]));
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn test_length_one_subset() {
        let query: RangeSet<i32> = RangeSet::from(vec![1..2]);

        // A subset with a range of length 1.
        let others = [RangeSet::from(vec![1..2])];

        let result = query.cover(others.iter());
        assert!(result.is_ok());
    }
}
