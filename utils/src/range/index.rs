use super::RangeSet;

/// A trait implemented for collections which can be indexed by a range set.
pub trait IndexRanges<T: Copy + Ord = usize> {
    /// The output type of the indexing operation.
    type Output;

    /// Index the collection by the given range set.
    ///
    /// # Panics
    ///
    /// Panics if any of the indices in the range set are out of bounds of the collection.
    ///
    /// # Examples
    ///
    /// ```
    /// use utils::range::{RangeSet, IndexRanges};
    ///
    /// let data = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    /// let index = RangeSet::from([(0..3), (5..8)]);
    ///
    /// assert_eq!(data.index_ranges(&index), vec![1, 2, 3, 6, 7, 8]);
    /// ```
    fn index_ranges(&self, ranges: &RangeSet<T>) -> Self::Output;
}

impl<T: Clone> IndexRanges for [T] {
    type Output = Vec<T>;

    #[inline]
    fn index_ranges(&self, index: &RangeSet<usize>) -> Self::Output {
        let mut out = Vec::with_capacity(index.len());
        for range in index.iter_ranges() {
            out.extend_from_slice(&self[range]);
        }
        out
    }
}

impl IndexRanges for str {
    type Output = String;

    #[inline]
    fn index_ranges(&self, index: &RangeSet<usize>) -> Self::Output {
        let mut out = String::with_capacity(index.len());
        for range in index.iter_ranges() {
            out.push_str(&self[range]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_ranges_slice() {
        let data = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let index = RangeSet::from([(0..3), (5..8)]);

        assert_eq!(data.index_ranges(&index), vec![1, 2, 3, 6, 7, 8]);
    }

    #[test]
    fn test_index_empty_ranges_slice() {
        let data = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let index = RangeSet::from([]);

        assert_eq!(data.index_ranges(&index), vec![]);
    }

    #[test]
    #[should_panic]
    fn test_index_ranges_out_of_bounds_slice() {
        let data = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let index = RangeSet::from([(0..3), (5..8), (10..12)]);

        data.index_ranges(&index);
    }

    #[test]
    fn test_index_ranges_str() {
        let data = "123456789";
        let index = RangeSet::from([(0..3), (5..8)]);

        assert_eq!(data.index_ranges(&index), "123678");
    }

    #[test]
    fn test_index_empty_ranges_str() {
        let data = "123456789";
        let index = RangeSet::from([]);

        assert_eq!(data.index_ranges(&index), "");
    }

    #[test]
    #[should_panic]
    fn test_index_ranges_out_of_bounds_str() {
        let data = "123456789";
        let index = RangeSet::from([(0..3), (5..8), (10..12)]);

        data.index_ranges(&index);
    }
}
